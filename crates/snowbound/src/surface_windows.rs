//! Frames reach the window through Direct3D 12 where Windows has it (10 and 11), and
//! otherwise, as on Windows 7, through OpenGL 2.1 on a WGL context: frames draw into an sRGB
//! target, copied to the window's back buffer and swapped.

#[path = "surface.rs"]
mod webgpu;

use draw::{GlTarget, Renderer, Target};
use std::{error::Error, sync::Arc};
use windows_sys::Win32::{
    Foundation::HWND,
    Graphics::{
        Gdi::{GetDC, HDC},
        OpenGL as gl,
    },
};
use winit::{
    raw_window_handle::{HasWindowHandle, RawWindowHandle},
    window::Window,
};

pub struct Surface {
    /// Device pixels frames and snapshots are drawn at; `configure` gives the window it.
    pub size: [u32; 2],
    backend: Backend,
}

enum Backend {
    Wgpu(Box<webgpu::Surface>),
    Gl {
        /// The window's device context, which the context draws to.
        device: HDC,
        /// Frames leave the desktop's glass showing through their transparent pixels.
        translucent: bool,
        /// The frame target, kept between frames.
        target: Option<GlTarget>,
    },
}

pub struct Frame {
    pub target: Target,
    /// The swap chain's texture, and whether it asks for reconfiguring, where Direct3D
    /// draws.
    texture: Option<(wgpu::SurfaceTexture, bool)>,
}

pub struct Offscreen {
    pub target: Target,
    texture: Option<wgpu::Texture>,
}

impl Surface {
    /// With `backdrop`, frames leave the system's material showing through their
    /// transparent pixels.
    pub async fn new(
        window: Arc<Window>,
        backdrop: bool,
    ) -> Result<(Self, Renderer), Box<dyn Error>> {
        let forced = std::env::var("SNOWBOUND_RENDERER").ok();
        if forced.as_deref() != Some("gl") {
            match webgpu::Surface::new(window.clone(), backdrop).await {
                Ok((surface, renderer)) => {
                    let backend = Backend::Wgpu(Box::new(surface));
                    let size = window.inner_size();
                    let size = [size.width, size.height];
                    return Ok((Self { size, backend }, renderer));
                }
                Err(error) if forced.is_some() => return Err(error),
                Err(error) => eprintln!("No Direct3D 12 ({error}); drawing with OpenGL"),
            }
        }
        let RawWindowHandle::Win32(handle) = window.window_handle()?.as_raw() else {
            unreachable!()
        };
        let device = unsafe { GetDC(handle.hwnd.get() as HWND) };
        let request = |alpha| gl::PIXELFORMATDESCRIPTOR {
            nSize: size_of::<gl::PIXELFORMATDESCRIPTOR>() as u16,
            nVersion: 1,
            dwFlags: gl::PFD_DRAW_TO_WINDOW
                | gl::PFD_SUPPORT_OPENGL
                | gl::PFD_DOUBLEBUFFER
                | gl::PFD_SUPPORT_COMPOSITION,
            iPixelType: gl::PFD_TYPE_RGBA,
            cColorBits: 32,
            cAlphaBits: alpha,
            iLayerType: gl::PFD_MAIN_PLANE as u8,
            ..unsafe { std::mem::zeroed() }
        };
        // A driver with no accelerated format that has alpha answers with Windows' own
        // OpenGL 1.1; frames then go without the glass behind them.
        let accelerated = |format: &gl::PIXELFORMATDESCRIPTOR| unsafe {
            let chosen = gl::ChoosePixelFormat(device, format);
            let mut found = std::mem::zeroed();
            let size = size_of::<gl::PIXELFORMATDESCRIPTOR>() as u32;
            (chosen != 0
                && gl::DescribePixelFormat(device, chosen, size, &mut found) != 0
                && (found.dwFlags & gl::PFD_GENERIC_FORMAT == 0
                    || found.dwFlags & gl::PFD_GENERIC_ACCELERATED != 0))
                .then_some(chosen)
        };
        let mut format = request(if backdrop { 8 } else { 0 });
        let mut translucent = backdrop;
        let mut chosen = accelerated(&format);
        if chosen.is_none() && backdrop {
            eprintln!("No accelerated OpenGL format with alpha; drawing opaque");
            format = request(0);
            translucent = false;
            chosen = accelerated(&format);
        }
        unsafe {
            let chosen = chosen.unwrap_or_else(|| gl::ChoosePixelFormat(device, &format));
            if chosen == 0 || gl::SetPixelFormat(device, chosen, &format) == 0 {
                return Err("No OpenGL pixel format".into());
            }
            let context = gl::wglCreateContext(device);
            if context.is_null() || gl::wglMakeCurrent(device, context) == 0 {
                return Err("No OpenGL context".into());
            }
            // WGL_EXT_swap_control: present on the display's refresh.
            type SwapInterval = unsafe extern "system" fn(i32) -> i32;
            if let Some(interval) = gl::wglGetProcAddress(c"wglSwapIntervalEXT".as_ptr().cast()) {
                std::mem::transmute::<unsafe extern "system" fn() -> isize, SwapInterval>(interval)(
                    1,
                );
            }
        }
        let string = |name| unsafe {
            let text = gl::glGetString(name);
            if text.is_null() {
                String::new()
            } else {
                std::ffi::CStr::from_ptr(text.cast())
                    .to_string_lossy()
                    .into_owned()
            }
        };
        let driver = format!(
            "OpenGL {} ({}, {})",
            string(gl::GL_VERSION),
            string(gl::GL_RENDERER),
            string(gl::GL_VENDOR)
        );
        eprintln!("Canvas GPU: {driver}");
        let renderer = Renderer::opengl().map_err(|error| {
            format!(
                "Snowbound needs OpenGL 2.1, and this display driver has {driver}. Install \
                 the driver from your graphics card's maker. ({error})"
            )
        })?;
        let size = window.inner_size();
        Ok((
            Self {
                size: [size.width, size.height],
                backend: Backend::Gl {
                    device,
                    translucent,
                    target: None,
                },
            },
            renderer,
        ))
    }

    /// Whether frames leave the system's backdrop showing.
    pub fn translucent(&self) -> bool {
        match &self.backend {
            Backend::Wgpu(surface) => surface.translucent(),
            Backend::Gl { translucent, .. } => *translucent,
        }
    }

    pub fn configure(&mut self, renderer: &Renderer) {
        if let Backend::Wgpu(surface) = &mut self.backend {
            surface.size = self.size;
            surface.configure(renderer);
        }
    }

    /// The window's next frame, or none to skip this one.
    pub fn frame(&mut self, renderer: &Renderer) -> Result<Option<Frame>, Box<dyn Error>> {
        if self.size.contains(&0) {
            return Ok(None);
        }
        match &mut self.backend {
            Backend::Wgpu(surface) => {
                surface.size = self.size;
                Ok(surface.frame(renderer)?.map(|frame| Frame {
                    target: frame.target,
                    texture: Some((frame.texture, frame.reconfigure)),
                }))
            }
            Backend::Gl { target, .. } => {
                let target = match target.take() {
                    Some(target) if target.size() == self.size => target,
                    _ => GlTarget::new(self.size)?,
                };
                Ok(Some(Frame {
                    target: target.into(),
                    texture: None,
                }))
            }
        }
    }

    pub fn present(&mut self, renderer: &Renderer, frame: Frame) {
        match (&mut self.backend, frame.target, frame.texture) {
            (Backend::Wgpu(surface), target, Some((texture, reconfigure))) => {
                let frame = webgpu::Frame {
                    target,
                    texture,
                    reconfigure,
                };
                surface.present(renderer, frame);
            }
            (
                Backend::Gl {
                    device,
                    translucent,
                    target: kept,
                },
                Target::Gl(target),
                None,
            ) => {
                if *translucent {
                    renderer.present_translucent(&target);
                } else {
                    target.present();
                }
                unsafe { gl::SwapBuffers(*device) };
                *kept = Some(target);
            }
            _ => unreachable!("Frames come from the surface's own backend"),
        }
    }

    /// A target the size of the window's frames that `read` reads back.
    pub fn offscreen(&mut self, renderer: &Renderer) -> Result<Offscreen, Box<dyn Error>> {
        match &mut self.backend {
            Backend::Wgpu(surface) => {
                surface.size = self.size;
                let offscreen = surface.offscreen(renderer)?;
                Ok(Offscreen {
                    target: offscreen.target,
                    texture: Some(offscreen.texture),
                })
            }
            Backend::Gl { .. } => Ok(Offscreen {
                target: GlTarget::new(self.size)?.into(),
                texture: None,
            }),
        }
    }

    /// The offscreen target's sRGB RGBA rows, top first.
    pub fn read(
        &self,
        renderer: &Renderer,
        offscreen: Offscreen,
    ) -> Result<Vec<u8>, Box<dyn Error>> {
        match (&self.backend, offscreen.target, offscreen.texture) {
            (Backend::Wgpu(surface), target, Some(texture)) => {
                surface.read(renderer, webgpu::Offscreen { target, texture })
            }
            (Backend::Gl { .. }, Target::Gl(target), None) => Ok(target.read_pixels()),
            _ => unreachable!("Snapshots come from the surface's own backend"),
        }
    }
}
