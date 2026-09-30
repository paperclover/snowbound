//! Submission through wgpu: Metal, Vulkan or GL, whichever the platform offers.
use super::*;

/// What a frame is drawn into.
pub type Target = wgpu::TextureView;

pub(super) type Image = wgpu::BindGroup;

/// Whether an offscreen picture's rows run bottom first.
pub(super) const FLIPPED: bool = false;

pub(super) struct Gpu {
    pipeline: wgpu::RenderPipeline,
    image_pipeline: wgpu::RenderPipeline,
    erase_pipeline: wgpu::RenderPipeline,
    multiply_pipeline: wgpu::RenderPipeline,
    atlas: wgpu::Texture,
    bind_group: wgpu::BindGroup,
    image_sampler: wgpu::Sampler,
    vertex_buffer: wgpu::Buffer,
    format: wgpu::TextureFormat,
    /// Offscreen pictures for groups, the target's size, and how each is sampled.
    groups: Vec<(wgpu::TextureView, wgpu::BindGroup)>,
    groups_size: [u32; 2],
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
                        attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4, 3 => Float32x2, 4 => Float32x4, 5 => Float32, 6 => Float32x2, 7 => Float32x3, 8 => Float32],
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
        // The fragment's colour is scaled by its coverage, so this leaves the target times
        // the colour where covered and the target elsewhere; alpha stays.
        let multiply_pipeline = make_pipeline(wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Dst,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        });
        let (atlas, bind_group) = atlas(&device, &pipeline, ATLAS_SIZE);
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
                multiply_pipeline,
                atlas,
                bind_group,
                image_sampler,
                vertex_buffer,
                format,
                groups: Vec::new(),
                groups_size: [0; 2],
            },
        )
    }

    /// The widest and tallest texture the device takes, in pixels.
    pub fn max_texture_dimension(&self) -> u32 {
        self.device.limits().max_texture_dimension_2d
    }

    pub(super) fn atlas_side(&self) -> u32 {
        self.gpu.atlas.width()
    }

    pub(super) fn new_atlas(&mut self, side: u32) {
        (self.gpu.atlas, self.gpu.bind_group) = atlas(&self.device, &self.gpu.pipeline, side);
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

    /// An offscreen picture `size` large for each group, kept for later frames.
    fn group_pictures(&mut self, size: [u32; 2]) {
        let gpu = &mut self.gpu;
        if gpu.groups_size != size {
            gpu.groups.clear();
            gpu.groups_size = size;
        }
        while gpu.groups.len() < self.groups.len() {
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("Draw group"),
                size: wgpu::Extent3d {
                    width: size[0],
                    height: size[1],
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: gpu.format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            let binding = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Draw group"),
                layout: &gpu.pipeline.get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&gpu.image_sampler),
                    },
                ],
            });
            gpu.groups.push((view, binding));
        }
    }

    /// Clears `target`, `size` device pixels, to linear `clear` and draws the prepared
    /// batches, each group's offscreen first.
    pub(super) fn submit(&mut self, target: &Target, size: [u32; 2], clear: [f32; 4]) {
        self.group_pictures(size);
        let gpu = &self.gpu;
        self.queue
            .write_buffer(&gpu.vertex_buffer, 0, bytemuck::cast_slice(&self.vertices));
        let mut encoder = self.device.create_command_encoder(&Default::default());
        let draw = |pass: &mut wgpu::RenderPass, batches: &[Batch]| {
            for batch in batches {
                let [x, y, width, height] = batch.scissor;
                pass.set_scissor_rect(x, y, width, height);
                let (pipeline, binding) = match batch.blend {
                    Blend::Over => (&gpu.pipeline, &gpu.bind_group),
                    Blend::Image(id) => (&gpu.image_pipeline, &self.images[&id].texture),
                    Blend::Erase => (&gpu.erase_pipeline, &gpu.bind_group),
                    Blend::Multiply => (&gpu.multiply_pipeline, &gpu.bind_group),
                };
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, binding, &[]);
                pass.draw(batch.vertices.clone(), 0..1);
            }
        };
        let begin = |encoder: &mut wgpu::CommandEncoder, view, clear: [f32; 4]| {
            let [r, g, b, a] = clear.map(f64::from);
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Draw"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
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
            pass.forget_lifetime()
        };
        for (group, (view, _)) in self.groups.iter().zip(&gpu.groups) {
            let mut pass = begin(&mut encoder, view, [0.0; 4]);
            draw(&mut pass, &self.batches[group.batches.clone()]);
        }
        let mut pass = begin(&mut encoder, target, clear);
        let mut next = 0;
        for (group, (_, binding)) in self.groups.iter().zip(&gpu.groups) {
            draw(&mut pass, &self.batches[next..group.batches.start]);
            pass.set_scissor_rect(0, 0, size[0], size[1]);
            pass.set_pipeline(&gpu.image_pipeline);
            pass.set_bind_group(0, binding, &[]);
            pass.draw(group.composite.clone(), 0..1);
            next = group.batches.end;
        }
        draw(&mut pass, &self.batches[next..]);
        drop(pass);
        self.queue.submit([encoder.finish()]);
    }
}

/// An empty atlas `side` texels square, and how the pipeline samples it.
fn atlas(
    device: &wgpu::Device,
    pipeline: &wgpu::RenderPipeline,
    side: u32,
) -> (wgpu::Texture, wgpu::BindGroup) {
    let atlas = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Glyph atlas"),
        size: wgpu::Extent3d {
            width: side,
            height: side,
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
    (atlas, bind_group)
}
