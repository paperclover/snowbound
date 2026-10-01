//! Frames reach the window through wgpu.

use crate::{Window, platform, trace_input};
use draw::Renderer;
use std::{error::Error, sync::Arc};

pub struct Surface {
    /// Device pixels frames and snapshots are drawn at; `configure` gives the window it.
    pub size: [u32; 2],
    window: Arc<Window>,
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
    /// With `backdrop`, frames leave the system's material showing through their
    /// transparent pixels.
    pub async fn new(
        window: Arc<Window>,
        backdrop: bool,
    ) -> Result<(Self, Renderer), Box<dyn Error>> {
        #[cfg(target_arch = "wasm32")]
        let instance = {
            let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
            descriptor.backends = wgpu::Backends::BROWSER_WEBGPU | wgpu::Backends::GL;
            wgpu::util::new_instance_with_webgpu_detection(descriptor).await
        };
        #[cfg(not(target_arch = "wasm32"))]
        #[cfg_attr(not(any(windows, target_os = "linux")), expect(unused_mut))]
        let mut descriptor =
            wgpu::InstanceDescriptor::new_with_display_handle_from_env(Box::new(window.clone()));
        // wgpu's OpenGL panics drawing to a Wayland window without libwayland-egl.
        #[cfg(target_os = "linux")]
        {
            use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
            if matches!(
                window.window_handle()?.as_raw(),
                RawWindowHandle::Wayland(_)
            ) && !crate::loader::loads(c"libwayland-egl.so.1")
            {
                descriptor.backends.remove(wgpu::Backends::GL);
            }
        }
        // Direct3D 12, which Windows 10 and 11 always have, and through DirectComposition
        // where the backdrop shows through; elsewhere `surface_windows` draws with OpenGL.
        #[cfg(windows)]
        {
            descriptor.backends = wgpu::Backends::DX12;
            if backdrop {
                descriptor.backend_options.dx12.presentation_system =
                    wgpu::Dx12SwapchainKind::DxgiFromVisual;
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        let instance = wgpu::Instance::new(descriptor);
        let surface = instance.create_surface(target(&window))?;
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
        eprintln!("Canvas GPU: {:?}", adapter.get_info());
        let renderer = Renderer::new(device.clone(), queue.clone(), view_format);
        Ok((
            Self {
                size: [config.width, config.height],
                window,
                device,
                queue,
                instance,
                surface,
                config,
                view_format,
                translucent,
            },
            renderer,
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
                self.surface = self.instance.create_surface(target(&self.window))?;
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

/// What the surface presents to: the window, or in the browser the page's canvas.
fn target(window: &Arc<Window>) -> wgpu::SurfaceTarget<'static> {
    #[cfg(target_arch = "wasm32")]
    return {
        let _ = window;
        wgpu::SurfaceTarget::Canvas(platform::canvas())
    };
    #[cfg(not(target_arch = "wasm32"))]
    window.clone().into()
}

/// `view` as what the renderer draws into, which is the view itself unless `draw` also
/// paints through OpenGL, as on Windows.
#[cfg_attr(not(windows), expect(clippy::useless_conversion))]
fn target_of(view: wgpu::TextureView) -> draw::Target {
    view.into()
}
