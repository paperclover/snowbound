//! Submission through wgpu: Metal, Vulkan or GL, whichever the platform offers.
use super::*;

/// What a frame is drawn into.
#[cfg(not(windows))]
pub type Target = wgpu::TextureView;

/// A picture's texture, as its batches bind it.
pub(super) struct Image(wgpu::BindGroup);

impl AsRef<Image> for Image {
    fn as_ref(&self) -> &Image {
        self
    }
}

pub(super) struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    layout: wgpu::BindGroupLayout,
    /// A pipeline for each of `Blend::STATES`.
    pipelines: [wgpu::RenderPipeline; 4],
    atlas: (wgpu::Texture, wgpu::BindGroup),
    nearest: wgpu::Sampler,
    linear: wgpu::Sampler,
    vertex_buffer: wgpu::Buffer,
    format: wgpu::TextureFormat,
    /// Offscreen pictures for groups, the target's size, and how each is sampled.
    groups: Vec<(wgpu::Texture, wgpu::BindGroup)>,
}

impl Renderer {
    pub fn new(device: wgpu::Device, queue: wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        Self::with_gpu(Gpu::new(device, queue, format))
    }

    #[cfg(not(windows))]
    pub fn device(&self) -> &wgpu::Device {
        &self.gpu.device
    }

    #[cfg(not(windows))]
    pub fn queue(&self) -> &wgpu::Queue {
        &self.gpu.queue
    }
}

impl Gpu {
    pub(super) fn new(
        device: wgpu::Device,
        queue: wgpu::Queue,
        format: wgpu::TextureFormat,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::include_wgsl!("../draw.wgsl"));
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
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
            bind_group_layouts: &[Some(&layout)],
            ..Default::default()
        });
        let attributes: [_; 9] = std::array::from_fn(|index| {
            let (_, floats, offset) = ATTRIBUTES[index];
            wgpu::VertexAttribute {
                format: [
                    wgpu::VertexFormat::Float32,
                    wgpu::VertexFormat::Float32x2,
                    wgpu::VertexFormat::Float32x3,
                    wgpu::VertexFormat::Float32x4,
                ][floats - 1],
                offset: offset as u64,
                shader_location: index as u32,
            }
        });
        let pipelines = Blend::STATES.map(|[color, dst_color, alpha, dst_alpha]| {
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
                        attributes: &attributes,
                    })],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fragment"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(wgpu::BlendState {
                            color: component([color, dst_color]),
                            alpha: component([alpha, dst_alpha]),
                        }),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        });
        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Draw vertices"),
            size: VERTEX_BUFFER_BYTES,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sampler = |filter| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                mag_filter: filter,
                min_filter: filter,
                ..Default::default()
            })
        };
        let nearest = sampler(wgpu::FilterMode::Nearest);
        let atlas = atlas(&device, &layout, &nearest, ATLAS_SIZE);
        Self {
            linear: sampler(wgpu::FilterMode::Linear),
            nearest,
            atlas,
            device,
            queue,
            layout,
            pipelines,
            vertex_buffer,
            format,
            groups: Vec::new(),
        }
    }

    pub(super) fn flipped(&self) -> bool {
        false
    }

    pub(super) fn max_texture_dimension(&self) -> u32 {
        self.device.limits().max_texture_dimension_2d
    }

    pub(super) fn atlas_side(&self) -> u32 {
        self.atlas.0.width()
    }

    pub(super) fn new_atlas(&mut self, side: u32) {
        self.atlas = atlas(&self.device, &self.layout, &self.nearest, side);
    }

    pub(super) fn write_atlas(&self, origin: [u32; 2], size: [u32; 2], rgba: &[u8]) {
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                origin: wgpu::Origin3d {
                    x: origin[0],
                    y: origin[1],
                    z: 0,
                },
                ..self.atlas.0.as_image_copy()
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
        let texture = texture(
            &self.device,
            image.size,
            wgpu::TextureFormat::Rgba8UnormSrgb,
            wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        );
        self.queue.write_texture(
            texture.as_image_copy(),
            image.pixels(),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(image.size[0] * 4),
                rows_per_image: Some(image.size[1]),
            },
            texture.size(),
        );
        Image(binding(&self.device, &self.layout, &texture, &self.linear))
    }

    /// Clears `target`, `size` device pixels, to linear `clear` and draws the prepared
    /// batches, each group's offscreen first.
    pub(super) fn submit(
        &mut self,
        frame: &Frame<'_>,
        target: &wgpu::TextureView,
        size: [u32; 2],
        clear: [f32; 4],
    ) {
        if self
            .groups
            .first()
            .is_some_and(|(picture, _)| [picture.width(), picture.height()] != size)
        {
            self.groups.clear();
        }
        while self.groups.len() < frame.groups.len() {
            let picture = texture(
                &self.device,
                size,
                self.format,
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            );
            let binding = binding(&self.device, &self.layout, &picture, &self.linear);
            self.groups.push((picture, binding));
        }
        let gpu = &*self;
        gpu.queue
            .write_buffer(&gpu.vertex_buffer, 0, bytemuck::cast_slice(frame.vertices));
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        let mut paint = |view: &wgpu::TextureView, clear: [f32; 4], batches: &[Batch]| {
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
            for batch in batches {
                let [x, y, width, height] = batch.scissor;
                pass.set_scissor_rect(x, y, width, height);
                pass.set_pipeline(&gpu.pipelines[batch.blend.state()]);
                let binding = match batch.blend {
                    Blend::Image(id) => &frame.image::<Image>(id).0,
                    Blend::Group(index) => &gpu.groups[index].1,
                    Blend::Over | Blend::Erase | Blend::Multiply => &gpu.atlas.1,
                };
                pass.set_bind_group(0, binding, &[]);
                pass.draw(batch.vertices.clone(), 0..1);
            }
        };
        for (batches, (picture, _)) in frame.groups.iter().zip(&gpu.groups) {
            paint(&picture.create_view(&Default::default()), [0.0; 4], batches);
        }
        paint(target, clear, frame.batches);
        gpu.queue.submit([encoder.finish()]);
    }
}

fn component([source, destination]: [Factor; 2]) -> wgpu::BlendComponent {
    let factor = |factor| match factor {
        Factor::Zero => wgpu::BlendFactor::Zero,
        Factor::One => wgpu::BlendFactor::One,
        Factor::SourceAlpha => wgpu::BlendFactor::SrcAlpha,
        Factor::OneMinusSourceAlpha => wgpu::BlendFactor::OneMinusSrcAlpha,
        Factor::Destination => wgpu::BlendFactor::Dst,
    };
    wgpu::BlendComponent {
        src_factor: factor(source),
        dst_factor: factor(destination),
        operation: wgpu::BlendOperation::Add,
    }
}

fn texture(
    device: &wgpu::Device,
    [width, height]: [u32; 2],
    format: wgpu::TextureFormat,
    usage: wgpu::TextureUsages,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Draw"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    })
}

/// How the pipelines sample `texture` through `sampler`.
fn binding(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    texture: &wgpu::Texture,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Draw"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(
                    &texture.create_view(&Default::default()),
                ),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}

/// An empty atlas `side` texels square, and how the pipelines sample it.
fn atlas(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    nearest: &wgpu::Sampler,
    side: u32,
) -> (wgpu::Texture, wgpu::BindGroup) {
    let atlas = texture(
        device,
        [side; 2],
        wgpu::TextureFormat::Rgba8UnormSrgb,
        wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
    );
    let binding = binding(device, layout, &atlas, nearest);
    (atlas, binding)
}
