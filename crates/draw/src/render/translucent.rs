/// Carries frames to a window whose compositor blends premultiplied sRGB, as Core Animation's
/// transparent layers do. The renderer blends in linear light into a texture of its own; on
/// the way to the window each pixel is premultiplied again in sRGB, so a translucent colour
/// over whatever shows through the window looks as it would over an opaque one.
pub struct Translucent {
    frame: Option<wgpu::Texture>,
    format: wgpu::TextureFormat,
    pipeline: wgpu::RenderPipeline,
}

impl Translucent {
    /// Frames the renderer draws in `format` for a window view in `window`, which stores its
    /// bytes as given.
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        window: wgpu::TextureFormat,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::include_wgsl!("translucent.wgsl"));
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Translucent window"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vertex"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fragment"),
                compilation_options: Default::default(),
                targets: &[Some(window.into())],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        Self {
            frame: None,
            format,
            pipeline,
        }
    }

    /// The view to draw a frame `size` pixels large into.
    pub fn target(&mut self, device: &wgpu::Device, size: [u32; 2]) -> wgpu::TextureView {
        let extent = wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        };
        let frame = match &self.frame {
            Some(frame) if frame.size() == extent => frame,
            _ => self
                .frame
                .insert(device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("Translucent frame"),
                    size: extent,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: self.format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })),
        };
        frame.create_view(&Default::default())
    }

    /// Copies the frame last drawn into `target` to `window`.
    pub fn present(&self, device: &wgpu::Device, queue: &wgpu::Queue, window: &wgpu::TextureView) {
        let Some(frame) = &self.frame else {
            return;
        };
        let binding = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Translucent frame"),
            layout: &self.pipeline.get_bind_group_layout(0),
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(
                    &frame.create_view(&Default::default()),
                ),
            }],
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Translucent window"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: window,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &binding, &[]);
            pass.draw(0..3, 0..1);
        }
        queue.submit([encoder.finish()]);
    }
}
