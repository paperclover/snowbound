//! Submission through wgpu: Metal, Vulkan or GL, whichever the platform offers.
use super::*;

/// What a frame is drawn into.
pub type Target = wgpu::TextureView;

pub(super) type Image = wgpu::BindGroup;

pub(super) struct Gpu {
    pipeline: wgpu::RenderPipeline,
    image_pipeline: wgpu::RenderPipeline,
    erase_pipeline: wgpu::RenderPipeline,
    atlas: wgpu::Texture,
    bind_group: wgpu::BindGroup,
    image_sampler: wgpu::Sampler,
    vertex_buffer: wgpu::Buffer,
}

impl Renderer {
    pub fn new(device: wgpu::Device, queue: wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::include_wgsl!("../draw.wgsl"));
        let binding_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Draw texture"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Draw"),
            bind_group_layouts: &[Some(&binding_layout)],
            ..Default::default()
        });
        let make_pipeline = |blend| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Draw"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vertex"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: size_of::<Vertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4, 3 => Float32x2, 4 => Float32x4, 5 => Float32],
                    })],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fragment"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(blend),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        // Coverage accumulates in alpha, leaving premultiplied colour over a transparent clear.
        let pipeline = make_pipeline(wgpu::BlendState {
            color: wgpu::BlendState::ALPHA_BLENDING.color,
            alpha: wgpu::BlendComponent::OVER,
        });
        let image_pipeline = make_pipeline(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING);
        let erase = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::Zero,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        };
        let erase_pipeline = make_pipeline(wgpu::BlendState {
            color: erase,
            alpha: erase,
        });
        let atlas = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Glyph atlas"),
            size: wgpu::Extent3d {
                width: ATLAS_SIZE,
                height: ATLAS_SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = atlas.create_view(&Default::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Glyph atlas"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Draw vertices"),
            size: VERTEX_BUFFER_BYTES,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let image_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Draw image filtering"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self::with_gpu(
            device,
            queue,
            Gpu {
                pipeline,
                image_pipeline,
                erase_pipeline,
                atlas,
                bind_group,
                image_sampler,
                vertex_buffer,
            },
        )
    }

    /// The widest and tallest texture the device takes, in pixels.
    pub fn max_texture_dimension(&self) -> u32 {
        self.device.limits().max_texture_dimension_2d
    }

    pub(super) fn write_atlas(&self, origin: [u32; 2], size: [u32; 2], rgba: &[u8]) {
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                origin: wgpu::Origin3d {
                    x: origin[0],
                    y: origin[1],
                    z: 0,
                },
                ..self.gpu.atlas.as_image_copy()
            },
            rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size[0] * 4),
                rows_per_image: Some(size[1]),
            },
            wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
        );
    }

    pub(super) fn upload_image(&self, image: &RasterImage) -> Image {
        let size = wgpu::Extent3d {
            width: image.size[0],
            height: image.size[1],
            depth_or_array_layers: 1,
        };
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Draw image"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            texture.as_image_copy(),
            image.pixels(),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(image.size[0] * 4),
                rows_per_image: Some(image.size[1]),
            },
            size,
        );
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Draw image"),
            layout: &self.gpu.pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(
                        &texture.create_view(&Default::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.gpu.image_sampler),
                },
            ],
        })
    }

    /// Clears `target` to linear `clear` and draws the prepared batches.
    pub(super) fn submit(&mut self, target: &Target, clear: [f32; 4]) {
        let gpu = &self.gpu;
        self.queue
            .write_buffer(&gpu.vertex_buffer, 0, bytemuck::cast_slice(&self.vertices));
        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let [r, g, b, a] = clear.map(f64::from);
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Draw"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r, g, b, a }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_vertex_buffer(0, gpu.vertex_buffer.slice(..));
            for batch in &self.batches {
                let [x, y, width, height] = batch.scissor;
                pass.set_scissor_rect(x, y, width, height);
                let (pipeline, binding) = match batch.blend {
                    Blend::Over => (&gpu.pipeline, &gpu.bind_group),
                    Blend::Image(id) => (&gpu.image_pipeline, &self.images[&id].texture),
                    Blend::Erase => (&gpu.erase_pipeline, &gpu.bind_group),
                };
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, binding, &[]);
                pass.draw(batch.vertices.clone(), 0..1);
            }
        }
        self.queue.submit([encoder.finish()]);
    }
}
