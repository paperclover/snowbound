//! Every backend in one build, for Windows: wgpu where Direct3D 12 answers, Direct3D 11
//! where it doesn't, as on Windows 7, and OpenGL 2.1 where neither does. Whoever makes the
//! renderer picks one.
use super::{d3d11, gl, webgpu, *};
use std::ffi::c_void;

/// What a frame is drawn into, for the backend the renderer draws with.
pub enum Target {
    Wgpu(wgpu::TextureView),
    D3d11(d3d11::Target),
    Gl(gl::Target),
}

impl From<wgpu::TextureView> for Target {
    fn from(view: wgpu::TextureView) -> Self {
        Self::Wgpu(view)
    }
}

impl Target {
    pub fn size(&self) -> [u32; 2] {
        match self {
            Self::Wgpu(view) => [view.texture().width(), view.texture().height()],
            Self::D3d11(target) => target.size(),
            Self::Gl(target) => target.size(),
        }
    }
}

pub(super) enum Image {
    Wgpu(webgpu::Image),
    D3d11(d3d11::Image),
    Gl(gl::Image),
}

impl AsRef<webgpu::Image> for Image {
    fn as_ref(&self) -> &webgpu::Image {
        match self {
            Self::Wgpu(image) => image,
            _ => unreachable!("Images belong to the renderer's backend"),
        }
    }
}

impl AsRef<d3d11::Image> for Image {
    fn as_ref(&self) -> &d3d11::Image {
        match self {
            Self::D3d11(image) => image,
            _ => unreachable!("Images belong to the renderer's backend"),
        }
    }
}

impl AsRef<gl::Image> for Image {
    fn as_ref(&self) -> &gl::Image {
        match self {
            Self::Gl(image) => image,
            _ => unreachable!("Images belong to the renderer's backend"),
        }
    }
}

pub(super) enum Gpu {
    Wgpu(webgpu::Gpu),
    D3d11(d3d11::Gpu),
    Gl(gl::Gpu),
}

impl From<webgpu::Gpu> for Gpu {
    fn from(gpu: webgpu::Gpu) -> Self {
        Self::Wgpu(gpu)
    }
}

impl From<gl::Gpu> for Gpu {
    fn from(gpu: gl::Gpu) -> Self {
        Self::Gl(gpu)
    }
}

impl Renderer {
    /// Draws with Direct3D 11 into a swap chain on `window`, an `HWND`; with the adapter's
    /// description.
    pub fn direct3d11(window: *mut c_void) -> Result<(Self, String), String> {
        d3d11::Gpu::new(window)
            .map(|(gpu, adapter)| (Self::with_gpu(Gpu::D3d11(gpu)), adapter))
            .map_err(|error| error.to_string())
    }
}

/// `$gpu` as whichever backend it holds, named `$backend` in `$body`.
macro_rules! each {
    ($gpu:expr, $backend:ident => $body:expr) => {
        match $gpu {
            Gpu::Wgpu($backend) => $body,
            Gpu::D3d11($backend) => $body,
            Gpu::Gl($backend) => $body,
        }
    };
}

impl Gpu {
    pub(super) fn target(&self, size: [u32; 2]) -> Result<Target, String> {
        match self {
            Self::D3d11(gpu) => gpu
                .target(size)
                .map(Target::D3d11)
                .map_err(|error| error.to_string()),
            Self::Gl(gpu) => gpu.target(size).map(Target::Gl),
            Self::Wgpu(_) => unreachable!("wgpu's targets are its surface's"),
        }
    }

    pub(super) fn present(&self, target: &Target, translucent: bool) {
        match (self, target) {
            // Direct3D 11 premultiplies every frame again, which leaves opaque ones be.
            (Self::D3d11(gpu), Target::D3d11(target)) => {
                if let Err(error) = gpu.present(target) {
                    eprintln!("Presenting failed: {error}");
                }
            }
            (Self::Gl(gpu), Target::Gl(target)) => gpu.present(target, translucent),
            _ => unreachable!("Frames are drawn into the renderer's backend's targets"),
        }
    }

    pub(super) fn read_pixels(&self, target: &Target) -> Result<Vec<u8>, String> {
        match (self, target) {
            (Self::D3d11(gpu), Target::D3d11(target)) => {
                gpu.read_pixels(target).map_err(|error| error.to_string())
            }
            (Self::Gl(gpu), Target::Gl(target)) => gpu.read_pixels(target),
            _ => unreachable!("Snapshots come from the renderer's backend's targets"),
        }
    }

    pub(super) fn flipped(&self) -> bool {
        each!(self, gpu => gpu.flipped())
    }

    pub(super) fn max_texture_dimension(&self) -> u32 {
        each!(self, gpu => gpu.max_texture_dimension())
    }

    pub(super) fn atlas_side(&self) -> u32 {
        each!(self, gpu => gpu.atlas_side())
    }

    pub(super) fn new_atlas(&mut self, side: u32) {
        each!(self, gpu => gpu.new_atlas(side))
    }

    pub(super) fn write_atlas(&self, origin: [u32; 2], size: [u32; 2], rgba: &[u8]) {
        each!(self, gpu => gpu.write_atlas(origin, size, rgba))
    }

    pub(super) fn upload_image(&self, image: &RasterImage) -> Image {
        match self {
            Self::Wgpu(gpu) => Image::Wgpu(gpu.upload_image(image)),
            Self::D3d11(gpu) => Image::D3d11(gpu.upload_image(image)),
            Self::Gl(gpu) => Image::Gl(gpu.upload_image(image)),
        }
    }

    pub(super) fn submit(
        &mut self,
        frame: &Frame<'_>,
        target: &Target,
        size: [u32; 2],
        clear: [f32; 4],
    ) {
        match (self, target) {
            (Self::Wgpu(gpu), Target::Wgpu(view)) => gpu.submit(frame, view, size, clear),
            (Self::D3d11(gpu), Target::D3d11(target)) => gpu.submit(frame, target, size, clear),
            (Self::Gl(gpu), Target::Gl(target)) => gpu.submit(frame, target, size, clear),
            _ => unreachable!("Frames are drawn into the renderer's backend's targets"),
        }
    }
}
