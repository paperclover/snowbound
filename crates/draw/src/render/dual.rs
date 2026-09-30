//! Both backends in one build, for Windows: wgpu where Direct3D 12 answers, and OpenGL 2.1
//! where it doesn't, as on Windows 7. Whoever makes the renderer picks one.
use super::{gl, webgpu, *};

/// What a frame is drawn into, for the backend the renderer draws with.
pub enum Target {
    Wgpu(wgpu::TextureView),
    Gl(gl::Target),
}

impl From<wgpu::TextureView> for Target {
    fn from(view: wgpu::TextureView) -> Self {
        Self::Wgpu(view)
    }
}

impl From<gl::Target> for Target {
    fn from(target: gl::Target) -> Self {
        Self::Gl(target)
    }
}

pub(super) enum Image {
    Wgpu(webgpu::Image),
    Gl(gl::Image),
}

impl AsRef<webgpu::Image> for Image {
    fn as_ref(&self) -> &webgpu::Image {
        match self {
            Self::Wgpu(image) => image,
            Self::Gl(_) => unreachable!("Images belong to the renderer's backend"),
        }
    }
}

impl AsRef<gl::Image> for Image {
    fn as_ref(&self) -> &gl::Image {
        match self {
            Self::Gl(image) => image,
            Self::Wgpu(_) => unreachable!("Images belong to the renderer's backend"),
        }
    }
}

pub(super) enum Gpu {
    Wgpu(webgpu::Gpu),
    Gl(gl::Gpu),
}

impl Renderer {
    pub fn new(device: wgpu::Device, queue: wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        Self::with_gpu(Gpu::Wgpu(webgpu::Gpu::new(device, queue, format)))
    }

    /// Draws with the OpenGL context current on this thread, which must stay current
    /// whenever the renderer or a `Target` is used.
    pub fn opengl() -> Result<Self, String> {
        gl::Gpu::new().map(|gpu| Self::with_gpu(Gpu::Gl(gpu)))
    }

    /// Copies `target` to the OpenGL context's window, as `gl`'s renderer does.
    pub fn present_translucent(&self, target: &gl::Target) {
        if let Gpu::Gl(gpu) = &self.gpu {
            gpu.present_translucent(target);
        }
    }
}

impl Gpu {
    pub(super) fn flipped(&self) -> bool {
        match self {
            Self::Wgpu(gpu) => gpu.flipped(),
            Self::Gl(gpu) => gpu.flipped(),
        }
    }

    pub(super) fn max_texture_dimension(&self) -> u32 {
        match self {
            Self::Wgpu(gpu) => gpu.max_texture_dimension(),
            Self::Gl(gpu) => gpu.max_texture_dimension(),
        }
    }

    pub(super) fn atlas_side(&self) -> u32 {
        match self {
            Self::Wgpu(gpu) => gpu.atlas_side(),
            Self::Gl(gpu) => gpu.atlas_side(),
        }
    }

    pub(super) fn new_atlas(&mut self, side: u32) {
        match self {
            Self::Wgpu(gpu) => gpu.new_atlas(side),
            Self::Gl(gpu) => gpu.new_atlas(side),
        }
    }

    pub(super) fn write_atlas(&self, origin: [u32; 2], size: [u32; 2], rgba: &[u8]) {
        match self {
            Self::Wgpu(gpu) => gpu.write_atlas(origin, size, rgba),
            Self::Gl(gpu) => gpu.write_atlas(origin, size, rgba),
        }
    }

    pub(super) fn upload_image(&self, image: &RasterImage) -> Image {
        match self {
            Self::Wgpu(gpu) => Image::Wgpu(gpu.upload_image(image)),
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
            (Self::Gl(gpu), Target::Gl(target)) => gpu.submit(frame, target, size, clear),
            _ => unreachable!("Frames are drawn into the renderer's backend's targets"),
        }
    }
}
