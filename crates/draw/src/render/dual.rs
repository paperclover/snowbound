//! Every backend a platform offers in one build, its maker picking one: wgpu everywhere,
//! with Direct3D 11 and OpenGL 2.1 on Windows, where Windows 7 has no Direct3D 12, OpenGL
//! on macOS, and the 2D canvas in a browser with neither WebGPU nor WebGL2.
#[cfg(target_arch = "wasm32")]
use super::canvas2d;
#[cfg(windows)]
use super::d3d11;
#[cfg(any(windows, target_os = "macos"))]
use super::gl;
use super::{webgpu, *};

/// What a frame is drawn into, for the backend the renderer draws with.
pub enum Target {
    Wgpu(wgpu::TextureView),
    #[cfg(windows)]
    D3d11(d3d11::Target),
    #[cfg(any(windows, target_os = "macos"))]
    Gl(gl::Target),
    #[cfg(target_arch = "wasm32")]
    Canvas(canvas2d::Target),
}

impl From<wgpu::TextureView> for Target {
    fn from(view: wgpu::TextureView) -> Self {
        Self::Wgpu(view)
    }
}

#[cfg(target_arch = "wasm32")]
impl Target {
    /// Frames drawn into `canvas` with its 2D context.
    pub fn canvas2d(canvas: web_sys::HtmlCanvasElement) -> Result<Self, String> {
        canvas2d::Target::new(canvas).map(Self::Canvas)
    }
}

impl Target {
    pub fn size(&self) -> [u32; 2] {
        match self {
            Self::Wgpu(view) => [view.texture().width(), view.texture().height()],
            #[cfg(windows)]
            Self::D3d11(target) => target.size(),
            #[cfg(any(windows, target_os = "macos"))]
            Self::Gl(target) => target.size(),
            #[cfg(target_arch = "wasm32")]
            Self::Canvas(target) => target.size(),
        }
    }
}

pub(super) enum Image {
    Wgpu(webgpu::Image),
    #[cfg(windows)]
    D3d11(d3d11::Image),
    #[cfg(any(windows, target_os = "macos"))]
    Gl(gl::Image),
    #[cfg(target_arch = "wasm32")]
    Canvas(canvas2d::Image),
}

/// `AsRef` from `Image` to its variant `$variant`, which holds `$image`.
macro_rules! image {
    ($variant:ident, $image:ty) => {
        impl AsRef<$image> for Image {
            fn as_ref(&self) -> &$image {
                match self {
                    Self::$variant(image) => image,
                    #[allow(unreachable_patterns)]
                    _ => unreachable!("Images belong to the renderer's backend"),
                }
            }
        }
    };
}

image!(Wgpu, webgpu::Image);
#[cfg(windows)]
image!(D3d11, d3d11::Image);
#[cfg(any(windows, target_os = "macos"))]
image!(Gl, gl::Image);
#[cfg(target_arch = "wasm32")]
image!(Canvas, canvas2d::Image);

// A renderer holds one, so its size goes unnoticed.
#[allow(clippy::large_enum_variant)]
pub(super) enum Gpu {
    Wgpu(webgpu::Gpu),
    #[cfg(windows)]
    D3d11(d3d11::Gpu),
    #[cfg(any(windows, target_os = "macos"))]
    Gl(gl::Gpu),
    #[cfg(target_arch = "wasm32")]
    Canvas(canvas2d::Gpu),
}

impl From<webgpu::Gpu> for Gpu {
    fn from(gpu: webgpu::Gpu) -> Self {
        Self::Wgpu(gpu)
    }
}

#[cfg(any(windows, target_os = "macos"))]
impl From<gl::Gpu> for Gpu {
    fn from(gpu: gl::Gpu) -> Self {
        Self::Gl(gpu)
    }
}

#[cfg(target_arch = "wasm32")]
impl From<canvas2d::Gpu> for Gpu {
    fn from(gpu: canvas2d::Gpu) -> Self {
        Self::Canvas(gpu)
    }
}

#[cfg(not(windows))]
impl Renderer {
    /// The device a renderer `Renderer::new` made draws with.
    pub fn device(&self) -> &wgpu::Device {
        self.wgpu().device()
    }

    pub fn queue(&self) -> &wgpu::Queue {
        self.wgpu().queue()
    }

    fn wgpu(&self) -> &webgpu::Gpu {
        match &self.gpu {
            Gpu::Wgpu(gpu) => gpu,
            _ => unreachable!("Only `Renderer::new` makes a renderer drawing with wgpu"),
        }
    }
}

#[cfg(windows)]
impl Renderer {
    /// Draws with Direct3D 11 into a swap chain on `window`, an `HWND`; with the adapter's
    /// description.
    pub fn direct3d11(window: *mut std::ffi::c_void) -> Result<(Self, String), String> {
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
            #[cfg(windows)]
            Gpu::D3d11($backend) => $body,
            #[cfg(any(windows, target_os = "macos"))]
            Gpu::Gl($backend) => $body,
            #[cfg(target_arch = "wasm32")]
            Gpu::Canvas($backend) => $body,
        }
    };
}

impl Gpu {
    #[cfg(any(windows, target_os = "macos"))]
    pub(super) fn target(&self, size: [u32; 2]) -> Result<Target, String> {
        match self {
            #[cfg(windows)]
            Self::D3d11(gpu) => gpu
                .target(size)
                .map(Target::D3d11)
                .map_err(|error| error.to_string()),
            Self::Gl(gpu) => gpu.target(size).map(Target::Gl),
            Self::Wgpu(_) => unreachable!("wgpu's targets are its surface's"),
        }
    }

    #[cfg(any(windows, target_os = "macos"))]
    pub(super) fn present(&self, target: &Target, translucent: bool) {
        match (self, target) {
            // Direct3D 11 premultiplies every frame again, which leaves opaque ones be.
            #[cfg(windows)]
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
            #[cfg(windows)]
            (Self::D3d11(gpu), Target::D3d11(target)) => {
                gpu.read_pixels(target).map_err(|error| error.to_string())
            }
            #[cfg(any(windows, target_os = "macos"))]
            (Self::Gl(gpu), Target::Gl(target)) => gpu.read_pixels(target),
            #[cfg(target_arch = "wasm32")]
            (Self::Canvas(gpu), Target::Canvas(target)) => gpu.read_pixels(target),
            _ => unreachable!("Snapshots come from the renderer's backend's targets"),
        }
    }

    pub(super) fn blends_linear(&self) -> bool {
        each!(self, gpu => gpu.blends_linear())
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
            #[cfg(windows)]
            Self::D3d11(gpu) => Image::D3d11(gpu.upload_image(image)),
            #[cfg(any(windows, target_os = "macos"))]
            Self::Gl(gpu) => Image::Gl(gpu.upload_image(image)),
            #[cfg(target_arch = "wasm32")]
            Self::Canvas(gpu) => Image::Canvas(gpu.upload_image(image)),
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
            #[cfg(windows)]
            (Self::D3d11(gpu), Target::D3d11(target)) => gpu.submit(frame, target, size, clear),
            #[cfg(any(windows, target_os = "macos"))]
            (Self::Gl(gpu), Target::Gl(target)) => gpu.submit(frame, target, size, clear),
            #[cfg(target_arch = "wasm32")]
            (Self::Canvas(gpu), Target::Canvas(target)) => gpu.submit(frame, target, size, clear),
            #[allow(unreachable_patterns)]
            _ => unreachable!("Frames are drawn into the renderer's backend's targets"),
        }
    }
}
