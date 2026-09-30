#[cfg(all(feature = "wgpu", windows))]
mod dual;
#[cfg(any(windows, not(feature = "wgpu")))]
mod gl;
mod icon;
#[cfg(feature = "pdf")]
mod pdf;
mod text;
#[cfg(feature = "wgpu")]
mod translucent;
#[cfg(feature = "wgpu")]
mod webgpu;

#[cfg(all(feature = "wgpu", windows))]
use dual as backend;
#[cfg(not(feature = "wgpu"))]
use gl as backend;
#[cfg(all(feature = "wgpu", not(windows)))]
use webgpu as backend;

pub use backend::Target;
#[cfg(all(feature = "wgpu", windows))]
pub use gl::Target as GlTarget;
pub use icon::{Palette, picture_icon};
#[cfg(feature = "pdf")]
pub use pdf::{Sheet, pdf};
pub use text::{Decoration, Glyph, GlyphRun, Glyphs, paint_parley_run};
#[cfg(feature = "wgpu")]
pub use translucent::Translucent;

use bytemuck::{Pod, Zeroable};
use image::ImageDecoder;
use linebender_resource_handle::WeakBlob;
use parley::fontique::Blob;
use std::{
    collections::{HashMap, HashSet},
    ops::Range,
};
use swash::{
    FontRef,
    scale::{Render, ScaleContext, Source, StrikeWith, image::Content},
    zeno::{Angle, Cap, Format, Join, Mask, Transform, Vector},
};

/// The atlas's side at first; it doubles, up to `MAX_ATLAS_SIZE`, when one frame needs
/// more than half of it.
const ATLAS_SIZE: u32 = 2048;
const MAX_ATLAS_SIZE: u32 = 8192;
/// Atlas entries per `ATLAS_SIZE` square of atlas.
const MAX_GLYPHS: usize = 8192;
const MAX_VERTICES: usize = 65_536;
const VERTEX_BUFFER_BYTES: u64 = (MAX_VERTICES * size_of::<Vertex>()) as u64;
/// Decoded bytes of all images one frame may paint.
pub const MAX_IMAGE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_IMAGES: usize = 256;
/// Decoded bytes one picture may take before `RasterImage::decode` shrinks it.
const MAX_DECODE_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Clone)]
pub struct RasterImage {
    size: [u32; 2],
    pixels: Blob<u8>,
}

/// A PNG, JPEG, GIF or TIFF decoder for `encoded`, refusing pictures past the decode limits.
fn decoder(encoded: &[u8]) -> Result<impl ImageDecoder + '_, RenderError> {
    if encoded.len() as u64 > MAX_DECODE_BYTES {
        return Err(RenderError::ImageBudget);
    }
    let format = image::guess_format(encoded).map_err(RenderError::ImageDecode)?;
    let mut reader = image::ImageReader::with_format(std::io::Cursor::new(encoded), format);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16_384);
    limits.max_image_height = Some(16_384);
    limits.max_alloc = Some(MAX_DECODE_BYTES);
    reader.limits(limits.clone());
    let mut decoder = reader.into_decoder().map_err(RenderError::ImageDecode)?;
    let (width, height) = decoder.dimensions();
    if u64::from(width) * u64::from(height) * 4 > MAX_DECODE_BYTES
        || decoder.total_bytes() > MAX_DECODE_BYTES
    {
        return Err(RenderError::ImageBudget);
    }
    limits
        .reserve(decoder.total_bytes())
        .map_err(RenderError::ImageDecode)?;
    decoder
        .set_limits(limits)
        .map_err(RenderError::ImageDecode)?;
    Ok(decoder)
}

/// Premultiplies straight-alpha sRGB RGBA in linear light, so filtering never bleeds the
/// colour of transparent pixels.
fn premultiply(pixels: &mut [u8]) {
    for pixel in pixels.chunks_exact_mut(4) {
        let alpha = pixel[3];
        if alpha == 255 {
            continue;
        }
        let linear = srgb(pixel[0], pixel[1], pixel[2]);
        for (byte, value) in pixel[..3].iter_mut().zip(linear) {
            *byte = srgb_byte(value * f32::from(alpha) / 255.0);
        }
    }
}

impl RasterImage {
    /// The pixel size of an encoded picture, read from its header, or why `decode` would
    /// refuse it.
    pub fn measure(encoded: &[u8]) -> Result<[u32; 2], RenderError> {
        let (width, height) = decoder(encoded)?.dimensions();
        Ok([width, height])
    }

    /// Decodes a picture, shrinking it to at most `within` pixels along each axis.
    pub fn decode(encoded: &[u8], within: [u32; 2]) -> Result<Self, RenderError> {
        let decoder = decoder(encoded)?;
        let mut pixels = image::DynamicImage::from_decoder(decoder)
            .map_err(RenderError::ImageDecode)?
            .into_rgba8();
        premultiply(&mut pixels);
        let size = [pixels.width(), pixels.height()];
        let shown = [0, 1].map(|axis| size[axis].min(within[axis]).max(1));
        if shown != size {
            pixels = image::imageops::resize(
                &pixels,
                shown[0],
                shown[1],
                image::imageops::FilterType::Triangle,
            );
        }
        Self::premultiplied(shown, pixels.into_raw())
    }

    /// Immutable, straight-alpha sRGB RGBA pixels retain one cache identity across clones.
    pub fn new(size: [u32; 2], mut pixels: Vec<u8>) -> Result<Self, RenderError> {
        premultiply(&mut pixels);
        Self::premultiplied(size, pixels)
    }

    fn premultiplied(size: [u32; 2], pixels: Vec<u8>) -> Result<Self, RenderError> {
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
        Ok(Self {
            size,
            pixels: pixels.into(),
        })
    }

    pub fn size(&self) -> [u32; 2] {
        self.size
    }

    /// The identity caches and uploads share across clones.
    pub fn id(&self) -> u64 {
        self.pixels.id()
    }

    /// Premultiplied sRGB RGBA rows.
    pub fn pixels(&self) -> &[u8] {
        self.pixels.as_ref()
    }
}

/// What a backend submits: the prepared vertices and batches, and the images they paint.
struct Frame<'a> {
    vertices: &'a [Vertex],
    batches: &'a [Batch],
    groups: &'a [Group],
    images: &'a HashMap<u64, CachedImage>,
}

struct CachedImage {
    texture: backend::Image,
    bytes: u64,
    /// The texture goes when every copy of its image has.
    pixels: WeakBlob<u8>,
}

struct Batch {
    vertices: Range<u32>,
    blend: Blend,
    scissor: [u32; 4],
}

/// Batches that paint offscreen as one, then onto the target by `composite`'s vertices.
struct Group {
    batches: Range<usize>,
    composite: Range<u32>,
}

/// How a batch meets what lies beneath it.
#[derive(Clone, Copy, PartialEq)]
enum Blend {
    Over,
    /// A premultiplied picture over it.
    Image(u64),
    /// Clearing it by the batch's coverage.
    Erase,
    /// Multiplying it by the batch's colour where covered.
    Multiply,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    position: [f32; 2],
    uv: [f32; 2],
    color: [f32; 4],
    local: [f32; 2],
    shape: [f32; 4],
    stroke: f32,
    /// The position from the middle of the rounded rectangle the vertex paints within, and
    /// that rectangle's half size and corner radius; a zero half size paints everywhere.
    clip_local: [f32; 2],
    clip: [f32; 3],
    /// The standard deviation, in device pixels, of the blur that makes `shape` a shadow;
    /// negative, `shape` is a tapered capsule's half length and end radii.
    blur: f32,
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
        /// The ink's sRGB luminance in sixteenths, which `text_coverage` weights by.
        tone: u8,
    },
    Icon {
        sources: &'static [&'static str],
        size: u32,
        /// The bits of the colour `currentColor` paints.
        ink: [u32; 3],
        palette: [Option<[u32; 3]>; 3],
    },
    Path {
        data: String,
        scale: u32,
        /// The style's kind and width bits.
        style: (u8, u32),
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

/// Primitives sharing one transform and clip.
pub struct Layer<'a> {
    /// Device pixels per layer unit.
    pub scale: f32,
    /// Device position of the layer origin.
    pub origin: [f32; 2],
    /// Device bounds `[left, top, right, bottom]` the layer paints within.
    pub clip: Option<[f32; 4]>,
    /// The colour behind the layer: on a dark one, text in dark colours of its own is
    /// lifted to stay legible against what lies behind it, as OneNote's dark page does.
    pub backdrop: Option<[f32; 4]>,
    /// Device bounds and corner radius of a rounded rectangle the layer paints only inside.
    pub round: Option<([f32; 4], f32)>,
    /// Consecutive layers with the same motion appear as one: painted together offscreen,
    /// then faded and leaned back as a whole, so none shows through another.
    pub motion: Option<Motion>,
    pub primitives: &'a [Primitive<'a>],
}

/// A layer part of the way through appearing: faded, and leaned back in perspective.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Motion {
    pub opacity: f32,
    /// Radians the layer turns about the horizontal line through `pivot`, its part below
    /// the line leaning away from the viewer.
    pub tilt: f32,
    /// Device point the layer turns about.
    pub pivot: [f32; 2],
}

impl Motion {
    /// Where device point `[x, y]` on a target `height` tall shows, turned.
    fn project(&self, [x, y]: [f32; 2], height: f32) -> [f32; 2] {
        // Seen from twice the target's height away, so a small turn reads as depth.
        let distance = 2.0 * height;
        let below = y - self.pivot[1];
        let near = distance / (distance + below * self.tilt.sin());
        [
            self.pivot[0] + (x - self.pivot[0]) * near,
            self.pivot[1] + below * self.tilt.cos() * near,
        ]
    }
}

/// A layer's transform onto the target.
#[derive(Clone, Copy)]
struct Space {
    size: [u32; 2],
    scale: f32,
    origin: [f32; 2],
    backdrop: Option<[f32; 4]>,
    round: Option<([f32; 4], f32)>,
}

impl Space {
    fn visible_rect(&self, rect: [f32; 4]) -> Result<Option<[f32; 4]>, RenderError> {
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

    /// Whole device pixels covering `rect` within the target, as a scissor rectangle.
    fn scissor(&self, rect: [f32; 4]) -> [u32; 4] {
        let left = rect[0].max(0.0).floor() as u32;
        let top = rect[1].max(0.0).floor() as u32;
        let right = (rect[2].min(self.size[0] as f32).ceil() as u32).max(left);
        let bottom = (rect[3].min(self.size[1] as f32).ceil() as u32).max(top);
        [left, top, right - left, bottom - top]
    }
}

fn intersect(a: [u32; 4], b: [u32; 4]) -> [u32; 4] {
    let left = a[0].max(b[0]);
    let top = a[1].max(b[1]);
    let right = (a[0] + a[2]).min(b[0] + b[2]).max(left);
    let bottom = (a[1] + a[3]).min(b[1] + b[3]).max(top);
    [left, top, right - left, bottom - top]
}

/// How a path paints; widths are layer units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PathStyle {
    Fill,
    /// Round caps and joins.
    Stroke(f32),
    /// Filled and blurred this far, as a soft shadow.
    Shadow(f32),
    /// Filled, clearing what lies beneath towards transparency by the colours' opacity, so
    /// whatever the system shows behind the window shows through.
    Erase,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Stroke {
    Solid(f32),
    /// Dash and gap lengths are twice the stroke width.
    Dashed(f32),
}

/// Coordinates are layer units; colors are linear RGBA.
pub enum Primitive<'a> {
    Text {
        text: &'a dyn Glyphs,
        origin: [f32; 2],
        /// Layer bounds of the painted glyphs, which keep their full shaping and advances.
        clip: Option<[f32; 4]>,
        /// The colour of runs and decorations without their own.
        ink: [f32; 4],
    },
    /// 16×16 SVG artwork painted `size` units square, later sources over earlier ones.
    /// `currentColor` paints in `tint`'s colour, slots take `palette`'s, other colours keep
    /// theirs, and `tint`'s opacity fades the whole icon.
    Icon {
        sources: &'static [&'static str],
        origin: [f32; 2],
        size: f32,
        tint: [f32; 4],
        palette: Palette,
    },
    /// SVG path data in layer units from `origin`, shaded from `colors[0]` at the top of
    /// what it paints to `colors[1]` at the bottom. Each distinct path is rasterized once
    /// per scale and style.
    Path {
        data: &'a str,
        origin: [f32; 2],
        style: PathStyle,
        colors: [[f32; 4]; 2],
    },
    Rect {
        rect: [f32; 4],
        color: [f32; 4],
    },
    RoundedRect {
        rect: [f32; 4],
        radius: [f32; 2],
        /// None fills the shape; stroke widths are positive layer units.
        stroke: Option<Stroke>,
        color: [f32; 4],
    },
    /// A rounded rectangle filled from `colors[0]` at its top to `colors[1]` at its bottom.
    Gradient {
        rect: [f32; 4],
        radius: [f32; 2],
        colors: [[f32; 4]; 2],
    },
    /// The soft shadow a rounded rectangle casts, blurred `blur` units wide as CSS's
    /// `box-shadow` takes it.
    Shadow {
        rect: [f32; 4],
        radius: f32,
        blur: f32,
        color: [f32; 4],
    },
    Image {
        image: &'a RasterImage,
        rect: [f32; 4],
    },
    /// A pen stroke between two points, `width` units across.
    Segment {
        from: [f32; 2],
        to: [f32; 2],
        width: f32,
        /// A round pen tip caps the ends; otherwise they are square.
        round: bool,
        color: [f32; 4],
    },
    /// A round pen's stroke between two points, `widths` units across at each, joined by the
    /// lines touching both ends, as a pressure pen's width changes along a stroke.
    Taper {
        from: [f32; 2],
        to: [f32; 2],
        widths: [f32; 2],
        color: [f32; 4],
    },
    /// A highlighter's stroke between two points, `width` units across with square ends:
    /// its opaque `color` multiplies what lies beneath, so text under it stays dark.
    Highlight {
        from: [f32; 2],
        to: [f32; 2],
        width: f32,
        color: [f32; 4],
    },
}

#[derive(Debug)]
pub enum RenderError {
    AtlasFull,
    FrameTooLarge,
    UnreadableFont,
    UnsupportedGlyph,
    InvalidLayer,
    InvalidPrimitive,
    InvalidImage,
    ImageBudget,
    ImageTooLarge,
    ImageDecode(image::ImageError),
}

/// Cache and buffer use after the last frame, for resource probes. Texture and buffer
/// sizes are as requested, without driver overhead.
#[derive(Debug)]
pub struct Occupancy {
    /// Glyph and icon atlas entries, including glyphs with no pixels.
    pub glyphs: usize,
    pub atlas_texels: u64,
    pub images: usize,
    pub image_bytes: u64,
    pub vertices: usize,
    pub batches: usize,
    pub vertex_capacity_bytes: usize,
    pub batch_capacity_bytes: usize,
    pub glyph_capacity: usize,
    pub image_capacity: usize,
    pub atlas_bytes: u64,
    pub vertex_buffer_bytes: u64,
    pub within_budget: bool,
}

pub struct Renderer {
    gpu: backend::Gpu,
    vertices: Vec<Vertex>,
    glyphs: HashMap<AtlasKey, Option<AtlasGlyph>>,
    images: HashMap<u64, CachedImage>,
    batches: Vec<Batch>,
    groups: Vec<Group>,
    /// Batches before this one take no more primitives.
    barrier: usize,
    scaler: ScaleContext,
    pen: [u32; 2],
    row_height: u32,
}

impl Renderer {
    fn with_gpu(gpu: backend::Gpu) -> Self {
        let renderer = Self {
            gpu,
            vertices: Vec::new(),
            glyphs: HashMap::new(),
            images: HashMap::new(),
            batches: Vec::new(),
            groups: Vec::new(),
            barrier: 0,
            scaler: ScaleContext::with_max_entries(32),
            pen: [1, 0],
            row_height: 1,
        };
        renderer.gpu.write_atlas([0, 0], [1, 1], &[255; 4]);
        renderer
    }

    /// The widest and tallest texture the device takes, in pixels.
    pub fn max_texture_dimension(&self) -> u32 {
        self.gpu.max_texture_dimension()
    }

    fn atlas_side(&self) -> u32 {
        self.gpu.atlas_side()
    }

    /// Replaces the atlas with an empty one twice as wide and tall.
    fn grow_atlas(&mut self) {
        self.gpu.new_atlas(self.atlas_side() * 2);
        self.clear_glyph_cache();
        self.gpu.write_atlas([0, 0], [1, 1], &[255; 4]);
    }

    fn glyph_limit(&self) -> usize {
        MAX_GLYPHS * (self.atlas_side() / ATLAS_SIZE).pow(2) as usize
    }

    fn uv(&self, glyph: AtlasGlyph) -> [f32; 4] {
        let side = self.atlas_side() as f32;
        [
            glyph.x as f32 / side,
            glyph.y as f32 / side,
            (glyph.x + glyph.width) as f32 / side,
            (glyph.y + glyph.height) as f32 / side,
        ]
    }

    /// The atlas's first texel, the white that solid fills sample.
    fn white(&self) -> [f32; 4] {
        [0.5 / self.atlas_side() as f32; 4]
    }

    pub fn clear_glyph_cache(&mut self) {
        self.glyphs.clear();
        self.pen = [1, 0];
        self.row_height = 1;
    }

    /// Forgets uploaded images; they upload again when next painted.
    pub fn clear_images(&mut self) {
        self.images.clear();
    }

    pub fn occupancy(&self) -> Occupancy {
        let image_bytes = self.images.values().map(|image| image.bytes).sum();
        Occupancy {
            glyphs: self.glyphs.len(),
            atlas_texels: self
                .glyphs
                .values()
                .flatten()
                .map(|glyph| u64::from(glyph.width) * u64::from(glyph.height))
                .sum(),
            images: self.images.len(),
            image_bytes,
            vertices: self.vertices.len(),
            batches: self.batches.len(),
            vertex_capacity_bytes: self.vertices.capacity() * size_of::<Vertex>(),
            batch_capacity_bytes: self.batches.capacity() * size_of::<Batch>(),
            glyph_capacity: self.glyphs.capacity(),
            image_capacity: self.images.capacity(),
            atlas_bytes: u64::from(self.atlas_side()).pow(2) * 4,
            vertex_buffer_bytes: VERTEX_BUFFER_BYTES,
            within_budget: self.glyphs.len() <= self.glyph_limit()
                && self.images.len() <= MAX_IMAGES
                && image_bytes <= MAX_IMAGE_BYTES
                && self.vertices.len() <= MAX_VERTICES,
        }
    }

    /// Clears `target`, `size` device pixels, to linear `clear` and paints the layers in order.
    pub fn draw(
        &mut self,
        target: &Target,
        size: [u32; 2],
        clear: [f32; 4],
        layers: &[Layer<'_>],
    ) -> Result<(), RenderError> {
        if size.contains(&0)
            || clear.iter().any(|v| !v.is_finite())
            || layers.iter().any(|layer| {
                !layer.scale.is_finite()
                    || layer.scale <= 0.0
                    || layer.origin.iter().any(|v| !v.is_finite())
                    || layer
                        .clip
                        .is_some_and(|clip| clip.iter().any(|v| !v.is_finite()))
            })
        {
            return Err(RenderError::InvalidLayer);
        }
        self.images
            .retain(|_, image| image.pixels.upgrade().is_some());
        let mut active_images = HashSet::new();
        let mut image_bytes = 0;
        for layer in layers {
            let space = Space {
                size,
                scale: layer.scale,
                origin: layer.origin,
                backdrop: layer.backdrop,
                round: layer.round,
            };
            for primitive in layer.primitives {
                if let Primitive::Image { image, rect } = primitive
                    && space.visible_rect(*rect)?.is_some()
                    && active_images.insert(image.id())
                {
                    image_bytes += image.pixels().len() as u64;
                    if image_bytes > MAX_IMAGE_BYTES || active_images.len() > MAX_IMAGES {
                        return Err(RenderError::ImageBudget);
                    }
                }
            }
        }
        let limit = self.max_texture_dimension().min(MAX_ATLAS_SIZE);
        let mut cleared = false;
        loop {
            self.vertices.clear();
            self.batches.clear();
            self.groups.clear();
            self.barrier = 0;
            let full = match self.prepare(size, layers, &active_images) {
                Err(RenderError::AtlasFull) => true,
                result => {
                    result?;
                    false
                }
            };
            // Once cleared, the atlas holds only this frame's entries; past half full, the
            // next frames would clear it again and again.
            let crowded = full || (cleared && 2 * self.pen[1] > self.atlas_side());
            if !crowded {
                break;
            }
            if !cleared {
                self.clear_glyph_cache();
                cleared = true;
            } else if self.atlas_side() * 2 <= limit {
                self.grow_atlas();
            } else if full {
                return Err(RenderError::AtlasFull);
            } else {
                break;
            }
        }
        let frame = Frame {
            vertices: &self.vertices,
            batches: &self.batches,
            groups: &self.groups,
            images: &self.images,
        };
        self.gpu.submit(&frame, target, size, clear);
        Ok(())
    }

    fn prepare(
        &mut self,
        size: [u32; 2],
        layers: &[Layer<'_>],
        active_images: &HashSet<u64>,
    ) -> Result<(), RenderError> {
        // The motion the layers painted since `start` share, where they paint as a group.
        let mut group: Option<(Motion, usize)> = None;
        for layer in layers {
            let motion = layer
                .motion
                .filter(|motion| motion.opacity < 1.0 || motion.tilt != 0.0);
            if motion != group.map(|(motion, _)| motion) {
                if let Some((motion, start)) = group {
                    self.group(motion, start, size)?;
                }
                // A group's batches never merge with those outside it.
                self.barrier = self.batches.len();
                group = motion.map(|motion| (motion, self.batches.len()));
            }
            let space = Space {
                size,
                scale: layer.scale,
                origin: layer.origin,
                backdrop: layer.backdrop,
                round: layer.round,
            };
            let bounds = match layer.clip {
                Some(clip) => space.scissor(clip),
                None => [0, 0, size[0], size[1]],
            };
            if bounds[2] == 0 || bounds[3] == 0 {
                continue;
            }
            for primitive in layer.primitives {
                self.primitive(space, bounds, primitive, active_images)?;
            }
        }
        if let Some((motion, start)) = group {
            self.group(motion, start, size)?;
        }
        Ok(())
    }

    /// Makes the batches from `start` a group appearing by `motion`: the vertices that
    /// paint its offscreen picture over the target, in strips so the turn stays in
    /// perspective across the picture.
    fn group(&mut self, motion: Motion, start: usize, size: [u32; 2]) -> Result<(), RenderError> {
        let batches = start..self.batches.len();
        if batches.is_empty() {
            return Ok(());
        }
        let [width, height] = size.map(|side| side as f32);
        let [mut left, mut top, mut right, mut bottom] = [width, height, 0.0, 0.0];
        let first = self.batches[start].vertices.start as usize;
        for vertex in &self.vertices[first..] {
            let [x, y] = [
                (vertex.position[0] + 1.0) * width / 2.0,
                (1.0 - vertex.position[1]) * height / 2.0,
            ];
            [left, top] = [left.min(x), top.min(y)];
            [right, bottom] = [right.max(x), bottom.max(y)];
        }
        let [left, top] = [left.max(0.0).floor(), top.max(0.0).floor()];
        let [right, bottom] = [right.min(width).ceil(), bottom.min(height).ceil()];
        let strips = if motion.tilt == 0.0 { 1 } else { 32 };
        if self.vertices.len() + 6 * strips > MAX_VERTICES {
            return Err(RenderError::FrameTooLarge);
        }
        let from = self.vertices.len() as u32;
        let flipped = self.gpu.flipped();
        let corner = |x: f32, y: f32| {
            let [shown_x, shown_y] = motion.project([x, y], height);
            let v = y / height;
            Vertex {
                position: [shown_x * 2.0 / width - 1.0, 1.0 - shown_y * 2.0 / height],
                uv: [x / width, if flipped { 1.0 - v } else { v }],
                color: [motion.opacity; 4],
                local: [0.0; 2],
                shape: [0.0; 4],
                stroke: 0.0,
                clip_local: [0.0; 2],
                clip: [0.0; 3],
                blur: 0.0,
            }
        };
        for strip in 0..strips {
            let [y0, y1] =
                [strip, strip + 1].map(|edge| top + (bottom - top) * edge as f32 / strips as f32);
            let [a, b, c, d] = [
                corner(left, y0),
                corner(left, y1),
                corner(right, y1),
                corner(right, y0),
            ];
            self.vertices.extend([a, b, c, a, c, d]);
        }
        self.groups.push(Group {
            batches,
            composite: from..self.vertices.len() as u32,
        });
        Ok(())
    }

    fn primitive(
        &mut self,
        space: Space,
        bounds: [u32; 4],
        primitive: &Primitive<'_>,
        active_images: &HashSet<u64>,
    ) -> Result<(), RenderError> {
        let start = self.vertices.len() as u32;
        let mut blend = Blend::Over;
        let mut scissor = bounds;
        match primitive {
            Primitive::Icon {
                sources,
                origin,
                size,
                tint,
                palette,
            } => {
                let origin = [
                    space.origin[0] + origin[0] * space.scale,
                    space.origin[1] + origin[1] * space.scale,
                ];
                self.icon(Space { origin, ..space }, sources, *size, *tint, palette)?;
            }
            Primitive::Path {
                data,
                origin,
                style,
                colors,
            } => {
                if *style == PathStyle::Erase {
                    blend = Blend::Erase;
                }
                self.path(space, data, *origin, *style, *colors)?;
            }
            Primitive::Text {
                text,
                origin,
                clip,
                ink,
            } => {
                let origin = [
                    space.origin[0] + origin[0] * space.scale,
                    space.origin[1] + origin[1] * space.scale,
                ];
                if origin.iter().any(|v| !v.is_finite()) {
                    return Err(RenderError::InvalidPrimitive);
                }
                if let Some(clip) = clip {
                    let Some(rect) = space.visible_rect(*clip)? else {
                        return Ok(());
                    };
                    scissor = intersect(bounds, space.scissor(rect));
                    if scissor[2] == 0 || scissor[3] == 0 {
                        return Ok(());
                    }
                }
                let space = Space { origin, ..space };
                text.runs(&mut |run| self.glyph_run(space, run, *ink))?;
            }
            Primitive::Rect { rect, color } => {
                self.quad(
                    space,
                    [
                        rect[0] * space.scale + space.origin[0],
                        rect[1] * space.scale + space.origin[1],
                        rect[2] * space.scale + space.origin[0],
                        rect[3] * space.scale + space.origin[1],
                    ],
                    self.white(),
                    *color,
                )?;
            }
            Primitive::RoundedRect {
                rect,
                radius,
                stroke,
                color,
            } => self.rounded_rect(space, *rect, *radius, *stroke, [*color; 2])?,
            Primitive::Gradient {
                rect,
                radius,
                colors,
            } => self.rounded_rect(space, *rect, *radius, None, *colors)?,
            Primitive::Shadow {
                rect,
                radius,
                blur,
                color,
            } => self.shadow(space, *rect, *radius, *blur, *color)?,
            Primitive::Segment {
                from,
                to,
                width,
                round,
                color,
            } => self.segment(space, *from, *to, [*width; 2], *round, *color)?,
            Primitive::Taper {
                from,
                to,
                widths,
                color,
            } => self.segment(space, *from, *to, *widths, true, *color)?,
            Primitive::Highlight {
                from,
                to,
                width,
                color,
            } => {
                blend = Blend::Multiply;
                self.segment(space, *from, *to, [*width; 2], false, *color)?;
            }
            Primitive::Image { image, rect } => {
                if let Some(rect) = space.visible_rect(*rect)? {
                    self.image(image, active_images)?;
                    blend = Blend::Image(image.id());
                    self.quad(space, rect, [0.0, 0.0, 1.0, 1.0], [1.0; 4])?;
                }
            }
        }
        let end = self.vertices.len() as u32;
        if let Some((rect, radius)) = space.round {
            let [width, height] = space.size.map(|side| side as f32);
            let half = [(rect[2] - rect[0]) / 2.0, (rect[3] - rect[1]) / 2.0];
            let middle = [rect[0] + half[0], rect[1] + half[1]];
            for vertex in &mut self.vertices[start as usize..] {
                vertex.clip_local = [
                    (vertex.position[0] + 1.0) * width / 2.0 - middle[0],
                    (1.0 - vertex.position[1]) * height / 2.0 - middle[1],
                ];
                vertex.clip = [half[0], half[1], radius.min(half[0]).min(half[1])];
            }
        }
        if start != end {
            if self.batches.len() > self.barrier
                && let Some(last) = self.batches.last_mut()
                && last.blend == blend
                && last.scissor == scissor
            {
                last.vertices.end = end;
            } else {
                self.batches.push(Batch {
                    vertices: start..end,
                    blend,
                    scissor,
                });
            }
        }
        Ok(())
    }

    fn image(&mut self, image: &RasterImage, active: &HashSet<u64>) -> Result<(), RenderError> {
        if self.images.contains_key(&image.id()) {
            return Ok(());
        }
        if image.size.iter().any(|v| *v > self.max_texture_dimension()) {
            return Err(RenderError::ImageTooLarge);
        }
        let bytes = image.pixels().len() as u64;
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
        self.images.insert(
            image.id(),
            CachedImage {
                texture: self.gpu.upload_image(image),
                bytes,
                pixels: image.pixels.downgrade(),
            },
        );
        Ok(())
    }

    fn glyph_run(
        &mut self,
        space: Space,
        run: GlyphRun<'_>,
        ink: [f32; 4],
    ) -> Result<(), RenderError> {
        let scale = space.scale;
        let origin = space.origin;
        let line_top = run.line[0] * scale + origin[1];
        let line_height = run.line[1] * scale;
        if line_top + line_height < 0.0 || line_top > space.size[1] as f32 {
            return Ok(());
        }
        let size = run.size * scale;
        if !size.is_finite() || size > self.atlas_side() as f32 {
            return Err(RenderError::AtlasFull);
        }
        let color = run
            .color
            .map_or(ink, |color| legible(color, space.backdrop, run.backdrop));
        let [red, green, blue, _] = color;
        let luminance = 0.2126 * red + 0.7152 * green + 0.0722 * blue;
        let tone = (f32::from(srgb_byte(luminance)) / 255.0 * 16.0).round() as u8;
        for glyph in run.glyphs {
            let x = glyph.x * scale + origin[0];
            let y = glyph.y * scale + origin[1];
            // Quantization affects raster coverage only, never advances or line breaks.
            let x = (x * 4.0).round() * 0.25;
            let y = (y * 4.0).round() * 0.25;
            let glyph_id = glyph
                .id
                .try_into()
                .map_err(|_| RenderError::UnsupportedGlyph)?;
            let phase = [((x - x.floor()) * 4.0) as u8, ((y - y.floor()) * 4.0) as u8];
            let key = AtlasKey::Text {
                font: run.font.id(),
                index: run.index,
                glyph: glyph_id,
                size: size.to_bits(),
                coords: run.coords.to_vec(),
                phase,
                embolden: run.embolden,
                skew: run.skew.unwrap_or(0.0).to_bits(),
                tone,
            };
            let cached = if let Some(cached) = self.glyphs.get(&key) {
                *cached
            } else {
                if self.glyphs.len() >= self.glyph_limit() {
                    return Err(RenderError::AtlasFull);
                }
                let font = FontRef::from_index(run.font.as_ref(), run.index as usize)
                    .ok_or(RenderError::UnreadableFont)?;
                let mut scaler = self
                    .scaler
                    .builder_with_id(font, [run.font.id(), u64::from(run.index)])
                    .size(size)
                    .hint(false)
                    .normalized_coords(run.coords.iter().copied())
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
                if run.embolden {
                    render.embolden(size / 24.0);
                }
                if let Some(skew) = run.skew {
                    render.transform(Some(Transform::skew(
                        Angle::from_degrees(skew),
                        Angle::from_degrees(0.0),
                    )));
                }
                let image = render.render(&mut scaler, glyph_id).map(|mut image| {
                    if image.content == Content::Mask {
                        let coverage = text_coverage(tone);
                        for alpha in &mut image.data {
                            *alpha = coverage[usize::from(*alpha)];
                        }
                    }
                    image
                });
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
                let mut left = x.floor() + glyph.left as f32;
                let mut top = y.floor() - glyph.top as f32;
                let mut width = glyph.width as f32;
                let mut height = glyph.height as f32;
                if glyph.color {
                    let ratio = (line_height / height).min(1.0);
                    left += width * (1.0 - ratio) * 0.5;
                    width *= ratio;
                    height *= ratio;
                    top = top.clamp(line_top, (line_top + line_height - height).max(line_top));
                }
                self.quad(
                    space,
                    [left, top, left + width, top + height],
                    self.uv(glyph),
                    if glyph.color { [1.0; 4] } else { color },
                )?;
            }
        }
        for decoration in run.decorations {
            let x = decoration.x * scale + origin[0];
            let y = decoration.y * scale + origin[1];
            self.quad(
                space,
                [
                    x,
                    y,
                    x + decoration.width * scale,
                    y + (decoration.thickness * scale).max(1.0),
                ],
                self.white(),
                decoration
                    .color
                    .map_or(ink, |color| legible(color, space.backdrop, run.backdrop)),
            )?;
        }
        Ok(())
    }

    fn upload(&mut self, image: swash::scale::image::Image) -> Result<AtlasGlyph, RenderError> {
        let p = image.placement;
        let side = self.atlas_side();
        if p.width + 2 > side || p.height + 2 > side {
            return Err(RenderError::AtlasFull);
        }
        if self.pen[0] + p.width + 1 > side {
            self.pen[0] = 0;
            self.pen[1] += self.row_height + 1;
            self.row_height = 0;
        }
        if self.pen[1] + p.height + 1 > side {
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
        self.gpu
            .write_atlas([cached.x, cached.y], [p.width, p.height], &rgba);
        self.pen[0] += p.width + 1;
        self.row_height = self.row_height.max(p.height);
        Ok(cached)
    }

    fn icon(
        &mut self,
        space: Space,
        sources: &'static [&'static str],
        size: f32,
        tint: [f32; 4],
        palette: &Palette,
    ) -> Result<(), RenderError> {
        // Whole device pixels keep a sprite's pixel-art edges crisp at every scale.
        let size = (size * space.scale).round().max(1.0);
        let [x, y] = space.origin.map(f32::round);
        if !size.is_finite() || [x, y].iter().any(|v| !v.is_finite()) {
            return Err(RenderError::InvalidPrimitive);
        }
        if x + size < 0.0 || y + size < 0.0 || x > space.size[0] as f32 || y > space.size[1] as f32
        {
            return Ok(());
        }
        if size + 2.0 > self.atlas_side() as f32 {
            return Err(RenderError::AtlasFull);
        }
        let size = size as u32;
        let ink = [tint[0], tint[1], tint[2]];
        let key = AtlasKey::Icon {
            sources,
            size,
            ink: ink.map(f32::to_bits),
            palette: palette.bits(),
        };
        let glyph = if let Some(Some(glyph)) = self.glyphs.get(&key) {
            *glyph
        } else {
            if self.glyphs.len() >= self.glyph_limit() {
                return Err(RenderError::AtlasFull);
            }
            let glyph = self.upload(icon::rasterize(sources, size, ink, palette))?;
            self.glyphs.insert(key, Some(glyph));
            glyph
        };
        self.quad(
            space,
            [x, y, x + glyph.width as f32, y + glyph.height as f32],
            self.uv(glyph),
            [1.0, 1.0, 1.0, tint[3]],
        )
    }

    /// `colors` are the top and bottom edges' colours.
    fn rounded_rect(
        &mut self,
        space: Space,
        rect: [f32; 4],
        radius: [f32; 2],
        stroke: Option<Stroke>,
        colors: [[f32; 4]; 2],
    ) -> Result<(), RenderError> {
        if radius
            .iter()
            .any(|value| !value.is_finite() || *value < 0.0)
        {
            return Err(RenderError::InvalidPrimitive);
        }
        let width = match stroke {
            None => 0.0,
            Some(Stroke::Solid(width) | Stroke::Dashed(width)) => {
                if !width.is_finite() || width <= 0.0 || width * space.scale == 0.0 {
                    return Err(RenderError::InvalidPrimitive);
                }
                width
            }
        };
        let rect = [
            rect[0] * space.scale + space.origin[0],
            rect[1] * space.scale + space.origin[1],
            rect[2] * space.scale + space.origin[0],
            rect[3] * space.scale + space.origin[1],
        ];
        let start = self.vertices.len();
        // A pixel of margin holds the antialiased fringe outside the edge.
        let fringe = [rect[0] - 1.0, rect[1] - 1.0, rect[2] + 1.0, rect[3] + 1.0];
        self.quad(space, fringe, self.white(), colors[0])?;
        let half = [(rect[2] - rect[0]) * 0.5, (rect[3] - rect[1]) * 0.5];
        if !(4.0 * (half[0] + half[1])).is_finite() || !colors[1].iter().all(|v| v.is_finite()) {
            return Err(RenderError::InvalidPrimitive);
        }
        let radius = if radius.contains(&0.0) {
            [0.0; 2]
        } else {
            std::array::from_fn(|axis| (radius[axis] * space.scale).min(half[axis]))
        };
        let width = (width * space.scale).min(half[0].min(half[1]));
        // The stroke sign selects its dash pattern at the shader boundary.
        let width = if matches!(stroke, Some(Stroke::Dashed(_))) {
            -width
        } else {
            width
        };
        for vertex in &mut self.vertices[start..] {
            vertex.shape = [half[0], half[1], radius[0], radius[1]];
            vertex.stroke = width;
            if vertex.local[1] > 0.0 {
                vertex.color = colors[1];
            }
        }
        Ok(())
    }

    fn shadow(
        &mut self,
        space: Space,
        rect: [f32; 4],
        radius: f32,
        blur: f32,
        color: [f32; 4],
    ) -> Result<(), RenderError> {
        if !radius.is_finite() || radius < 0.0 || !blur.is_finite() || blur <= 0.0 {
            return Err(RenderError::InvalidPrimitive);
        }
        let rect = [0, 1, 2, 3].map(|i| rect[i] * space.scale + space.origin[i % 2]);
        let sigma = blur * space.scale / 2.0;
        let reach = 3.0 * sigma;
        let start = self.vertices.len();
        self.quad(
            space,
            [
                rect[0] - reach,
                rect[1] - reach,
                rect[2] + reach,
                rect[3] + reach,
            ],
            self.white(),
            color,
        )?;
        let half = [(rect[2] - rect[0]) * 0.5, (rect[3] - rect[1]) * 0.5];
        let radius = (radius * space.scale).min(half[0]).min(half[1]).max(0.0);
        for vertex in &mut self.vertices[start..] {
            vertex.shape = [half[0], half[1], radius, radius];
            vertex.blur = sigma;
        }
        Ok(())
    }

    fn path(
        &mut self,
        space: Space,
        data: &str,
        origin: [f32; 2],
        style: PathStyle,
        colors: [[f32; 4]; 2],
    ) -> Result<(), RenderError> {
        let [x, y] = [0, 1]
            .map(|axis| ((space.origin[axis] + origin[axis] * space.scale) * 4.0).round() * 0.25);
        let (kind, width) = match style {
            PathStyle::Fill | PathStyle::Erase => (0, 0.0),
            PathStyle::Stroke(width) => (1, width),
            PathStyle::Shadow(width) => (2, width),
        };
        if [x, y]
            .iter()
            .chain(colors.as_flattened())
            .any(|v| !v.is_finite())
            || !width.is_finite()
            || width < 0.0
            || (width == 0.0 && kind != 0)
        {
            return Err(RenderError::InvalidPrimitive);
        }
        let phase = [((x - x.floor()) * 4.0) as u8, ((y - y.floor()) * 4.0) as u8];
        let key = AtlasKey::Path {
            data: data.to_owned(),
            scale: space.scale.to_bits(),
            style: (kind, width.to_bits()),
            phase,
        };
        let glyph = match self.glyphs.get(&key) {
            Some(glyph) => *glyph,
            None => {
                if self.glyphs.len() >= self.glyph_limit() {
                    return Err(RenderError::AtlasFull);
                }
                let mut mask = Mask::new(data);
                mask.transform(Some(Transform::scale(space.scale, space.scale)))
                    .offset(phase.map(|quarter| f32::from(quarter) * 0.25));
                if let PathStyle::Stroke(width) = style {
                    mask.style(swash::zeno::Stroke {
                        start_cap: Cap::Round,
                        end_cap: Cap::Round,
                        join: Join::Round,
                        ..swash::zeno::Stroke::new(width)
                    });
                }
                let (mut pixels, mut placement) = mask.render();
                if let PathStyle::Shadow(blur) = style {
                    (pixels, placement) = soften(&pixels, placement, blur * space.scale);
                }
                let glyph = (placement.width > 0 && placement.height > 0)
                    .then(|| {
                        self.upload(swash::scale::image::Image {
                            content: Content::Mask,
                            placement,
                            data: pixels,
                            ..Default::default()
                        })
                    })
                    .transpose()?;
                self.glyphs.insert(key, glyph);
                glyph
            }
        };
        let Some(glyph) = glyph else {
            return Ok(());
        };
        let left = x.floor() + glyph.left as f32;
        let top = y.floor() + glyph.top as f32;
        let start = self.vertices.len();
        self.quad(
            space,
            [
                left,
                top,
                left + glyph.width as f32,
                top + glyph.height as f32,
            ],
            self.uv(glyph),
            colors[0],
        )?;
        for vertex in &mut self.vertices[start..] {
            if vertex.local[1] > 0.0 {
                vertex.color = colors[1];
            }
        }
        Ok(())
    }

    /// Draws the segment as a capsule in its own frame, reusing the rounded-rectangle distance;
    /// ends of two widths make it a tapered capsule, which the shader marks by a negative blur.
    fn segment(
        &mut self,
        space: Space,
        from: [f32; 2],
        to: [f32; 2],
        widths: [f32; 2],
        round: bool,
        color: [f32; 4],
    ) -> Result<(), RenderError> {
        let pixel = |p: [f32; 2]| {
            [
                p[0] * space.scale + space.origin[0],
                p[1] * space.scale + space.origin[1],
            ]
        };
        let [from, to] = [pixel(from), pixel(to)];
        // Hairlines stay one device pixel wide.
        let radii = widths.map(|width| (width * space.scale).max(1.0) * 0.5);
        let radius = radii[0].max(radii[1]);
        if from
            .iter()
            .chain(&to)
            .chain(&color)
            .chain(&radii)
            .any(|v| !v.is_finite())
        {
            return Err(RenderError::InvalidPrimitive);
        }
        let pad = radius + 1.0;
        if from[0].max(to[0]) + pad < 0.0
            || from[1].max(to[1]) + pad < 0.0
            || from[0].min(to[0]) - pad > space.size[0] as f32
            || from[1].min(to[1]) - pad > space.size[1] as f32
        {
            return Ok(());
        }
        if self.vertices.len() + 6 > MAX_VERTICES {
            return Err(RenderError::FrameTooLarge);
        }
        let delta = [to[0] - from[0], to[1] - from[1]];
        let length = delta[0].hypot(delta[1]);
        let along = if length > 0.0 {
            [delta[0] / length, delta[1] / length]
        } else {
            [1.0, 0.0]
        };
        let across = [-along[1], along[0]];
        let center = [(from[0] + to[0]) * 0.5, (from[1] + to[1]) * 0.5];
        let half = [length * 0.5 + radius, radius];
        let corner = if round { radius } else { 0.0 };
        let tapered = radii[0] != radii[1];
        let (shape, blur) = if tapered {
            ([length * 0.5, radii[0], radii[1], 0.0], -1.0)
        } else {
            ([half[0], half[1], corner, corner], 0.0)
        };
        let [hx, hy] = [half[0] + 1.0, half[1] + 1.0];
        for local in [
            [-hx, -hy],
            [-hx, hy],
            [hx, hy],
            [-hx, -hy],
            [hx, hy],
            [hx, -hy],
        ] {
            let x = center[0] + along[0] * local[0] + across[0] * local[1];
            let y = center[1] + along[1] * local[0] + across[1] * local[1];
            self.vertices.push(Vertex {
                position: [
                    x * 2.0 / space.size[0] as f32 - 1.0,
                    1.0 - y * 2.0 / space.size[1] as f32,
                ],
                uv: [0.5 / self.atlas_side() as f32; 2],
                color,
                local,
                shape,
                stroke: 0.0,
                clip_local: [0.0; 2],
                clip: [0.0; 3],
                blur,
            });
        }
        Ok(())
    }

    fn quad(
        &mut self,
        space: Space,
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
            || rect[0] > space.size[0] as f32
            || rect[1] > space.size[1] as f32
        {
            return Ok(());
        }
        if self.vertices.len() + 6 > MAX_VERTICES {
            return Err(RenderError::FrameTooLarge);
        }
        let x0 = rect[0] * 2.0 / space.size[0] as f32 - 1.0;
        let x1 = rect[2] * 2.0 / space.size[0] as f32 - 1.0;
        let y0 = 1.0 - rect[1] * 2.0 / space.size[1] as f32;
        let y1 = 1.0 - rect[3] * 2.0 / space.size[1] as f32;
        if [x0, x1, y0, y1].iter().any(|v| !v.is_finite()) {
            return Err(RenderError::InvalidPrimitive);
        }
        let [hx, hy] = [(rect[2] - rect[0]) * 0.5, (rect[3] - rect[1]) * 0.5];
        for (position, uv, local) in [
            ([x0, y0], [uv[0], uv[1]], [-hx, -hy]),
            ([x0, y1], [uv[0], uv[3]], [-hx, hy]),
            ([x1, y1], [uv[2], uv[3]], [hx, hy]),
            ([x0, y0], [uv[0], uv[1]], [-hx, -hy]),
            ([x1, y1], [uv[2], uv[3]], [hx, hy]),
            ([x1, y0], [uv[2], uv[1]], [hx, -hy]),
        ] {
            self.vertices.push(Vertex {
                position,
                uv,
                color,
                local,
                shape: [0.0; 4],
                stroke: 0.0,
                clip_local: [0.0; 2],
                clip: [0.0; 3],
                blur: 0.0,
            });
        }
        Ok(())
    }
}

/// Blurs a coverage mask about `radius` device pixels with three box blurs on each axis,
/// growing it to hold the spread.
fn soften(
    pixels: &[u8],
    placement: swash::zeno::Placement,
    radius: f32,
) -> (Vec<u8>, swash::zeno::Placement) {
    let reach = (radius / 2.0).ceil().max(1.0) as usize;
    let pad = 3 * reach;
    let [width, height] = [placement.width as usize, placement.height as usize];
    let [wide, tall] = [width + 2 * pad, height + 2 * pad];
    let mut values = vec![0.0_f32; wide * tall];
    for row in 0..height {
        for column in 0..width {
            values[(row + pad) * wide + column + pad] = f32::from(pixels[row * width + column]);
        }
    }
    let window = (2 * reach + 1) as f32;
    let mut line = Vec::new();
    for _ in 0..3 {
        // Rows, then columns.
        for (count, step, lines, stride) in [(wide, 1, tall, wide), (tall, wide, wide, 1)] {
            for index in 0..lines {
                let at = |position: usize| index * stride + position * step;
                line.clear();
                line.extend((0..count).map(|position| values[at(position)]));
                let mut sum: f32 = line[..reach.min(count)].iter().sum();
                for position in 0..count {
                    if position + reach < count {
                        sum += line[position + reach];
                    }
                    values[at(position)] = sum / window;
                    if position >= reach {
                        sum -= line[position - reach];
                    }
                }
            }
        }
    }
    (
        values
            .into_iter()
            .map(|value| value.round().clamp(0.0, 255.0) as u8)
            .collect(),
        swash::zeno::Placement {
            left: placement.left - pad as i32,
            top: placement.top - pad as i32,
            width: wide as u32,
            height: tall as u32,
        },
    )
}

fn srgb_byte(value: f32) -> u8 {
    let srgb = if value <= 0.0031308 {
        value * 12.92
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    };
    (srgb * 255.0).round() as u8
}

/// Coverage for text of sRGB luminance `tone` sixteenths that, blended in linear light,
/// darkens a white backdrop as the rasterized coverage would blended sRGB-encoded, as
/// systems blend text: dark text keeps its weight instead of thinning, light text is
/// nearly untouched.
fn text_coverage(tone: u8) -> [u8; 256] {
    let encoded = f32::from(tone) / 16.0;
    let ink = linear(encoded);
    std::array::from_fn(|coverage| {
        let coverage = coverage as f32 / 255.0;
        let weighted = if ink < 0.999 {
            (1.0 - linear(1.0 - coverage * (1.0 - encoded))) / (1.0 - ink)
        } else {
            coverage
        };
        (weighted * 255.0).round() as u8
    })
}

fn linear(value: f32) -> f32 {
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

/// Least OKLab lightness difference between text and the backdrop it stays legible on.
const LEGIBLE: f32 = 0.4;

/// On a dark layer `backdrop`, `color` lifted clear of the lightness of what lies `behind`
/// it, or of the backdrop, keeping hue and chroma.
fn legible(color: [f32; 4], backdrop: Option<[f32; 4]>, behind: Option<[f32; 4]>) -> [f32; 4] {
    let Some(backdrop) = backdrop.filter(|backdrop| oklab(*backdrop)[0] < 0.5) else {
        return color;
    };
    let [under, ..] = oklab(behind.unwrap_or(backdrop));
    let [lightness, a, b] = oklab(color);
    if under >= 0.5 || lightness >= under + LEGIBLE {
        return color;
    }
    let [red, green, blue] = from_oklab([under + LEGIBLE, a, b]);
    [red, green, blue, color[3]]
}

/// OKLab lightness and opponent axes of a linear RGB colour.
pub fn oklab([red, green, blue, _]: [f32; 4]) -> [f32; 3] {
    let [l, m, s] = [
        [0.412_221_46, 0.536_332_55, 0.051_445_995],
        [0.211_903_5, 0.680_699_5, 0.107_396_96],
        [0.088_302_46, 0.281_718_85, 0.629_978_7],
    ]
    .map(|[r, g, b]| (r * red + g * green + b * blue).cbrt());
    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

/// Linear RGB of an OKLab colour, clipped to the sRGB gamut.
pub fn from_oklab([lightness, a, b]: [f32; 3]) -> [f32; 3] {
    let [l, m, s] = [
        lightness + 0.396_337_78 * a + 0.215_803_76 * b,
        lightness - 0.105_561_346 * a - 0.063_854_17 * b,
        lightness - 0.089_484_18 * a - 1.291_485_5 * b,
    ]
    .map(|value| value.powi(3));
    [
        [4.076_741_7, -3.307_711_6, 0.230_969_94],
        [-1.268_438, 2.609_757_4, -0.341_319_38],
        [-0.004_196_086_3, -0.703_418_6, 1.707_614_7],
    ]
    .map(|[x, y, z]| (x * l + y * m + z * s).clamp(0.0, 1.0))
}

/// Linear RGBA of an opaque sRGB colour.
pub fn srgb(red: u8, green: u8, blue: u8) -> [f32; 4] {
    let [red, green, blue] = [red, green, blue].map(|byte| linear(f32::from(byte) / 255.0));
    [red, green, blue, 1.0]
}

/// The sRGB bytes of linear RGBA `color`, as `srgb` takes them.
pub fn srgb_bytes([red, green, blue, _]: [f32; 4]) -> [u8; 3] {
    [red, green, blue].map(srgb_byte)
}

/// The hue in degrees of a linear colour, as it looks in sRGB.
pub fn hue(color: [f32; 4]) -> f32 {
    let [r, g, b] = [color[0], color[1], color[2]].map(|value| {
        if value <= 0.003_130_8 {
            value * 12.92
        } else {
            1.055 * value.powf(1.0 / 2.4) - 0.055
        }
    });
    let max = r.max(g).max(b);
    let range = max - r.min(g).min(b);
    if range == 0.0 {
        return 0.0;
    }
    let sector = if max == r {
        (g - b) / range
    } else if max == g {
        (b - r) / range + 2.0
    } else {
        (r - g) / range + 4.0
    };
    (sector * 60.0).rem_euclid(360.0)
}

/// The light theme's accent `[saturation, lightness]`, also the default ink of a section's pen.
pub const LIGHT_ACCENT: [f32; 2] = [0.60, 0.45];

/// An sRGB hue, saturation and lightness as linear RGBA.
pub fn hsl(hue: f32, saturation: f32, lightness: f32) -> [f32; 4] {
    let chroma = (1.0 - (2.0 * lightness - 1.0).abs()) * saturation;
    let channel = |offset: f32| {
        let k = (offset + hue / 30.0).rem_euclid(12.0);
        let value = lightness - chroma / 2.0 * (k - 3.0).min(9.0 - k).clamp(-1.0, 1.0);
        (value * 255.0).round().clamp(0.0, 255.0) as u8
    };
    srgb(channel(0.0), channel(8.0), channel(4.0))
}

#[cfg(all(test, feature = "wgpu"))]
mod tests {
    use super::*;
    use parley::{
        FontContext, FontFamily, FontFamilyName, GenericFamily, Layout, LayoutContext,
        PositionedLayoutItem, StyleProperty,
    };
    use std::time::Duration;

    const CHECKBOX: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><defs><linearGradient id="a" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#ffffff"/><stop offset="1" stop-color="#7090b0"/></linearGradient></defs><path fill="url(#a)" d="M2 2h12v12H2zM4 4v8h8V4z"/></svg>"##;
    const MARK: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><defs><linearGradient id="b" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#20c040"/><stop offset="1" stop-color="#107020"/></linearGradient></defs><path fill="url(#b)" d="M4 8l3 3 5-7-1-1-4 5-2-2z"/></svg>"##;
    const DOT: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><defs><linearGradient id="c" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#ffd000"/><stop offset="1" stop-color="#c08000"/></linearGradient></defs><path fill="url(#c)" d="M8 2a6 6 0 1 0 0.01 0z"/></svg>"##;

    struct Text(Layout<[u8; 4]>);

    impl Glyphs for Text {
        fn runs(
            &self,
            paint: &mut dyn FnMut(GlyphRun<'_>) -> Result<(), RenderError>,
        ) -> Result<(), RenderError> {
            for line in self.0.lines() {
                let metrics = line.metrics();
                for item in line.items() {
                    if let PositionedLayoutItem::GlyphRun(run) = item {
                        paint_parley_run(
                            &run,
                            "",
                            metrics.baseline,
                            0.0,
                            [metrics.block_min_coord, metrics.line_height],
                            |_| None,
                            None,
                            paint,
                        )?;
                    }
                }
            }
            Ok(())
        }
    }

    fn shape(text: &str, family: &str, size: f32, underline: bool, width: f32) -> Text {
        let mut fonts = FontContext::default();
        let mut context = LayoutContext::default();
        let mut builder = context.ranged_builder(&mut fonts, text, 1.0, false);
        builder.push_default(StyleProperty::FontFamily(FontFamily::List(
            vec![
                FontFamilyName::named(family),
                GenericFamily::SansSerif.into(),
            ]
            .into(),
        )));
        builder.push_default(StyleProperty::FontSize(size));
        builder.push_default(StyleProperty::Underline(underline));
        let mut layout = builder.build(text);
        layout.break_all_lines(Some(width));
        Text(layout)
    }

    #[test]
    fn text_is_lifted_against_what_lies_behind_it_on_a_dark_layer() {
        let [black, dark_red] = [[0.0, 0.0, 0.0, 1.0], [0.2, 0.0, 0.0, 1.0]];
        let [dark, yellow, navy] = [srgb(0x1f, 0x20, 0x22), srgb(255, 255, 0), srgb(0, 0, 128)];
        let lightness = |color| oklab(color)[0];
        assert_eq!(legible(black, None, Some(navy)), black, "an unset layer");
        assert_eq!(
            legible(black, Some([1.0; 4]), Some(navy)),
            black,
            "a light layer keeps its colours, as OneNote's light page does"
        );
        assert!(lightness(legible(black, Some(dark), None)) >= lightness(dark) + LEGIBLE - 1e-3);
        assert_eq!(
            legible(black, Some(dark), Some(yellow)),
            black,
            "a light highlight on a dark layer"
        );
        assert_eq!(legible(dark_red, Some(dark), Some(yellow)), dark_red);
        let lifted = legible(black, Some(dark), Some(navy));
        assert!(lightness(lifted) >= lightness(navy) + LEGIBLE - 1e-3);
    }

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
        let decoded = RasterImage::decode(&encoded, [2, 1]).unwrap();
        assert_eq!(decoded.size, [2, 1]);
        assert_eq!(decoded.pixels(), [255, 64, 0, 255, 0, 93, 188, 128]);
        for end in 0..encoded.len() - 12 {
            assert!(
                RasterImage::decode(&encoded[..end], [2, 1]).is_err(),
                "{end}"
            );
        }

        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut jpeg)
            .encode(&[255; 4 * 4 * 3], 4, 4, image::ExtendedColorType::Rgb8)
            .unwrap();
        let decoded = RasterImage::decode(&jpeg, [4, 4]).unwrap();
        assert_eq!(decoded.size, [4, 4]);
        assert_eq!(decoded.pixels(), [255; 4 * 4 * 4]);

        let mut gif = Vec::new();
        image::codecs::gif::GifEncoder::new(&mut gif)
            .encode(&[255; 2 * 2 * 4], 2, 2, image::ExtendedColorType::Rgba8)
            .unwrap();
        let decoded = RasterImage::decode(&gif, [2, 2]).unwrap();
        assert_eq!(decoded.size, [2, 2]);
        assert_eq!(decoded.pixels(), [255; 2 * 2 * 4]);

        for size in [[8193, 8192], [16_385, 1]] {
            let mut header = Vec::new();
            let mut encoder = png::Encoder::new(&mut header, size[0], size[1]);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer.write_chunk(png::chunk::IDAT, &[]).unwrap();
            drop(writer);
            let error = RasterImage::measure(&header).err().unwrap();
            if size[0] == 8193 {
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
    fn decoding_shrinks_to_the_size_shown_without_bleeding_transparent_colour() {
        let mut encoded = Vec::new();
        let mut encoder = png::Encoder::new(&mut encoded, 4, 2);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let row = [
            [255, 0, 0, 255],
            [255, 0, 0, 255],
            [0, 255, 0, 0],
            [0, 255, 0, 0],
        ];
        encoder
            .write_header()
            .unwrap()
            .write_image_data(&[row, row].as_flattened().concat())
            .unwrap();
        assert_eq!(RasterImage::measure(&encoded).unwrap(), [4, 2]);
        let shown = RasterImage::decode(&encoded, [2, 1]).unwrap();
        assert_eq!(shown.size(), [2, 1]);
        let [left, right] = [&shown.pixels()[..4], &shown.pixels()[4..]];
        assert!(left[0] > 200 && left[3] > 200, "{left:?}");
        assert!(right[1] == 0 && right[0] <= right[3], "{right:?}");
        assert_eq!(
            RasterImage::decode(&encoded, [8, 8]).unwrap().size(),
            [4, 2]
        );
        assert_eq!(
            RasterImage::decode(&encoded, [0, 0]).unwrap().size(),
            [1, 1]
        );
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
        assert_eq!(image.id(), image.clone().id());
    }

    struct Target {
        texture: wgpu::Texture,
        readback: wgpu::Buffer,
    }

    impl Target {
        fn new(device: &wgpu::Device) -> Self {
            Self::with_format(device, wgpu::TextureFormat::Rgba8UnormSrgb)
        }

        fn with_format(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
            Self {
                texture: device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("Draw readback test"),
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
                }),
                readback: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("Draw readback"),
                    size: 512 * 256 * 4,
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
            }
        }

        fn draw(&self, renderer: &mut Renderer, layers: &[Layer<'_>]) -> Result<(), RenderError> {
            renderer.draw(
                &self.texture.create_view(&Default::default()),
                [512, 256],
                [1.0; 4],
                layers,
            )
        }

        fn capture(&self, renderer: &Renderer) -> Vec<u8> {
            let mut encoder = renderer
                .device()
                .create_command_encoder(&Default::default());
            encoder.copy_texture_to_buffer(
                self.texture.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &self.readback,
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
            renderer.queue().submit([encoder.finish()]);
            let (sender, receiver) = std::sync::mpsc::channel();
            self.readback
                .map_async(wgpu::MapMode::Read, .., move |result| {
                    sender.send(result).unwrap();
                });
            renderer
                .device()
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: Some(Duration::from_secs(5)),
                })
                .unwrap();
            receiver
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .unwrap();
            let capture = self.readback.get_mapped_range(..).unwrap().to_vec();
            self.readback.unmap();
            capture
        }
    }

    fn page<'a>(primitives: &'a [Primitive<'a>]) -> [Layer<'a>; 1] {
        [Layer {
            scale: 2.0,
            origin: [24.0; 2],
            clip: None,
            backdrop: None,
            round: None,
            motion: None,
            primitives,
        }]
    }

    #[test]
    #[ignore = "requires a native GPU adapter"]
    fn layers_sharing_a_motion_fade_as_one_inside_their_rounded_clip() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
        let (device, queue) =
            pollster::block_on(adapter.request_device(&Default::default())).unwrap();
        let target = Target::new(&device);
        let mut renderer = Renderer::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb);
        let rect = |rect, color| [Primitive::Rect { rect, color }];
        let [blue, red, black] = [
            rect([0.0, 0.0, 100.0, 100.0], [0.0, 0.0, 1.0, 1.0]),
            rect([50.0, 0.0, 150.0, 100.0], [1.0, 0.0, 0.0, 1.0]),
            rect([200.0, 0.0, 300.0, 100.0], [0.0, 0.0, 0.0, 1.0]),
        ];
        let half = Motion {
            opacity: 0.5,
            tilt: 0.0,
            pivot: [0.0; 2],
        };
        let layer = |primitives, motion, round| Layer {
            scale: 1.0,
            origin: [0.0; 2],
            clip: None,
            backdrop: None,
            round,
            motion,
            primitives,
        };
        target
            .draw(
                &mut renderer,
                &[
                    layer(&blue, Some(half), None),
                    layer(&red, Some(half), None),
                    layer(&black, None, Some(([200.0, 0.0, 300.0, 100.0], 30.0))),
                ],
            )
            .unwrap();
        let capture = target.capture(&renderer);
        let pixel = |x: usize, y: usize| &capture[(y * 512 + x) * 4..(y * 512 + x) * 4 + 4];
        let [overlap, alone] = [pixel(75, 50), pixel(25, 50)];
        assert!(
            overlap[0] == 255 && overlap[1] == overlap[2] && overlap[1] < 255,
            "the red covers the blue before both fade: {overlap:?}"
        );
        assert!(
            alone[2] == 255 && alone[0] == alone[1] && alone[0] < 255,
            "{alone:?}"
        );
        assert_eq!(pixel(202, 2), [255; 4], "outside the rounded corner");
        assert_eq!(pixel(250, 50), [0, 0, 0, 255]);
    }

    #[test]
    #[ignore = "requires a native GPU adapter"]
    fn eviction_and_repaint_reproduce_pixels() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
        let (device, queue) =
            pollster::block_on(adapter.request_device(&Default::default())).unwrap();
        let format = wgpu::TextureFormat::Rgba8UnormSrgb;
        let target = Target::new(&device);
        let mut renderer = Renderer::new(device, queue, format);
        let layout = shape("Hello 🌳\nCafé e\u{301} שלום", "Arial", 11.0, true, 220.0);
        let mut captures = Vec::new();
        let image = RasterImage::new(
            [2, 2],
            vec![255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 0, 0, 0, 0],
        )
        .unwrap();
        let transparent_edge =
            RasterImage::new([2, 1], vec![255, 0, 0, 255, 0, 0, 255, 0]).unwrap();
        let icons: [&'static [&'static str]; 4] = [&[CHECKBOX], &[CHECKBOX, MARK], &[DOT], &[MARK]];
        let phase_layout = shape("H", "Arial", 11.0, false, 20.0);
        let mut primitives = vec![
            Primitive::Rect {
                rect: [0.0, 0.0, 70.0, 14.0],
                color: [0.55, 0.73, 1.0, 0.5],
            },
            Primitive::Text {
                clip: None,
                text: &layout,
                ink: [0.0, 0.0, 0.0, 1.0],
                origin: [0.0; 2],
            },
            Primitive::Text {
                clip: None,
                text: &layout,
                ink: [0.0, 0.0, 0.0, 1.0],
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
            Primitive::RoundedRect {
                rect: [128.0, 48.0, 148.0, 68.0],
                radius: [5.0, 10.0],
                stroke: None,
                color: [1.0, 0.0, 0.0, 1.0],
            },
            Primitive::RoundedRect {
                rect: [164.0, 48.0, 184.0, 68.0],
                radius: [5.0; 2],
                stroke: Some(Stroke::Solid(1.0)),
                color: [0.0, 0.0, 1.0, 1.0],
            },
            Primitive::RoundedRect {
                rect: [200.0, 48.0, 240.0, 68.0],
                radius: [3.0, 10.0],
                stroke: Some(Stroke::Dashed(1.0)),
                color: [0.0, 0.0, 1.0, 1.0],
            },
            Primitive::Segment {
                from: [226.0, 104.0],
                to: [240.0, 104.0],
                width: 4.0,
                round: true,
                color: [1.0, 0.0, 0.0, 1.0],
            },
        ];
        for (index, sources) in icons.iter().enumerate() {
            primitives.push(Primitive::Icon {
                sources,
                origin: [128.125 + 20.0 * index as f32, 80.25],
                size: 12.0,
                tint: [1.0; 4],
                palette: Palette::default(),
            });
        }
        primitives.extend((0..9).map(|step| Primitive::Text {
            clip: None,
            text: &phase_layout,
            ink: [0.0, 0.0, 0.0, 1.0],
            origin: [8.0 + 24.0 * step as f32, 96.0 + 0.125 * step as f32],
        }));
        for pass in 0..6 {
            if pass == 2 {
                renderer.clear_glyph_cache();
                renderer.images.clear();
            } else if pass == 3 {
                renderer =
                    Renderer::new(renderer.device().clone(), renderer.queue().clone(), format);
            }
            let clipping = [
                Primitive::Text {
                    text: &layout,
                    ink: [0.0, 0.0, 0.0, 1.0],
                    origin: [0.0; 2],
                    clip: (pass == 5).then_some([2.5, 1.25, 30.75, 20.5]),
                },
                Primitive::Rect {
                    rect: [40.0, 40.0, 70.0, 50.0],
                    color: [0.0, 0.0, 1.0, 1.0],
                },
            ];
            target
                .draw(
                    &mut renderer,
                    &page(if pass < 4 { &primitives } else { &clipping }),
                )
                .unwrap();
            captures.push(target.capture(&renderer));
        }
        let centroids: Vec<_> = (0..9)
            .map(|step| {
                let mut weight = 0.0_f64;
                let mut moment = 0.0_f64;
                for y in 212..252 {
                    for x in (40 + 48 * step)..(72 + 48 * step) {
                        // Black text over white leaves the rasterized coverage sRGB-encoded.
                        let coverage = 1.0 - f64::from(captures[0][(y * 512 + x) * 4]) / 255.0;
                        weight += coverage;
                        moment += coverage * y as f64;
                    }
                }
                assert!(weight > 10.0, "phase sample {step} was not painted");
                moment / weight
            })
            .collect();
        let pixel = |x: usize, y: usize| &captures[0][(y * 512 + x) * 4..(y * 512 + x) * 4 + 4];
        assert_eq!(pixel(490, 232), [255, 0, 0, 255]);
        assert_eq!(pixel(473, 232), [255, 0, 0, 255]);
        assert_eq!(pixel(490, 238), [255; 4]);
        assert_eq!(pixel(472, 228), [255; 4]);
        assert_eq!(pixel(300, 140), [255, 0, 0, 255]);
        assert_eq!(pixel(280, 120), [255; 4]);
        assert_eq!(pixel(282, 124), [255; 4]);
        assert_eq!(pixel(281, 140), [255, 0, 0, 255]);
        assert_eq!(pixel(372, 140), [255; 4]);
        assert_eq!(pixel(372, 120), [0, 0, 255, 255]);
        assert_eq!(pixel(352, 120), [255; 4]);
        assert_eq!(pixel(424, 120), [255; 4]);
        assert_eq!(pixel(464, 140), [255; 4]);
        assert!((440..480).any(|x| pixel(x, 120)[0] < 40));
        assert!((440..480).any(|x| pixel(x, 120)[0] > 250));
        for x in 440..472 {
            assert!(pixel(x, 120)[0].abs_diff(pixel(x + 8, 120)[0]) <= 1);
        }
        for (step, centroid) in centroids.iter().enumerate() {
            let movement = centroid - centroids[0];
            assert!(
                (movement - step as f64 * 0.25).abs() < 0.06,
                "quarter-pixel step {step} moved the glyph by {movement} px"
            );
        }
        assert!(!renderer.glyphs.is_empty());
        assert!(renderer.glyphs.len() <= renderer.glyph_limit());
        assert_eq!(
            renderer
                .glyphs
                .keys()
                .filter(|key| matches!(key, AtlasKey::Icon { .. }))
                .count(),
            4
        );
        for x in [280, 320, 360, 400] {
            let colored = (184..210)
                .flat_map(|y| (x..x + 26).map(move |x| (y * 512 + x) * 4))
                .filter(|offset| captures[0][*offset..*offset + 3].iter().any(|v| *v < 180))
                .count();
            assert!(colored > 10, "icon at {x} was not painted");
        }
        let count = renderer.glyphs.len();
        target
            .draw(
                &mut renderer,
                &page(&[Primitive::Icon {
                    sources: icons[0],
                    origin: [10000.0; 2],
                    size: 12.0,
                    tint: [1.0; 4],
                    palette: Palette::default(),
                }]),
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
        assert_ne!(captures[4], captures[5]);
        for y in 0..256 {
            for x in 0..512 {
                let offset = (y * 512 + x) * 4;
                let expected = if ((29..86).contains(&x) && (26..65).contains(&y))
                    || ((104..164).contains(&x) && (104..124).contains(&y))
                {
                    &captures[4][offset..offset + 4]
                } else {
                    &[255; 4]
                };
                assert_eq!(
                    &captures[5][offset..offset + 4],
                    expected,
                    "clipping at {x},{y}"
                );
            }
        }
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
            Primitive::RoundedRect {
                rect: [-1e38, 0.0, 1e38, 1e38],
                radius: [3.0; 2],
                stroke: Some(Stroke::Dashed(1.0)),
                color: [1.0; 4],
            },
            Primitive::RoundedRect {
                rect: [0.0, 0.0, 10.0, 10.0],
                radius: [f32::NAN, 2.0],
                stroke: None,
                color: [1.0; 4],
            },
            Primitive::RoundedRect {
                rect: [0.0, 0.0, 10.0, 10.0],
                radius: [2.0; 2],
                stroke: Some(Stroke::Solid(0.0)),
                color: [1.0; 4],
            },
            Primitive::RoundedRect {
                rect: [0.0, 0.0, 10.0, 10.0],
                radius: [2.0; 2],
                stroke: Some(Stroke::Dashed(f32::NAN)),
                color: [1.0; 4],
            },
            Primitive::Rect {
                rect: [0.0, 0.0, f32::NAN, 1.0],
                color: [1.0; 4],
            },
            Primitive::Rect {
                rect: [2.0, 0.0, 1.0, 1.0],
                color: [1.0; 4],
            },
            Primitive::Text {
                clip: None,
                text: &layout,
                ink: [0.0, 0.0, 0.0, 1.0],
                origin: [f32::MAX; 2],
            },
            Primitive::Text {
                clip: Some([0.0, 0.0, f32::NAN, 1.0]),
                text: &layout,
                ink: [0.0, 0.0, 0.0, 1.0],
                origin: [0.0; 2],
            },
            Primitive::Text {
                clip: Some([2.0, 0.0, 1.0, 1.0]),
                text: &layout,
                ink: [0.0, 0.0, 0.0, 1.0],
                origin: [0.0; 2],
            },
        ] {
            assert!(matches!(
                target.draw(&mut renderer, &page(&[primitive])),
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
            target.draw(&mut renderer, &page(&primitives)),
            Err(RenderError::ImageBudget)
        ));
        let primitives: Vec<_> = too_many
            .iter()
            .map(|image| Primitive::Image {
                image,
                rect: [-20.0, -20.0, -19.0, -19.0],
            })
            .collect();
        target.draw(&mut renderer, &page(&primitives)).unwrap();
        assert_eq!(renderer.images.len(), 2);
        let mut seen = HashSet::from([image.id(), transparent_edge.id()]);
        let mut previous = None;
        for _ in 0..5 {
            let large = RasterImage::new([2048; 2], vec![255; 2048 * 2048 * 4]).unwrap();
            seen.insert(large.id());
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
            target.draw(&mut renderer, &page(&primitives)).unwrap();
            assert!(renderer.images.contains_key(&large.id()));
            if let Some(image) = &previous {
                assert!(renderer.images.contains_key(&image.id()));
            }
            assert!(renderer.occupancy().within_budget);
            previous = Some(large);
        }
        assert!(renderer.images.len() <= MAX_IMAGES);
        assert!(seen.iter().any(|id| !renderer.images.contains_key(id)));
        let last = previous.take().unwrap().id();
        target.draw(&mut renderer, &page(&[])).unwrap();
        assert!(
            !renderer.images.contains_key(&last),
            "a texture outlived its image"
        );
        renderer.images.clear();
        target
            .draw(
                &mut renderer,
                &page(&[Primitive::Image {
                    image: &image,
                    rect: [128.0, 8.0, 160.0, 40.0],
                }]),
            )
            .unwrap();
        assert_eq!(renderer.images.len(), 1);
        assert!(renderer.images.contains_key(&image.id()));
    }

    #[test]
    #[ignore = "requires a native GPU adapter"]
    fn paths_fill_and_stroke_in_layer_units() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
        let (device, queue) =
            pollster::block_on(adapter.request_device(&Default::default())).unwrap();
        let target = Target::new(&device);
        let mut renderer = Renderer::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb);
        let primitives = [
            Primitive::Path {
                data: "M0 0H20L30 20H0Z",
                origin: [10.0, 10.0],
                style: PathStyle::Fill,
                colors: [[0.0, 0.0, 1.0, 1.0]; 2],
            },
            Primitive::Path {
                data: "M0 0H40",
                origin: [10.0, 60.0],
                style: PathStyle::Stroke(4.0),
                colors: [[1.0, 0.0, 0.0, 1.0]; 2],
            },
            Primitive::Path {
                data: "M0 0H20V20H0Z",
                origin: [150.0, 20.0],
                style: PathStyle::Shadow(6.0),
                colors: [[0.0, 0.0, 0.0, 1.0]; 2],
            },
            Primitive::Shadow {
                rect: [170.0, 90.0, 190.0, 110.0],
                radius: 4.0,
                blur: 6.0,
                color: [0.0, 0.0, 0.0, 1.0],
            },
            Primitive::Gradient {
                rect: [100.0, 10.0, 120.0, 110.0],
                radius: [0.0; 2],
                colors: [[0.0, 0.0, 1.0, 1.0], [0.0, 1.0, 0.0, 1.0]],
            },
        ];
        target
            .draw(
                &mut renderer,
                &[Layer {
                    scale: 2.0,
                    origin: [0.0; 2],
                    clip: None,
                    backdrop: None,
                    round: None,
                    motion: None,
                    primitives: &primitives,
                }],
            )
            .unwrap();
        let capture = target.capture(&renderer);
        let pixel = |x: usize, y: usize| &capture[(y * 512 + x) * 4..(y * 512 + x) * 4 + 4];
        assert_eq!(pixel(40, 40), [0, 0, 255, 255]);
        assert_eq!(pixel(70, 55), [0, 0, 255, 255], "inside the slanted edge");
        assert_eq!(pixel(75, 30), [255; 4], "outside the slanted edge");
        let edge = pixel(65, 30);
        assert!(
            edge[0] > 0 && edge[0] < 255,
            "the slant is antialiased: {edge:?}"
        );
        assert_eq!(pixel(60, 120), [255, 0, 0, 255]);
        assert_eq!(pixel(60, 112), [255; 4], "strokes are their width across");
        assert_eq!(pixel(17, 120)[1], 0, "round caps reach past the ends");
        assert_eq!(pixel(12, 120), [255; 4]);
        let [top, bottom] = [pixel(220, 21), pixel(220, 218)];
        assert!(
            top[2] > 250 && top[1] < 60,
            "gradients start at the top colour: {top:?}"
        );
        assert!(
            bottom[1] > 250 && bottom[2] < 60,
            "and end at the bottom's: {bottom:?}"
        );
        let [inside, fading, beyond] = [pixel(320, 60), pixel(298, 60), pixel(270, 60)];
        assert!(inside[0] < 40, "a shadow is dark inside: {inside:?}");
        assert!(
            fading[0] > 40 && fading[0] < 230,
            "and fades at its edge: {fading:?}"
        );
        assert_eq!(beyond, [255; 4]);
        let [inside, fading, beyond] = [pixel(360, 200), pixel(340, 200), pixel(310, 200)];
        assert!(
            inside[0] < 40,
            "a rectangle's shadow is dark inside: {inside:?}"
        );
        assert!(
            fading[0] > 40 && fading[0] < 230,
            "and fades at its edge: {fading:?}"
        );
        assert_eq!(beyond, [255; 4]);
        assert_eq!(renderer.glyphs.len(), 3, "and takes no room in the atlas");
    }

    #[test]
    #[ignore = "requires a native GPU adapter"]
    fn icons_past_one_atlas_never_fail_a_frame() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
        let (device, queue) =
            pollster::block_on(adapter.request_device(&Default::default())).unwrap();
        let target = Target::new(&device);
        let format = wgpu::TextureFormat::Rgba8UnormSrgb;
        let mut renderer = Renderer::new(device, queue, format);
        // Each tint is an entry of its own, 64 device pixels square.
        let icons = |tints: Range<usize>| -> Vec<Primitive<'static>> {
            tints
                .map(|tint| Primitive::Icon {
                    sources: &[CHECKBOX],
                    origin: [0.0; 2],
                    size: 32.0,
                    tint: [tint as f32 / 65_536.0, 0.0, 0.0, 1.0],
                    palette: Palette::default(),
                })
                .collect()
        };
        let one_atlas = (ATLAS_SIZE as usize / 65).pow(2);
        for frame in 0..8 {
            let start = frame * one_atlas / 3;
            let primitives = icons(start..start + one_atlas / 3);
            target.draw(&mut renderer, &page(&primitives)).unwrap();
        }
        assert_eq!(
            renderer.atlas_side(),
            ATLAS_SIZE,
            "frames each needing a third of the atlas evict, never grow it"
        );
        let primitives = icons(0..3 * one_atlas);
        target.draw(&mut renderer, &page(&primitives)).unwrap();
        assert!(renderer.atlas_side() > ATLAS_SIZE);
        assert!(renderer.glyphs.len() >= 3 * one_atlas);
        let capture = target.capture(&renderer);
        let pixel = |x: usize, y: usize| &capture[(y * 512 + x) * 4..(y * 512 + x) * 4 + 4];
        assert_ne!(pixel(24 + 12, 24 + 32), [255; 4], "the icons paint");
    }

    #[test]
    #[ignore = "requires a native GPU adapter"]
    fn transparent_targets_hold_premultiplied_coverage_and_erase() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
        let (device, queue) =
            pollster::block_on(adapter.request_device(&Default::default())).unwrap();
        let target = Target::new(&device);
        let mut renderer = Renderer::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb);
        let primitives = [
            Primitive::Rect {
                rect: [0.0, 0.0, 10.0, 10.0],
                color: [1.0, 1.0, 1.0, 0.5],
            },
            Primitive::Rect {
                rect: [20.0, 0.0, 40.0, 10.0],
                color: [0.0, 0.0, 1.0, 1.0],
            },
            Primitive::Path {
                data: "M0 0H10V10H0Z",
                origin: [30.0, 0.0],
                style: PathStyle::Erase,
                colors: [[0.0, 0.0, 0.0, 1.0]; 2],
            },
        ];
        renderer
            .draw(
                &target.texture.create_view(&Default::default()),
                [512, 256],
                [0.0; 4],
                &[Layer {
                    scale: 1.0,
                    origin: [0.0; 2],
                    clip: None,
                    backdrop: None,
                    round: None,
                    motion: None,
                    primitives: &primitives,
                }],
            )
            .unwrap();
        let capture = target.capture(&renderer);
        let pixel = |x: usize, y: usize| &capture[(y * 512 + x) * 4..(y * 512 + x) * 4 + 4];
        assert_eq!(pixel(5, 5)[3], 128, "half coverage is half opaque");
        assert_eq!(pixel(5, 5)[0], 188, "over premultiplied colour");
        assert_eq!(pixel(25, 5), [0, 0, 255, 255]);
        assert_eq!(pixel(35, 5), [0; 4], "erased back to transparency");
        assert_eq!(pixel(50, 5), [0; 4]);
    }

    #[test]
    #[ignore = "requires a native GPU adapter"]
    fn translucent_windows_take_colour_premultiplied_in_srgb() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
        let (device, queue) =
            pollster::block_on(adapter.request_device(&Default::default())).unwrap();
        let format = wgpu::TextureFormat::Rgba8UnormSrgb;
        let mut renderer = Renderer::new(device.clone(), queue.clone(), format);
        let mut translucent = Translucent::new(&device, format, wgpu::TextureFormat::Rgba8Unorm);
        let primitives = [
            Primitive::Rect {
                rect: [0.0, 0.0, 10.0, 10.0],
                color: [1.0, 1.0, 1.0, 0.2],
            },
            Primitive::Rect {
                rect: [20.0, 0.0, 30.0, 10.0],
                color: [0.2, 0.2, 0.2, 1.0],
            },
        ];
        let frame = translucent.target(&device, [512, 256]);
        renderer
            .draw(
                &frame,
                [512, 256],
                [0.0; 4],
                &[Layer {
                    scale: 1.0,
                    origin: [0.0; 2],
                    clip: None,
                    backdrop: None,
                    round: None,
                    motion: None,
                    primitives: &primitives,
                }],
            )
            .unwrap();
        let window = Target::with_format(&device, wgpu::TextureFormat::Rgba8Unorm);
        translucent.present(
            &device,
            &queue,
            &window.texture.create_view(&Default::default()),
        );
        let capture = window.capture(&renderer);
        let pixel = |x: usize, y: usize| &capture[(y * 512 + x) * 4..(y * 512 + x) * 4 + 4];
        assert_eq!(
            pixel(5, 5),
            [51, 51, 51, 51],
            "white at a fifth, premultiplied in sRGB"
        );
        assert_eq!(
            pixel(25, 5),
            [124, 124, 124, 255],
            "opaque colours encode as ever"
        );
        assert_eq!(pixel(50, 5), [0; 4]);
    }

    #[test]
    #[ignore = "requires a native GPU adapter"]
    fn layers_transform_and_clip_independently() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
        let (device, queue) =
            pollster::block_on(adapter.request_device(&Default::default())).unwrap();
        let target = Target::new(&device);
        let mut renderer = Renderer::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb);
        let fill = [Primitive::Rect {
            rect: [0.0, 0.0, 300.0, 100.0],
            color: [1.0, 0.0, 0.0, 1.0],
        }];
        let chrome = [Primitive::Rect {
            rect: [0.0, 0.0, 10.0, 10.0],
            color: [0.0, 0.0, 1.0, 1.0],
        }];
        target
            .draw(
                &mut renderer,
                &[
                    Layer {
                        scale: 1.0,
                        origin: [0.0; 2],
                        clip: Some([100.0, 50.0, 200.5, 80.0]),
                        backdrop: None,
                        round: None,
                        motion: None,
                        primitives: &fill,
                    },
                    Layer {
                        scale: 2.0,
                        origin: [300.0, 10.0],
                        clip: None,
                        backdrop: None,
                        round: None,
                        motion: None,
                        primitives: &chrome,
                    },
                ],
            )
            .unwrap();
        let capture = target.capture(&renderer);
        let pixel = |x: usize, y: usize| &capture[(y * 512 + x) * 4..(y * 512 + x) * 4 + 4];
        assert_eq!(pixel(10, 10), [255; 4], "clipped away");
        assert_eq!(pixel(100, 50), [255, 0, 0, 255]);
        assert_eq!(
            pixel(200, 79),
            [255, 0, 0, 255],
            "partial pixels round outward"
        );
        assert_eq!(pixel(99, 60), [255; 4]);
        assert_eq!(pixel(201, 60), [255; 4]);
        assert_eq!(pixel(150, 80), [255; 4]);
        assert_eq!(pixel(319, 29), [0, 0, 255, 255]);
        assert_eq!(pixel(320, 30), [255; 4]);
        assert!(matches!(
            target.draw(
                &mut renderer,
                &[Layer {
                    scale: 0.0,
                    origin: [0.0; 2],
                    clip: None,
                    backdrop: None,
                    round: None,
                    motion: None,
                    primitives: &fill,
                }]
            ),
            Err(RenderError::InvalidLayer)
        ));
    }
}
