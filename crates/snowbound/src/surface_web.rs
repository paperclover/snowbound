//! Frames reach the page through WebGPU or WebGL2, by wgpu, or else through its 2D canvas.
//! Each start takes a canvas of its own, as a canvas keeps the first kind of context it
//! hands out, which the page shows once `show` is called.

#[path = "surface.rs"]
mod webgpu;

use crate::{Window, platform, settings::Backend as Chosen};
use draw::{Renderer, Target};
use std::{error::Error, sync::Arc};
use web_sys::HtmlCanvasElement;

pub struct Surface {
    /// Device pixels frames and snapshots are drawn at; `configure` gives the window it.
    pub size: [u32; 2],
    canvas: HtmlCanvasElement,
    /// wgpu's surface, or none where the 2D canvas draws.
    wgpu: Option<Box<webgpu::Surface>>,
}

pub struct Frame {
    pub target: Target,
    /// The canvas's texture, and whether it asks for reconfiguring, where wgpu draws.
    texture: Option<(wgpu::SurfaceTexture, bool)>,
}

pub struct Offscreen {
    pub target: Target,
    texture: Option<wgpu::Texture>,
}

impl Surface {
    /// Draws with `chosen` into a canvas of its own. Answers the adapter's name too.
    pub async fn new(
        window: Arc<Window>,
        backdrop: bool,
        chosen: Chosen,
    ) -> Result<(Self, Renderer, String), Box<dyn Error>> {
        let canvas = platform::fresh_canvas()?;
        let size = window.inner_size();
        let size = [size.width, size.height];
        if chosen == Chosen::Canvas2d {
            Target::canvas2d(canvas.clone())?;
            let surface = Self {
                size,
                canvas,
                wgpu: None,
            };
            return Ok((surface, Renderer::canvas2d()?, chosen.label().into()));
        }
        // wgpu's own errors say no more than this to someone choosing a renderer.
        let offered = match chosen {
            Chosen::Webgpu => web_sys::window().is_some_and(|window| {
                js_sys::Reflect::has(&window.navigator(), &"gpu".into()).unwrap_or(false)
            }),
            // On a canvas of its own: a context keeps the settings it was first asked with.
            _ => (platform::fresh_canvas()?
                .get_context("webgl2")
                .ok()
                .flatten())
            .is_some(),
        };
        if !offered {
            return Err("Not in this browser".into());
        }
        let (wgpu, renderer, adapter) =
            webgpu::Surface::new(window, backdrop, chosen, canvas.clone()).await?;
        let surface = Self {
            size,
            canvas,
            wgpu: Some(Box::new(wgpu)),
        };
        Ok((surface, renderer, adapter))
    }

    /// Puts the surface's canvas in the page in place of the one there.
    pub fn show(&self) {
        platform::adopt_canvas(&self.canvas);
    }

    pub fn translucent(&self) -> bool {
        false
    }

    pub fn configure(&mut self, renderer: &Renderer) {
        if let Some(surface) = &mut self.wgpu {
            surface.size = self.size;
            surface.configure(renderer);
        }
    }

    /// The page's next frame, or none to skip this one.
    pub fn frame(&mut self, renderer: &Renderer) -> Result<Option<Frame>, Box<dyn Error>> {
        if self.size.contains(&0) {
            return Ok(None);
        }
        Ok(match &mut self.wgpu {
            Some(surface) => {
                surface.size = self.size;
                surface.frame(renderer)?.map(|frame| Frame {
                    target: frame.target,
                    texture: Some((frame.texture, frame.reconfigure)),
                })
            }
            None => Some(Frame {
                target: Target::canvas2d(self.canvas.clone())?,
                texture: None,
            }),
        })
    }

    /// Hands wgpu's frame to the canvas; the 2D canvas shows what was drawn once the frame's
    /// task ends.
    pub fn present(&mut self, renderer: &Renderer, frame: Frame) {
        if let (Some(surface), Some((texture, reconfigure))) = (&mut self.wgpu, frame.texture) {
            let frame = webgpu::Frame {
                target: frame.target,
                texture,
                reconfigure,
            };
            surface.present(renderer, frame);
        }
    }

    /// A target the size of the page's frames that `read` reads back.
    pub fn offscreen(&mut self, renderer: &Renderer) -> Result<Offscreen, Box<dyn Error>> {
        match &mut self.wgpu {
            Some(surface) => {
                surface.size = self.size;
                let offscreen = surface.offscreen(renderer)?;
                Ok(Offscreen {
                    target: offscreen.target,
                    texture: Some(offscreen.texture),
                })
            }
            None => {
                let canvas = platform::fresh_canvas()?;
                canvas.set_width(self.size[0]);
                canvas.set_height(self.size[1]);
                Ok(Offscreen {
                    target: Target::canvas2d(canvas)?,
                    texture: None,
                })
            }
        }
    }

    /// The offscreen target's sRGB RGBA rows, top first.
    pub fn read(
        &self,
        renderer: &Renderer,
        offscreen: Offscreen,
    ) -> Result<Vec<u8>, Box<dyn Error>> {
        match (&self.wgpu, offscreen.texture) {
            (Some(surface), Some(texture)) => surface.read(
                renderer,
                webgpu::Offscreen {
                    target: offscreen.target,
                    texture,
                },
            ),
            (None, None) => Ok(renderer.read_pixels(&offscreen.target)?),
            _ => unreachable!("Snapshots come from the surface's own backend"),
        }
    }
}
