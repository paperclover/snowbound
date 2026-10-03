//! Frames reach the window through Metal, or through OpenGL on its view where chosen.

#[path = "surface_gl.rs"]
mod gl;
#[path = "surface.rs"]
mod metal;

use crate::settings::Backend as Chosen;
use draw::{Renderer, Target};
use std::{error::Error, sync::Arc};
use winit::window::Window;

pub struct Surface {
    /// Device pixels frames and snapshots are drawn at; `configure` gives the window it.
    pub size: [u32; 2],
    backend: Backend,
}

enum Backend {
    Metal(Box<metal::Surface>),
    Gl(gl::Surface),
}

pub struct Frame {
    pub target: Target,
    /// The drawable's texture, and whether it asks for reconfiguring, where Metal draws.
    texture: Option<(wgpu::SurfaceTexture, bool)>,
}

pub struct Offscreen {
    pub target: Target,
    texture: Option<wgpu::Texture>,
}

impl Surface {
    /// Draws with `chosen`; with `backdrop`, frames leave the window's material showing
    /// through their transparent pixels. Answers the adapter's name too.
    pub async fn new(
        window: Arc<Window>,
        backdrop: bool,
        chosen: Chosen,
    ) -> Result<(Self, Renderer, String), Box<dyn Error>> {
        let size = window.inner_size();
        let size = [size.width, size.height];
        // OpenGL draws beneath the window's material, so draws opaque without it.
        let opengl = chosen == Chosen::Gl;
        if backdrop {
            crate::platform::show_backdrop(&window, !opengl);
        }
        let (backend, renderer, adapter) = if opengl {
            let (surface, renderer, adapter) = gl::Surface::new(window, false, chosen).await?;
            (Backend::Gl(surface), renderer, adapter)
        } else {
            let (surface, renderer, adapter) =
                metal::Surface::new(window, backdrop, chosen).await?;
            (Backend::Metal(Box::new(surface)), renderer, adapter)
        };
        Ok((Self { size, backend }, renderer, adapter))
    }

    /// Whether frames leave the system's backdrop showing.
    pub fn translucent(&self) -> bool {
        match &self.backend {
            Backend::Metal(surface) => surface.translucent(),
            Backend::Gl(surface) => surface.translucent(),
        }
    }

    pub fn configure(&mut self, renderer: &Renderer) {
        match &mut self.backend {
            Backend::Metal(surface) => {
                surface.size = self.size;
                surface.configure(renderer);
            }
            Backend::Gl(surface) => {
                surface.size = self.size;
                surface.configure(renderer);
            }
        }
    }

    /// The window's next frame, or none to skip this one.
    pub fn frame(&mut self, renderer: &Renderer) -> Result<Option<Frame>, Box<dyn Error>> {
        Ok(match &mut self.backend {
            Backend::Metal(surface) => {
                surface.size = self.size;
                surface.frame(renderer)?.map(|frame| Frame {
                    target: frame.target,
                    texture: Some((frame.texture, frame.reconfigure)),
                })
            }
            Backend::Gl(surface) => {
                surface.size = self.size;
                surface.frame(renderer)?.map(|frame| Frame {
                    target: frame.target,
                    texture: None,
                })
            }
        })
    }

    pub fn present(&mut self, renderer: &Renderer, frame: Frame) {
        match (&mut self.backend, frame.texture) {
            (Backend::Metal(surface), Some((texture, reconfigure))) => {
                let frame = metal::Frame {
                    target: frame.target,
                    texture,
                    reconfigure,
                };
                surface.present(renderer, frame);
            }
            (Backend::Gl(surface), None) => surface.present(
                renderer,
                gl::Frame {
                    target: frame.target,
                },
            ),
            _ => unreachable!("Frames come from the surface's own backend"),
        }
    }

    /// A target the size of the window's frames that `read` reads back.
    pub fn offscreen(&mut self, renderer: &Renderer) -> Result<Offscreen, Box<dyn Error>> {
        match &mut self.backend {
            Backend::Metal(surface) => {
                surface.size = self.size;
                let offscreen = surface.offscreen(renderer)?;
                Ok(Offscreen {
                    target: offscreen.target,
                    texture: Some(offscreen.texture),
                })
            }
            Backend::Gl(surface) => {
                surface.size = self.size;
                Ok(Offscreen {
                    target: surface.offscreen(renderer)?.target,
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
        match (&self.backend, offscreen.texture) {
            (Backend::Metal(surface), Some(texture)) => surface.read(
                renderer,
                metal::Offscreen {
                    target: offscreen.target,
                    texture,
                },
            ),
            (Backend::Gl(surface), None) => surface.read(
                renderer,
                gl::Frame {
                    target: offscreen.target,
                },
            ),
            _ => unreachable!("Snapshots come from the surface's own backend"),
        }
    }
}
