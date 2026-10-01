//! Frames reach the window through an OpenGL context on its view, where wgpu has no
//! backend: Mac OS X 10.6. Frames draw into an sRGB target and are copied to the view.

use draw::{Renderer, Target};
use objc2::{
    ffi::NSInteger,
    msg_send, msg_send_id,
    rc::{Allocated, Retained},
    runtime::{AnyClass, AnyObject},
};
use std::{error::Error, sync::Arc};
use winit::{
    raw_window_handle::{HasWindowHandle, RawWindowHandle},
    window::Window,
};

pub struct Surface {
    /// Device pixels frames and snapshots are drawn at; `configure` gives the window it.
    pub size: [u32; 2],
    context: Retained<AnyObject>,
    /// Frames leave the window's textured backdrop showing through their transparent
    /// pixels.
    translucent: bool,
    /// The frame target, kept between frames.
    target: Option<Target>,
}

pub struct Frame {
    pub target: Target,
}

pub type Offscreen = Frame;

impl Surface {
    /// With `backdrop`, the surface is transparent where frames are.
    pub async fn new(
        window: Arc<Window>,
        backdrop: bool,
    ) -> Result<(Self, Renderer), Box<dyn Error>> {
        let RawWindowHandle::AppKit(handle) = window.window_handle()?.as_raw() else {
            unreachable!()
        };
        // NSOpenGLPFADoubleBuffer, NSOpenGLPFAAccelerated, NSOpenGLPFAColorSize 24,
        // NSOpenGLPFAAlphaSize 8.
        let attributes: [u32; 7] = [5, 73, 8, 24, 11, 8, 0];
        let context = unsafe {
            let class = |name| AnyClass::get(name).expect("AppKit is linked");
            let format: Allocated<AnyObject> = msg_send_id![class("NSOpenGLPixelFormat"), alloc];
            let format: Option<Retained<AnyObject>> =
                msg_send_id![format, initWithAttributes: attributes.as_ptr()];
            let format = format.ok_or("No accelerated OpenGL pixel format")?;
            let context: Allocated<AnyObject> = msg_send_id![class("NSOpenGLContext"), alloc];
            let context: Option<Retained<AnyObject>> = msg_send_id![
                context,
                initWithFormat: &*format,
                shareContext: std::ptr::null::<AnyObject>()
            ];
            let context = context.ok_or("No OpenGL context")?;
            let view = handle.ns_view.as_ptr().cast::<AnyObject>();
            let _: () = msg_send![&context, setView: view];
            let _: () = msg_send![&context, makeCurrentContext];
            // NSOpenGLCPSwapInterval: present on the display's refresh.
            let interval: i32 = 1;
            let _: () = msg_send![&context, setValues: &interval, forParameter: 222 as NSInteger];
            if backdrop {
                // NSOpenGLCPSurfaceOpacity.
                let opaque: i32 = 0;
                let _: () = msg_send![&context, setValues: &opaque, forParameter: 236 as NSInteger];
            }
            context
        };
        let renderer = Renderer::opengl()?;
        eprintln!("Canvas GPU: OpenGL");
        let size = window.inner_size();
        Ok((
            Self {
                size: [size.width, size.height],
                context,
                translucent: backdrop,
                target: None,
            },
            renderer,
        ))
    }

    pub fn translucent(&self) -> bool {
        self.translucent
    }

    pub fn configure(&mut self, _: &Renderer) {
        let _: () = unsafe { msg_send![&self.context, update] };
    }

    /// The window's next frame, or none to skip this one.
    pub fn frame(&mut self, renderer: &Renderer) -> Result<Option<Frame>, Box<dyn Error>> {
        if self.size.contains(&0) {
            return Ok(None);
        }
        let target = match self.target.take() {
            Some(target) if target.size() == self.size => target,
            _ => renderer.target(self.size)?,
        };
        Ok(Some(Frame { target }))
    }

    pub fn present(&mut self, renderer: &Renderer, frame: Frame) {
        renderer.present(&frame.target, self.translucent);
        let _: () = unsafe { msg_send![&self.context, flushBuffer] };
        self.target = Some(frame.target);
    }

    /// A target the size of the window's frames that `read` reads back.
    pub fn offscreen(&self, renderer: &Renderer) -> Result<Offscreen, Box<dyn Error>> {
        Ok(Frame {
            target: renderer.target(self.size)?,
        })
    }

    /// The offscreen target's sRGB RGBA rows, top first.
    pub fn read(
        &self,
        renderer: &Renderer,
        offscreen: Offscreen,
    ) -> Result<Vec<u8>, Box<dyn Error>> {
        Ok(renderer.read_pixels(&offscreen.target)?)
    }
}
