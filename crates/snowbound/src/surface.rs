//! Frames reach the window through wgpu.

use crate::{Window, platform, settings::Backend, trace_input};
use draw::Renderer;
use std::{error::Error, sync::Arc};

pub struct Surface {
    /// Device pixels frames and snapshots are drawn at; `configure` gives the window it.
    pub size: [u32; 2],
    window: Arc<Window>,
    /// The canvas a browser's frames show in.
    #[cfg(target_arch = "wasm32")]
    canvas: web_sys::HtmlCanvasElement,
    device: wgpu::Device,
    queue: wgpu::Queue,
    instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    /// The sRGB view of the surface's format, which the renderer blends in, where the surface
    /// itself is not sRGB, as a browser's canvas is not.
    view_format: wgpu::TextureFormat,
    /// Carries frames to the window where the system's backdrop shows through the strip.
    translucent: Option<draw::Translucent>,
}

pub struct Frame {
    pub target: draw::Target,
    pub(super) texture: wgpu::SurfaceTexture,
    pub(super) reconfigure: bool,
}

pub struct Offscreen {
    pub target: draw::Target,
    pub(super) texture: wgpu::Texture,
}

impl Surface {
    /// Draws with `backend` through wgpu; with `backdrop`, frames leave the system's
    /// material showing through their transparent pixels. Answers the adapter's name too.
    pub async fn new(
        window: Arc<Window>,
        backdrop: bool,
        backend: Backend,
        #[cfg(target_arch = "wasm32")] canvas: web_sys::HtmlCanvasElement,
    ) -> Result<(Self, Renderer, String), Box<dyn Error>> {
        #[cfg(target_arch = "wasm32")]
        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
        #[cfg(not(target_arch = "wasm32"))]
        let mut descriptor =
            wgpu::InstanceDescriptor::new_with_display_handle_from_env(Box::new(window.clone()));
        descriptor.backends = match backend {
            Backend::Webgpu => wgpu::Backends::BROWSER_WEBGPU,
            Backend::Webgl2 | Backend::Gl => wgpu::Backends::GL,
            Backend::Metal => wgpu::Backends::METAL,
            Backend::Vulkan => wgpu::Backends::VULKAN,
            Backend::D3d12 => wgpu::Backends::DX12,
            _ => return Err(format!("wgpu has no {}", backend.label()).into()),
        };
        // wgpu's OpenGL panics drawing to a Wayland window without libwayland-egl.
        #[cfg(target_os = "linux")]
        {
            use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
            if backend == Backend::Gl
                && matches!(
                    window.window_handle()?.as_raw(),
                    RawWindowHandle::Wayland(_)
                )
                && !crate::loader::loads(c"libwayland-egl.so.1")
            {
                return Err("Needs libwayland-egl".into());
            }
        }
        // Through DirectComposition where the backdrop shows through.
        #[cfg(windows)]
        if backdrop {
            descriptor.backend_options.dx12.presentation_system =
                wgpu::Dx12SwapchainKind::DxgiFromVisual;
        }
        #[cfg(target_arch = "wasm32")]
        let instance = wgpu::util::new_instance_with_webgpu_detection(descriptor).await;
        #[cfg(not(target_arch = "wasm32"))]
        let instance = wgpu::Instance::new(descriptor);
        #[cfg(target_arch = "wasm32")]
        let target = wgpu::SurfaceTarget::Canvas(canvas.clone());
        #[cfg(not(target_arch = "wasm32"))]
        let target: wgpu::SurfaceTarget<'static> = window.clone().into();
        let surface = instance.create_surface(target)?;
        platform::configure_presentation(&surface);
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                compatible_surface: Some(&surface),
                ..Default::default()
            })
            .await?;
        // A browser without WebGPU draws through WebGL2, whose limits are lower.
        #[cfg(target_arch = "wasm32")]
        let required_limits =
            wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits());
        #[cfg(not(target_arch = "wasm32"))]
        let required_limits = wgpu::Limits::default();
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                required_limits,
                ..Default::default()
            })
            .await?;
        let size = window.inner_size();
        let mut config = surface
            .get_default_config(&adapter, size.width, size.height)
            .ok_or("No supported canvas surface")?;
        let alpha = surface.get_capabilities(&adapter).alpha_modes;
        let translucent = backdrop.then(|| {
            // DirectComposition's premultiplied swap chains and Core Animation's non-opaque
            // layers both composite colour premultiplied in sRGB, as `Translucent` leaves
            // it; wgpu calls the second, the only one Metal offers, post-multiplied.
            config.alpha_mode = if alpha.contains(&wgpu::CompositeAlphaMode::PreMultiplied) {
                wgpu::CompositeAlphaMode::PreMultiplied
            } else {
                wgpu::CompositeAlphaMode::PostMultiplied
            };
            let window_format = config.format.remove_srgb_suffix();
            config.view_formats.push(window_format);
            draw::Translucent::new(&device, config.format, window_format)
        });
        if platform::cuts_corners() && alpha.contains(&wgpu::CompositeAlphaMode::PreMultiplied) {
            config.alpha_mode = wgpu::CompositeAlphaMode::PreMultiplied;
        }
        #[cfg(target_arch = "wasm32")]
        let view_format = config.format.add_srgb_suffix();
        #[cfg(not(target_arch = "wasm32"))]
        let view_format = config.format;
        if view_format != config.format {
            config.view_formats.push(view_format);
        }
        surface.configure(&device, &config);
        let info = adapter.get_info();
        eprintln!("Canvas GPU: {info:?}");
        let renderer = Renderer::new(device.clone(), queue.clone(), view_format);
        Ok((
            Self {
                size: [config.width, config.height],
                window,
                #[cfg(target_arch = "wasm32")]
                canvas,
                device,
                queue,
                instance,
                surface,
                config,
                view_format,
                translucent,
            },
            renderer,
            info.name,
        ))
    }

    /// Whether frames leave the system's backdrop showing.
    pub fn translucent(&self) -> bool {
        self.translucent.is_some()
    }

    pub fn configure(&mut self, _: &Renderer) {
        [self.config.width, self.config.height] = self.size;
        self.surface.configure(&self.device, &self.config);
    }

    /// The window's next frame, or none to skip this one.
    pub fn frame(&mut self, renderer: &Renderer) -> Result<Option<Frame>, Box<dyn Error>> {
        if self.size.contains(&0) {
            return Ok(None);
        }
        let (texture, reconfigure) = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(texture) => (texture, false),
            wgpu::CurrentSurfaceTexture::Suboptimal(texture) => (texture, true),
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.configure(renderer);
                self.window.request_redraw();
                return Ok(None);
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                self.surface = self.instance.create_surface(self.target())?;
                platform::configure_presentation(&self.surface);
                self.configure(renderer);
                self.window.request_redraw();
                return Ok(None);
            }
            wgpu::CurrentSurfaceTexture::Timeout => {
                trace_input(&"Surface timeout");
                return Ok(None);
            }
            wgpu::CurrentSurfaceTexture::Occluded => {
                trace_input(&"Surface occluded");
                return Ok(None);
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                return Err("Canvas surface validation failed".into());
            }
        };
        let target = match &mut self.translucent {
            Some(translucent) => translucent.target(&self.device, self.size),
            None => texture.texture.create_view(&wgpu::TextureViewDescriptor {
                format: Some(self.view_format),
                ..Default::default()
            }),
        };
        let target = target_of(target);
        Ok(Some(Frame {
            target,
            texture,
            reconfigure,
        }))
    }

    pub fn present(&mut self, renderer: &Renderer, frame: Frame) {
        if let Some(translucent) = &self.translucent {
            let window = frame
                .texture
                .texture
                .create_view(&wgpu::TextureViewDescriptor {
                    format: Some(self.config.format.remove_srgb_suffix()),
                    ..Default::default()
                });
            translucent.present(&self.device, &self.queue, &window);
        }
        self.window.pre_present_notify();
        self.queue.present(frame.texture);
        platform::commit_presentation(&self.window);
        if frame.reconfigure {
            self.configure(renderer);
        }
    }

    /// A target the size of the window's frames that `read` reads back.
    pub fn offscreen(&self, _: &Renderer) -> Result<Offscreen, Box<dyn Error>> {
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Snapshot"),
            size: wgpu::Extent3d {
                width: self.size[0],
                height: self.size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.view_format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        Ok(Offscreen {
            target: target_of(texture.create_view(&Default::default())),
            texture,
        })
    }

    /// What the surface presents to: the window, or in the browser its canvas.
    fn target(&self) -> wgpu::SurfaceTarget<'static> {
        #[cfg(target_arch = "wasm32")]
        return wgpu::SurfaceTarget::Canvas(self.canvas.clone());
        #[cfg(not(target_arch = "wasm32"))]
        self.window.clone().into()
    }

    /// The offscreen target's sRGB RGBA rows, top first.
    pub fn read(&self, _: &Renderer, offscreen: Offscreen) -> Result<Vec<u8>, Box<dyn Error>> {
        let size = self.size;
        let row = (size[0] * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Snapshot readback"),
            size: u64::from(row) * u64::from(size[1]),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            offscreen.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(size[1]),
                },
            },
            offscreen.texture.size(),
        );
        self.queue.submit([encoder.finish()]);
        buffer.map_async(wgpu::MapMode::Read, .., |_| {});
        self.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(std::time::Duration::from_secs(5)),
        })?;
        let bgra = matches!(
            self.view_format,
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
        );
        let mut pixels = Vec::with_capacity(size[0] as usize * size[1] as usize * 4);
        for line in buffer.get_mapped_range(..)?.chunks_exact(row as usize) {
            pixels.extend_from_slice(&line[..size[0] as usize * 4]);
        }
        if bgra {
            for pixel in pixels.chunks_exact_mut(4) {
                pixel.swap(0, 2);
            }
        }
        Ok(pixels)
    }
}

/// Takes the layer out of the view, for whatever draws next.
#[cfg(target_os = "macos")]
impl Drop for Surface {
    fn drop(&mut self) {
        platform::release_presentation(&self.surface);
    }
}

/// `view` as what the renderer draws into, which is the view itself unless `draw` also
/// paints otherwise: through Direct3D 11 or OpenGL, or a browser's 2D canvas.
#[cfg_attr(
    not(any(windows, target_os = "macos", target_arch = "wasm32")),
    expect(clippy::useless_conversion)
)]
fn target_of(view: wgpu::TextureView) -> draw::Target {
    view.into()
}
