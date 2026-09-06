pub mod page;
#[cfg(test)]
mod profile;
mod tags;

use bytemuck::{Pod, Zeroable};
use image::ImageDecoder;
use one_canvas::{
    layout::TextLayout,
    outline::{ParagraphTag, TagIcon},
};
use parley::{PositionedLayoutItem, fontique::Blob};
use std::{
    collections::{HashMap, HashSet},
    ops::Range,
};
use swash::{
    FontRef,
    scale::{Render, ScaleContext, Source, StrikeWith, image::Content},
    zeno::{Angle, Format, Transform, Vector},
};

const ATLAS_SIZE: u32 = 2048;
const MAX_GLYPHS: usize = 8192;
const MAX_VERTICES: usize = 65_536;
const MAX_IMAGE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_IMAGES: usize = 256;

#[derive(Clone)]
pub struct RasterImage {
    size: [u32; 2],
    pixels: Blob<u8>,
}

impl RasterImage {
    pub fn decode(encoded: &[u8]) -> Result<Self, RenderError> {
        if encoded.len() as u64 > MAX_IMAGE_BYTES {
            return Err(RenderError::ImageBudget);
        }
        let format = image::guess_format(encoded).map_err(RenderError::ImageDecode)?;
        let mut reader = image::ImageReader::with_format(std::io::Cursor::new(encoded), format);
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(16_384);
        limits.max_image_height = Some(16_384);
        limits.max_alloc = Some(MAX_IMAGE_BYTES);
        reader.limits(limits.clone());
        let mut decoder = reader.into_decoder().map_err(RenderError::ImageDecode)?;
        let (width, height) = decoder.dimensions();
        if u64::from(width) * u64::from(height) * 4 > MAX_IMAGE_BYTES
            || decoder.total_bytes() > MAX_IMAGE_BYTES
        {
            return Err(RenderError::ImageBudget);
        }
        limits
            .reserve(decoder.total_bytes())
            .map_err(RenderError::ImageDecode)?;
        decoder
            .set_limits(limits)
            .map_err(RenderError::ImageDecode)?;
        let pixels = image::DynamicImage::from_decoder(decoder)
            .map_err(RenderError::ImageDecode)?
            .into_rgba8();
        Self::new([width, height], pixels.into_raw())
    }

    /// Immutable, straight-alpha sRGB RGBA pixels retain one cache identity across clones.
    pub fn new(size: [u32; 2], mut pixels: Vec<u8>) -> Result<Self, RenderError> {
        let bytes = u64::from(size[0])
            .checked_mul(u64::from(size[1]))
            .and_then(|v| v.checked_mul(4))
            .ok_or(RenderError::InvalidImage)?;
        if size.contains(&0) || bytes != pixels.len() as u64 {
            return Err(RenderError::InvalidImage);
        }
        if bytes > MAX_IMAGE_BYTES {
            return Err(RenderError::ImageBudget);
        }
        // Premultiply in linear light before filtering; transparent RGB must not bleed at edges.
        for pixel in pixels.chunks_exact_mut(4) {
            let alpha = pixel[3];
            if alpha == 255 {
                continue;
            }
            let linear = colorref(u32::from_le_bytes(pixel.try_into().unwrap()));
            for (byte, value) in pixel[..3].iter_mut().zip(linear) {
                let value = value * f32::from(alpha) / 255.0;
                *byte = srgb_byte(value);
            }
        }
        Ok(Self {
            size,
            pixels: pixels.into(),
        })
    }
}

struct CachedImage {
    binding: wgpu::BindGroup,
    bytes: u64,
}

struct Batch {
    vertices: Range<u32>,
    image: Option<u64>,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    position: [f32; 2],
    uv: [f32; 2],
    color: [f32; 4],
}

#[derive(Hash, PartialEq, Eq)]
enum AtlasKey {
    Text {
        font: u64,
        index: u32,
        glyph: u16,
        size: u32,
        coords: Vec<i16>,
        phase: [u8; 2],
        embolden: bool,
        skew: u32,
    },
    Tag {
        icon: TagIcon,
        size: u32,
        phase: [u8; 2],
    },
}

#[derive(Clone, Copy)]
struct AtlasGlyph {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    left: i32,
    top: i32,
    color: bool,
}

#[derive(Clone, Copy)]
pub struct Viewport {
    pub size: [u32; 2],
    /// Physical pixels per document point.
    pub scale: f32,
    /// Physical pixel position of the document origin.
    pub origin: [f32; 2],
}

impl Viewport {
    pub fn document_point(&self, point: [f32; 2]) -> [f32; 2] {
        [
            (point[0] - self.origin[0]) / self.scale,
            (point[1] - self.origin[1]) / self.scale,
        ]
    }

    fn visible_image_rect(&self, rect: [f32; 4]) -> Result<Option<[f32; 4]>, RenderError> {
        let rect = [
            rect[0] * self.scale + self.origin[0],
            rect[1] * self.scale + self.origin[1],
            rect[2] * self.scale + self.origin[0],
            rect[3] * self.scale + self.origin[1],
        ];
        if rect.iter().any(|v| !v.is_finite()) || rect[2] < rect[0] || rect[3] < rect[1] {
            return Err(RenderError::InvalidPrimitive);
        }
        Ok((rect[0] < self.size[0] as f32
            && rect[1] < self.size[1] as f32
            && rect[2] > 0.0
            && rect[3] > 0.0
            && rect[0] < rect[2]
            && rect[1] < rect[3])
            .then_some(rect))
    }
}

/// Coordinates are document points; colors are linear RGBA.
pub enum Primitive<'a> {
    Tag {
        tag: &'a ParagraphTag,
        origin: [f32; 2],
    },
    Text {
        layout: &'a TextLayout,
        origin: [f32; 2],
    },
    Rect {
        rect: [f32; 4],
        color: [f32; 4],
    },
    Image {
        image: &'a RasterImage,
        rect: [f32; 4],
    },
}

#[derive(Debug)]
pub enum RenderError {
    AtlasFull,
    FrameTooLarge,
    UnreadableFont,
    UnsupportedGlyph,
    InvalidViewport,
    InvalidPrimitive,
    InvalidImage,
    ImageBudget,
    ImageTooLarge,
    ImageDecode(image::ImageError),
}

pub struct Renderer {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    image_pipeline: wgpu::RenderPipeline,
    atlas: wgpu::Texture,
    bind_group: wgpu::BindGroup,
    image_sampler: wgpu::Sampler,
    vertex_buffer: wgpu::Buffer,
    vertices: Vec<Vertex>,
    glyphs: HashMap<AtlasKey, Option<AtlasGlyph>>,
    images: HashMap<u64, CachedImage>,
    batches: Vec<Batch>,
    scaler: ScaleContext,
    pen: [u32; 2],
    row_height: u32,
}

impl Renderer {
    pub fn new(device: wgpu::Device, queue: wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::include_wgsl!("canvas.wgsl"));
        let binding_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Canvas texture"),
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
            label: Some("Canvas"),
            bind_group_layouts: &[Some(&binding_layout)],
            ..Default::default()
        });
        let make_pipeline = |blend| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Canvas"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vertex"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: size_of::<Vertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4],
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
        let pipeline = make_pipeline(wgpu::BlendState::ALPHA_BLENDING);
        let image_pipeline = make_pipeline(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING);
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
            label: Some("Canvas vertices"),
            size: (MAX_VERTICES * size_of::<Vertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let image_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Canvas image filtering"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        queue.write_texture(
            atlas.as_image_copy(),
            &[255; 4],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4),
                rows_per_image: Some(1),
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        Self {
            device,
            queue,
            pipeline,
            image_pipeline,
            atlas,
            bind_group,
            image_sampler,
            vertex_buffer,
            vertices: Vec::new(),
            glyphs: HashMap::new(),
            images: HashMap::new(),
            batches: Vec::new(),
            scaler: ScaleContext::with_max_entries(32),
            pen: [1, 0],
            row_height: 1,
        }
    }

    pub fn clear_glyph_cache(&mut self) {
        self.glyphs.clear();
        self.pen = [1, 0];
        self.row_height = 1;
    }

    pub fn draw(
        &mut self,
        target: &wgpu::TextureView,
        viewport: Viewport,
        primitives: &[Primitive<'_>],
    ) -> Result<(), RenderError> {
        if viewport.size.contains(&0)
            || !viewport.scale.is_finite()
            || viewport.scale <= 0.0
            || viewport.origin.iter().any(|v| !v.is_finite())
        {
            return Err(RenderError::InvalidViewport);
        }
        let mut active_images = HashSet::new();
        let mut image_bytes = 0;
        for primitive in primitives {
            if let Primitive::Image { image, rect } = primitive
                && viewport.visible_image_rect(*rect)?.is_some()
                && active_images.insert(image.pixels.id())
            {
                image_bytes += image.pixels.as_ref().len() as u64;
                if image_bytes > MAX_IMAGE_BYTES || active_images.len() > MAX_IMAGES {
                    return Err(RenderError::ImageBudget);
                }
            }
        }
        for attempt in 0..2 {
            self.vertices.clear();
            self.batches.clear();
            match self.prepare(viewport, primitives, &active_images) {
                Err(RenderError::AtlasFull) if attempt == 0 => self.clear_glyph_cache(),
                result => {
                    result?;
                    break;
                }
            }
        }
        self.queue
            .write_buffer(&self.vertex_buffer, 0, bytemuck::cast_slice(&self.vertices));
        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Canvas"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::WHITE),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
            for batch in &self.batches {
                pass.set_pipeline(if batch.image.is_some() {
                    &self.image_pipeline
                } else {
                    &self.pipeline
                });
                let binding = batch
                    .image
                    .map(|id| &self.images[&id].binding)
                    .unwrap_or(&self.bind_group);
                pass.set_bind_group(0, binding, &[]);
                pass.draw(batch.vertices.clone(), 0..1);
            }
        }
        self.queue.submit([encoder.finish()]);
        Ok(())
    }

    fn prepare(
        &mut self,
        viewport: Viewport,
        primitives: &[Primitive<'_>],
        active_images: &HashSet<u64>,
    ) -> Result<(), RenderError> {
        for primitive in primitives {
            let start = self.vertices.len() as u32;
            let mut image_id = None;
            match primitive {
                Primitive::Tag { tag, origin } => {
                    let origin = [
                        viewport.origin[0] + (origin[0] + tag.origin[0]) * viewport.scale,
                        viewport.origin[1] + (origin[1] + tag.origin[1]) * viewport.scale,
                    ];
                    self.tag(Viewport { origin, ..viewport }, tag)?;
                }
                Primitive::Text { layout, origin } => {
                    let origin = [
                        viewport.origin[0] + origin[0] * viewport.scale,
                        viewport.origin[1] + origin[1] * viewport.scale,
                    ];
                    if origin.iter().any(|v| !v.is_finite()) {
                        return Err(RenderError::InvalidPrimitive);
                    }
                    self.text(Viewport { origin, ..viewport }, layout)?;
                }
                Primitive::Rect { rect, color } => {
                    self.quad(
                        viewport,
                        [
                            rect[0] * viewport.scale + viewport.origin[0],
                            rect[1] * viewport.scale + viewport.origin[1],
                            rect[2] * viewport.scale + viewport.origin[0],
                            rect[3] * viewport.scale + viewport.origin[1],
                        ],
                        [0.5 / ATLAS_SIZE as f32; 4],
                        *color,
                    )?;
                }
                Primitive::Image { image, rect } => {
                    if let Some(rect) = viewport.visible_image_rect(*rect)? {
                        self.image(image, active_images)?;
                        image_id = Some(image.pixels.id());
                        self.quad(viewport, rect, [0.0, 0.0, 1.0, 1.0], [1.0; 4])?;
                    }
                }
            }
            let end = self.vertices.len() as u32;
            if start != end {
                if let Some(last) = self.batches.last_mut()
                    && last.image == image_id
                {
                    last.vertices.end = end;
                } else {
                    self.batches.push(Batch {
                        vertices: start..end,
                        image: image_id,
                    });
                }
            }
        }
        Ok(())
    }

    fn image(&mut self, image: &RasterImage, active: &HashSet<u64>) -> Result<(), RenderError> {
        if self.images.contains_key(&image.pixels.id()) {
            return Ok(());
        }
        if image
            .size
            .iter()
            .any(|v| *v > self.device.limits().max_texture_dimension_2d)
        {
            return Err(RenderError::ImageTooLarge);
        }
        let bytes = image.pixels.as_ref().len() as u64;
        while self.images.values().map(|i| i.bytes).sum::<u64>() + bytes > MAX_IMAGE_BYTES
            || self.images.len() >= MAX_IMAGES
        {
            let id = self
                .images
                .keys()
                .find(|id| !active.contains(id))
                .copied()
                .ok_or(RenderError::ImageBudget)?;
            self.images.remove(&id);
        }
        let size = wgpu::Extent3d {
            width: image.size[0],
            height: image.size[1],
            depth_or_array_layers: 1,
        };
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Canvas image"),
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
            image.pixels.as_ref(),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(image.size[0] * 4),
                rows_per_image: Some(image.size[1]),
            },
            size,
        );
        let binding = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Canvas image"),
            layout: &self.pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(
                        &texture.create_view(&Default::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.image_sampler),
                },
            ],
        });
        self.images
            .insert(image.pixels.id(), CachedImage { binding, bytes });
        Ok(())
    }

    fn text(&mut self, viewport: Viewport, layout: &TextLayout) -> Result<(), RenderError> {
        let scale = viewport.scale;
        let origin = viewport.origin;
        for (line, line_box) in layout.lines() {
            let top = line_box.top * scale + origin[1];
            if top + line_box.height * scale < 0.0 || top > viewport.size[1] as f32 {
                continue;
            }
            for item in line.items() {
                let PositionedLayoutItem::GlyphRun(glyph_run) = item else {
                    continue;
                };
                let run = glyph_run.run();
                let data = &run.font().font;
                let size = run.font_size() * scale;
                if !size.is_finite() || size > ATLAS_SIZE as f32 {
                    return Err(RenderError::AtlasFull);
                }
                let synthesis = run.synthesis();
                let coords: Vec<i16> = run
                    .normalized_coords()
                    .iter()
                    .map(|c| c.to_bits())
                    .collect();
                let color = glyph_run.style().brush.color;
                for glyph in glyph_run.positioned_glyphs() {
                    let x = glyph.x * scale + origin[0];
                    let y =
                        (glyph.y + line_box.baseline - line.metrics().baseline) * scale + origin[1];
                    // Quantization affects raster coverage only, never advances or line breaks.
                    let x = (x * 4.0).round() * 0.25;
                    let y = (y * 4.0).round() * 0.25;
                    let glyph_id = glyph
                        .id
                        .try_into()
                        .map_err(|_| RenderError::UnsupportedGlyph)?;
                    let phase = [((x - x.floor()) * 4.0) as u8, ((y - y.floor()) * 4.0) as u8];
                    let key = AtlasKey::Text {
                        font: data.data.id(),
                        index: data.index,
                        glyph: glyph_id,
                        size: size.to_bits(),
                        coords: coords.clone(),
                        phase,
                        embolden: synthesis.embolden(),
                        skew: synthesis.skew().unwrap_or(0.0).to_bits(),
                    };
                    let cached = if let Some(cached) = self.glyphs.get(&key) {
                        *cached
                    } else {
                        if self.glyphs.len() >= MAX_GLYPHS {
                            return Err(RenderError::AtlasFull);
                        }
                        let font = FontRef::from_index(data.data.as_ref(), data.index as usize)
                            .ok_or(RenderError::UnreadableFont)?;
                        let mut scaler = self
                            .scaler
                            .builder_with_id(font, [data.data.id(), u64::from(data.index)])
                            .size(size)
                            .hint(false)
                            .normalized_coords(coords.iter().copied())
                            .build();
                        let mut render = Render::new(&[
                            Source::ColorOutline(0),
                            Source::ColorBitmap(StrikeWith::BestFit),
                            Source::Outline,
                        ]);
                        // Swash rasterizes outlines in Y-up coordinates.
                        render.format(Format::Alpha).offset(Vector::new(
                            f32::from(phase[0]) * 0.25,
                            -f32::from(phase[1]) * 0.25,
                        ));
                        if synthesis.embolden() {
                            render.embolden(size / 24.0);
                        }
                        if let Some(skew) = synthesis.skew() {
                            render.transform(Some(Transform::skew(
                                Angle::from_degrees(skew),
                                Angle::from_degrees(0.0),
                            )));
                        }
                        let image = render.render(&mut scaler, glyph_id);
                        let cached = if let Some(image) =
                            image.filter(|i| i.placement.width > 0 && i.placement.height > 0)
                        {
                            Some(self.upload(image)?)
                        } else {
                            None
                        };
                        self.glyphs.insert(key, cached);
                        cached
                    };
                    if let Some(glyph) = cached {
                        let left = x.floor() + glyph.left as f32;
                        let top = y.floor() - glyph.top as f32;
                        let atlas_size = ATLAS_SIZE as f32;
                        self.quad(
                            viewport,
                            [
                                left,
                                top,
                                left + glyph.width as f32,
                                top + glyph.height as f32,
                            ],
                            [
                                glyph.x as f32 / atlas_size,
                                glyph.y as f32 / atlas_size,
                                (glyph.x + glyph.width) as f32 / atlas_size,
                                (glyph.y + glyph.height) as f32 / atlas_size,
                            ],
                            if glyph.color {
                                [1.0; 4]
                            } else {
                                colorref(color)
                            },
                        )?;
                    }
                }
                let metrics = run.font_metrics();
                for (decoration, offset, thickness) in [
                    (
                        &glyph_run.style().underline,
                        metrics.underline_offset,
                        metrics.underline_size,
                    ),
                    (
                        &glyph_run.style().strikethrough,
                        metrics.strikethrough_offset,
                        metrics.strikethrough_size,
                    ),
                ] {
                    if let Some(decoration) = decoration {
                        let x = glyph_run.offset() * scale + origin[0];
                        let y = (line_box.baseline - decoration.offset.unwrap_or(offset)) * scale
                            + origin[1];
                        self.quad(
                            viewport,
                            [
                                x,
                                y,
                                x + glyph_run.advance() * scale,
                                y + (decoration.size.unwrap_or(thickness) * scale).max(1.0),
                            ],
                            [0.5 / ATLAS_SIZE as f32; 4],
                            colorref(decoration.brush.color),
                        )?;
                    }
                }
            }
        }
        Ok(())
    }

    fn upload(&mut self, image: swash::scale::image::Image) -> Result<AtlasGlyph, RenderError> {
        let p = image.placement;
        if p.width + 2 > ATLAS_SIZE || p.height + 2 > ATLAS_SIZE {
            return Err(RenderError::AtlasFull);
        }
        if self.pen[0] + p.width + 1 > ATLAS_SIZE {
            self.pen[0] = 0;
            self.pen[1] += self.row_height + 1;
            self.row_height = 0;
        }
        if self.pen[1] + p.height + 1 > ATLAS_SIZE {
            return Err(RenderError::AtlasFull);
        }
        let cached = AtlasGlyph {
            x: self.pen[0],
            y: self.pen[1],
            width: p.width,
            height: p.height,
            left: p.left,
            top: p.top,
            color: image.content == Content::Color,
        };
        let rgba = match image.content {
            Content::Mask => image
                .data
                .iter()
                .flat_map(|a| [255, 255, 255, *a])
                .collect(),
            _ => image.data,
        };
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                origin: wgpu::Origin3d {
                    x: cached.x,
                    y: cached.y,
                    z: 0,
                },
                ..self.atlas.as_image_copy()
            },
            &rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(p.width * 4),
                rows_per_image: Some(p.height),
            },
            wgpu::Extent3d {
                width: p.width,
                height: p.height,
                depth_or_array_layers: 1,
            },
        );
        self.pen[0] += p.width + 1;
        self.row_height = self.row_height.max(p.height);
        Ok(cached)
    }

    fn tag(&mut self, viewport: Viewport, tag: &ParagraphTag) -> Result<(), RenderError> {
        let size = ParagraphTag::SIZE * viewport.scale;
        let [x, y] = viewport.origin.map(|v| (v * 4.0).round() * 0.25);
        if !size.is_finite() || [x, y].iter().any(|v| !v.is_finite()) {
            return Err(RenderError::InvalidPrimitive);
        }
        if x + size + 1.0 < 0.0
            || y + size + 1.0 < 0.0
            || x - 1.0 > viewport.size[0] as f32
            || y - 1.0 > viewport.size[1] as f32
        {
            return Ok(());
        }
        if size + 3.0 > ATLAS_SIZE as f32 {
            return Err(RenderError::AtlasFull);
        }
        let phase = [((x - x.floor()) * 4.0) as u8, ((y - y.floor()) * 4.0) as u8];
        let key = AtlasKey::Tag {
            icon: tag.icon,
            size: size.to_bits(),
            phase,
        };
        let glyph = if let Some(Some(glyph)) = self.glyphs.get(&key) {
            *glyph
        } else {
            if self.glyphs.len() >= MAX_GLYPHS {
                return Err(RenderError::AtlasFull);
            }
            let glyph = self.upload(tags::rasterize(tag.icon, size, phase))?;
            self.glyphs.insert(key, Some(glyph));
            glyph
        };
        let left = x.floor();
        let top = y.floor();
        let atlas = ATLAS_SIZE as f32;
        self.quad(
            viewport,
            [
                left,
                top,
                left + glyph.width as f32,
                top + glyph.height as f32,
            ],
            [
                glyph.x as f32 / atlas,
                glyph.y as f32 / atlas,
                (glyph.x + glyph.width) as f32 / atlas,
                (glyph.y + glyph.height) as f32 / atlas,
            ],
            [1.0, 1.0, 1.0, if tag.disabled { 0.45 } else { 1.0 }],
        )
    }

    fn quad(
        &mut self,
        viewport: Viewport,
        rect: [f32; 4],
        uv: [f32; 4],
        color: [f32; 4],
    ) -> Result<(), RenderError> {
        if rect.iter().chain(&color).any(|v| !v.is_finite())
            || rect[2] < rect[0]
            || rect[3] < rect[1]
        {
            return Err(RenderError::InvalidPrimitive);
        }
        if rect[2] < 0.0
            || rect[3] < 0.0
            || rect[0] > viewport.size[0] as f32
            || rect[1] > viewport.size[1] as f32
        {
            return Ok(());
        }
        if self.vertices.len() + 6 > MAX_VERTICES {
            return Err(RenderError::FrameTooLarge);
        }
        let x0 = rect[0] * 2.0 / viewport.size[0] as f32 - 1.0;
        let x1 = rect[2] * 2.0 / viewport.size[0] as f32 - 1.0;
        let y0 = 1.0 - rect[1] * 2.0 / viewport.size[1] as f32;
        let y1 = 1.0 - rect[3] * 2.0 / viewport.size[1] as f32;
        if [x0, x1, y0, y1].iter().any(|v| !v.is_finite()) {
            return Err(RenderError::InvalidPrimitive);
        }
        for (position, uv) in [
            ([x0, y0], [uv[0], uv[1]]),
            ([x0, y1], [uv[0], uv[3]]),
            ([x1, y1], [uv[2], uv[3]]),
            ([x0, y0], [uv[0], uv[1]]),
            ([x1, y1], [uv[2], uv[3]]),
            ([x1, y0], [uv[2], uv[1]]),
        ] {
            self.vertices.push(Vertex {
                position,
                uv,
                color,
            });
        }
        Ok(())
    }
}

fn srgb_byte(value: f32) -> u8 {
    let srgb = if value <= 0.0031308 {
        value * 12.92
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    };
    (srgb * 255.0).round() as u8
}

pub fn colorref(color: u32) -> [f32; 4] {
    let linear = |byte: u32| {
        let value = (byte & 255) as f32 / 255.0;
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    [linear(color), linear(color >> 8), linear(color >> 16), 1.0]
}

#[cfg(test)]
mod tests {
    use super::*;
    use one_canvas::{layout::TextEngine, text::Paragraph};
    use onestore::document::Format as TextFormat;
    use std::time::Duration;

    #[test]
    fn decoding_preserves_pixels_and_rejects_large_or_truncated_images() {
        let mut encoded = Vec::new();
        let mut encoder = png::Encoder::new(&mut encoded, 2, 1);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let rgba = [255, 64, 0, 255, 0, 128, 255, 128];
        encoder
            .write_header()
            .unwrap()
            .write_image_data(&rgba)
            .unwrap();
        let decoded = RasterImage::decode(&encoded).unwrap();
        assert_eq!(decoded.size, [2, 1]);
        assert_eq!(decoded.pixels.as_ref(), [255, 64, 0, 255, 0, 93, 188, 128]);
        for end in 0..encoded.len() - 12 {
            assert!(RasterImage::decode(&encoded[..end]).is_err(), "{end}");
        }

        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut jpeg)
            .encode(&[255; 4 * 4 * 3], 4, 4, image::ExtendedColorType::Rgb8)
            .unwrap();
        let decoded = RasterImage::decode(&jpeg).unwrap();
        assert_eq!(decoded.size, [4, 4]);
        assert_eq!(decoded.pixels.as_ref(), [255; 4 * 4 * 4]);

        for size in [[8192, 8192], [16_385, 1]] {
            let mut header = Vec::new();
            let mut encoder = png::Encoder::new(&mut header, size[0], size[1]);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer.write_chunk(png::chunk::IDAT, &[]).unwrap();
            drop(writer);
            let error = RasterImage::decode(&header).err().unwrap();
            if size[0] == 8192 {
                assert!(matches!(error, RenderError::ImageBudget), "{error:?}");
            } else {
                assert!(
                    matches!(
                        error,
                        RenderError::ImageDecode(image::ImageError::Limits(_))
                    ),
                    "{error:?}"
                );
            }
        }
    }

    #[test]
    fn image_pixels_have_valid_dimensions_and_immutable_clone_identity() {
        assert!(matches!(
            RasterImage::new([0, 1], Vec::new()),
            Err(RenderError::InvalidImage)
        ));
        assert!(matches!(
            RasterImage::new([2, 2], vec![255; 15]),
            Err(RenderError::InvalidImage)
        ));
        assert!(matches!(
            RasterImage::new([u32::MAX; 2], Vec::new()),
            Err(RenderError::InvalidImage)
        ));
        let image = RasterImage::new([1, 1], vec![1, 2, 3, 255]).unwrap();
        assert_eq!(image.pixels.id(), image.clone().pixels.id());
    }

    #[test]
    #[ignore = "requires a native GPU adapter"]
    fn eviction_and_repaint_reproduce_pixels() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
        let (device, queue) =
            pollster::block_on(adapter.request_device(&Default::default())).unwrap();
        let format = wgpu::TextureFormat::Rgba8UnormSrgb;
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Canvas readback test"),
            size: wgpu::Extent3d {
                width: 512,
                height: 256,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Canvas readback"),
            size: 512 * 256 * 4,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut renderer = Renderer::new(device, queue, format);
        let mut engine = TextEngine::default();
        let layout = engine
            .layout(
                &Paragraph::new(
                    "Hello 🌳\nCafé e\u{301} שלום".into(),
                    TextFormat {
                        underline: Some(true),
                        ..TextFormat::default()
                    },
                ),
                220.0,
            )
            .unwrap();
        let viewport = Viewport {
            size: [512, 256],
            scale: 2.0,
            origin: [24.0; 2],
        };
        let mut captures = Vec::new();
        let image = RasterImage::new(
            [2, 2],
            vec![255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 0, 0, 0, 0],
        )
        .unwrap();
        let transparent_edge =
            RasterImage::new([2, 1], vec![255, 0, 0, 255, 0, 0, 255, 0]).unwrap();
        let tags = [
            TagIcon::CheckBox { checked: false },
            TagIcon::CheckBox { checked: true },
            TagIcon::Question,
            TagIcon::Music,
        ]
        .map(|icon| ParagraphTag {
            icon,
            origin: [0.0; 2],
            label: String::new(),
            disabled: false,
        });
        let phase_layout = engine
            .layout(
                &Paragraph::new(
                    "H".into(),
                    TextFormat {
                        font: Some("Arial".into()),
                        font_size: Some(11.0),
                        ..TextFormat::default()
                    },
                ),
                20.0,
            )
            .unwrap();
        let mut primitives = vec![
            Primitive::Rect {
                rect: [0.0, 0.0, 70.0, 14.0],
                color: [0.55, 0.73, 1.0, 0.5],
            },
            Primitive::Text {
                layout: &layout,
                origin: [0.0; 2],
            },
            Primitive::Text {
                layout: &layout,
                origin: [8.0, 64.0],
            },
            Primitive::Image {
                image: &image,
                rect: [128.0, 8.0, 160.0, 40.0],
            },
            Primitive::Rect {
                rect: [128.0, 8.0, 132.0, 12.0],
                color: [0.0, 0.0, 1.0, 1.0],
            },
            Primitive::Image {
                image: &transparent_edge,
                rect: [176.0, 8.0, 208.0, 40.0],
            },
            Primitive::Tag {
                tag: &tags[0],
                origin: [128.125, 80.25],
            },
            Primitive::Tag {
                tag: &tags[1],
                origin: [148.125, 80.25],
            },
            Primitive::Tag {
                tag: &tags[2],
                origin: [168.125, 80.25],
            },
            Primitive::Tag {
                tag: &tags[3],
                origin: [188.125, 80.25],
            },
        ];
        primitives.extend((0..9).map(|step| Primitive::Text {
            layout: &phase_layout,
            origin: [8.0 + 24.0 * step as f32, 96.0 + 0.125 * step as f32],
        }));
        for pass in 0..4 {
            if pass == 2 {
                renderer.clear_glyph_cache();
                renderer.images.clear();
            } else if pass == 3 {
                renderer = Renderer::new(renderer.device.clone(), renderer.queue.clone(), format);
            }
            renderer
                .draw(
                    &target.create_view(&Default::default()),
                    viewport,
                    &primitives,
                )
                .unwrap();
            let mut encoder = renderer.device.create_command_encoder(&Default::default());
            encoder.copy_texture_to_buffer(
                target.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &readback,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(512 * 4),
                        rows_per_image: Some(256),
                    },
                },
                wgpu::Extent3d {
                    width: 512,
                    height: 256,
                    depth_or_array_layers: 1,
                },
            );
            renderer.queue.submit([encoder.finish()]);
            let (sender, receiver) = std::sync::mpsc::channel();
            readback.map_async(wgpu::MapMode::Read, .., move |result| {
                sender.send(result).unwrap();
            });
            renderer
                .device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: Some(Duration::from_secs(5)),
                })
                .unwrap();
            receiver
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .unwrap();
            captures.push(readback.get_mapped_range(..).unwrap().to_vec());
            readback.unmap();
        }
        let centroids: Vec<_> = (0..9)
            .map(|step| {
                let mut weight = 0.0_f64;
                let mut moment = 0.0_f64;
                for y in 212..252 {
                    for x in (40 + 48 * step)..(72 + 48 * step) {
                        let value = captures[0][(y * 512 + x) * 4];
                        let coverage = 1.0 - f64::from(colorref(u32::from(value))[0]);
                        weight += coverage;
                        moment += coverage * y as f64;
                    }
                }
                assert!(weight > 10.0, "phase sample {step} was not painted");
                moment / weight
            })
            .collect();
        for (step, centroid) in centroids.iter().enumerate() {
            let movement = centroid - centroids[0];
            assert!(
                (movement - step as f64 * 0.25).abs() < 0.06,
                "quarter-pixel step {step} moved the glyph by {movement} px"
            );
        }
        assert!(!renderer.glyphs.is_empty());
        assert!(renderer.glyphs.len() <= MAX_GLYPHS);
        assert_eq!(
            renderer
                .glyphs
                .keys()
                .filter(|key| matches!(key, AtlasKey::Tag { .. }))
                .count(),
            4
        );
        for x in [280, 320, 360, 400] {
            let colored = (184..210)
                .flat_map(|y| (x..x + 26).map(move |x| (y * 512 + x) * 4))
                .filter(|offset| captures[0][*offset..*offset + 3].iter().any(|v| *v < 180))
                .count();
            assert!(colored > 10, "tag at {x} was not painted");
        }
        let count = renderer.glyphs.len();
        renderer
            .draw(
                &target.create_view(&Default::default()),
                viewport,
                &[Primitive::Tag {
                    tag: &tags[0],
                    origin: [10000.0; 2],
                }],
            )
            .unwrap();
        assert_eq!(renderer.glyphs.len(), count);
        assert!(
            captures[0]
                .chunks_exact(4)
                .filter(|pixel| pixel[0] < 200)
                .count()
                > 100
        );
        assert_eq!(captures[0], captures[1]);
        assert_eq!(captures[0], captures[2]);
        assert_eq!(captures[0], captures[3]);
        for (x, y, color) in [
            (288, 48, [255, 0, 0, 255]),
            (336, 96, [255; 4]),
            (284, 44, [0, 0, 255, 255]),
        ] {
            let offset = (y * 512 + x) * 4;
            assert_eq!(captures[0][offset..offset + 4], color);
        }
        let edge = &captures[0][(48 * 512 + 408) * 4..(48 * 512 + 408) * 4 + 4];
        assert_eq!(edge[0], 255, "{edge:?}");
        assert_eq!(edge[1], edge[2], "{edge:?}");
        assert!((180..=200).contains(&edge[1]), "{edge:?}");
        assert_eq!(renderer.images.len(), 2);
        for primitive in [
            Primitive::Rect {
                rect: [0.0, 0.0, f32::NAN, 1.0],
                color: [1.0; 4],
            },
            Primitive::Rect {
                rect: [2.0, 0.0, 1.0, 1.0],
                color: [1.0; 4],
            },
            Primitive::Text {
                layout: &layout,
                origin: [f32::MAX; 2],
            },
        ] {
            assert!(matches!(
                renderer.draw(
                    &target.create_view(&Default::default()),
                    viewport,
                    &[primitive]
                ),
                Err(RenderError::InvalidPrimitive)
            ));
        }
        let too_many: Vec<_> = (0..=MAX_IMAGES)
            .map(|_| RasterImage::new([1, 1], vec![255; 4]).unwrap())
            .collect();
        let primitives: Vec<_> = too_many
            .iter()
            .map(|image| Primitive::Image {
                image,
                rect: [0.0, 0.0, 1.0, 1.0],
            })
            .collect();
        assert!(matches!(
            renderer.draw(
                &target.create_view(&Default::default()),
                viewport,
                &primitives
            ),
            Err(RenderError::ImageBudget)
        ));
        let primitives: Vec<_> = too_many
            .iter()
            .map(|image| Primitive::Image {
                image,
                rect: [-20.0, -20.0, -19.0, -19.0],
            })
            .collect();
        renderer
            .draw(
                &target.create_view(&Default::default()),
                viewport,
                &primitives,
            )
            .unwrap();
        assert_eq!(renderer.images.len(), 2);
        let mut seen = HashSet::from([image.pixels.id(), transparent_edge.pixels.id()]);
        let mut previous = None;
        for _ in 0..5 {
            let large = RasterImage::new([2048; 2], vec![255; 2048 * 2048 * 4]).unwrap();
            seen.insert(large.pixels.id());
            let mut primitives = vec![Primitive::Image {
                image: &large,
                rect: [0.0, 0.0, 16.0, 16.0],
            }];
            if let Some(image) = &previous {
                primitives.push(Primitive::Image {
                    image,
                    rect: [16.0, 0.0, 32.0, 16.0],
                });
            }
            renderer
                .draw(
                    &target.create_view(&Default::default()),
                    viewport,
                    &primitives,
                )
                .unwrap();
            assert!(renderer.images.contains_key(&large.pixels.id()));
            if let Some(image) = &previous {
                assert!(renderer.images.contains_key(&image.pixels.id()));
            }
            assert!(renderer.images.values().map(|i| i.bytes).sum::<u64>() <= MAX_IMAGE_BYTES);
            previous = Some(large);
        }
        assert!(renderer.images.len() <= MAX_IMAGES);
        assert!(seen.iter().any(|id| !renderer.images.contains_key(id)));
        renderer.images.clear();
        renderer
            .draw(
                &target.create_view(&Default::default()),
                viewport,
                &[Primitive::Image {
                    image: &image,
                    rect: [128.0, 8.0, 160.0, 40.0],
                }],
            )
            .unwrap();
        assert_eq!(renderer.images.len(), 1);
        assert!(renderer.images.contains_key(&image.pixels.id()));
    }
}
