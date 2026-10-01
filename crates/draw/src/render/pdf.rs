//! Layers written as the sheets of a PDF: glyphs as text in subset fonts, which viewers
//! select, search and copy, shapes and ink as vector paths, pictures and icons as images.

use super::{
    GlyphRun, Layer, PathStyle, Primitive, RasterImage, RenderError, Stroke, icon, srgb_byte,
};
use parley::fontique::Blob;
use pdf_writer::{
    Content, Filter, Finish, Name, Pdf, Rect, Ref, Str, TextStr,
    types::{
        BlendMode, CidFontType, FontFlags, LineCapStyle, LineJoinStyle, SystemInfo,
        TextRenderingMode, UnicodeCmap,
    },
};
use std::collections::{BTreeMap, HashMap, btree_map::Entry};
use swash::{
    FontRef, StringId,
    scale::{Render, ScaleContext, Source, StrikeWith, image::Content as Pixels},
    tag_from_bytes,
    zeno::{Command, PathData},
};

/// One sheet of paper.
pub struct Sheet<'a> {
    /// Width and height in points.
    pub size: [f32; 2],
    /// Layers whose device units are points from the sheet's top left.
    pub layers: Vec<Layer<'a>>,
}

/// `sheets` as a PDF document titled `title`.
pub fn pdf(title: &str, sheets: &[Sheet<'_>]) -> Result<Vec<u8>, RenderError> {
    let mut document = Document {
        pdf: Pdf::new(),
        next: 1,
        fonts: Vec::new(),
        images: HashMap::new(),
        states: HashMap::new(),
        scale: ScaleContext::new(),
    };
    let catalog = document.reserve();
    let tree = document.reserve();
    let pages: Vec<_> = sheets
        .iter()
        .map(|sheet| {
            let page = document.reserve();
            let content = document.reserve();
            let used = document.sheet(sheet, content)?;
            Ok((page, used))
        })
        .collect::<Result<_, RenderError>>()?;
    for ((page, used), sheet) in pages.iter().zip(sheets) {
        let mut writer = document.pdf.page(*page);
        writer
            .parent(tree)
            .media_box(Rect::new(0.0, 0.0, sheet.size[0], sheet.size[1]))
            .contents(used.content);
        let mut resources = writer.resources();
        let mut fonts = resources.fonts();
        for font in &used.fonts {
            let reference = document.fonts[*font].reference;
            fonts.pair(Name(format!("F{font}").as_bytes()), reference);
        }
        fonts.finish();
        let mut images = resources.x_objects();
        for image in &used.images {
            images.pair(Name(format!("I{}", image.get()).as_bytes()), *image);
        }
        images.finish();
        let mut states = resources.ext_g_states();
        for state in &used.states {
            states.pair(Name(format!("G{}", state.get()).as_bytes()), *state);
        }
    }
    document
        .pdf
        .pages(tree)
        .kids(pages.iter().map(|(page, _)| *page))
        .count(pages.len() as i32);
    document.pdf.catalog(catalog).pages(tree);
    let info = document.reserve();
    document
        .pdf
        .document_info(info)
        .title(TextStr(title))
        .producer(TextStr("Snowbound"));
    let fonts = std::mem::take(&mut document.fonts);
    for font in fonts.into_iter().filter(|font| font.outlined) {
        document.write_font(font)?;
    }
    Ok(document.pdf.finish())
}

struct Document {
    pdf: Pdf,
    next: i32,
    fonts: Vec<Font>,
    /// Image XObjects by what they show.
    images: HashMap<ImageKey, Ref>,
    /// Graphics states by fill and stroke opacity bits and whether they multiply.
    states: HashMap<(u32, bool), Ref>,
    scale: ScaleContext,
}

#[derive(Hash, PartialEq, Eq)]
enum ImageKey {
    Raster(u64),
    Icon {
        sources: usize,
        size: u32,
        tint: [u32; 3],
        palette: [Option<[u32; 3]>; 3],
    },
    Glyph {
        font: u64,
        index: u32,
        glyph: u32,
        size: u32,
        color: [u32; 3],
    },
}

/// A face the document shows glyphs of.
struct Font {
    reference: Ref,
    data: Blob<u8>,
    index: u32,
    /// Whether its glyphs are outlines the PDF embeds; others draw as pictures.
    outlined: bool,
    glyphs: subsetter::GlyphRemapper,
    /// The text each subset glyph shows where it first appeared.
    text: BTreeMap<u16, String>,
}

/// What a sheet's content refers to.
struct Used {
    content: Ref,
    fonts: Vec<usize>,
    images: Vec<Ref>,
    states: Vec<Ref>,
}

impl Used {
    fn add<T: PartialEq>(list: &mut Vec<T>, item: T) {
        if !list.contains(&item) {
            list.push(item);
        }
    }
}

fn compress(data: &[u8]) -> Vec<u8> {
    miniz_oxide::deflate::compress_to_vec_zlib(data, 6)
}

/// sRGB components of a linear colour.
fn encoded(color: [f32; 4]) -> [f32; 3] {
    [color[0], color[1], color[2]].map(|value| f32::from(srgb_byte(value.clamp(0.0, 1.0))) / 255.0)
}

/// Bézier handle length for a quarter circle of radius one.
const KAPPA: f32 = 0.552_284_8;

/// Appends an ellipse's quarter arcs around `rect`'s corners of radii `radius`, as a
/// closed path.
fn rounded(content: &mut Content, [left, top, right, bottom]: [f32; 4], radius: [f32; 2]) {
    let rx = radius[0].clamp(0.0, (right - left) / 2.0);
    let ry = radius[1].clamp(0.0, (bottom - top) / 2.0);
    if rx <= 0.0 || ry <= 0.0 {
        content.rect(left, top, right - left, bottom - top);
        return;
    }
    let [kx, ky] = [rx * KAPPA, ry * KAPPA];
    content.move_to(left + rx, top);
    content.line_to(right - rx, top);
    content.cubic_to(right - rx + kx, top, right, top + ry - ky, right, top + ry);
    content.line_to(right, bottom - ry);
    content.cubic_to(
        right,
        bottom - ry + ky,
        right - rx + kx,
        bottom,
        right - rx,
        bottom,
    );
    content.line_to(left + rx, bottom);
    content.cubic_to(
        left + rx - kx,
        bottom,
        left,
        bottom - ry + ky,
        left,
        bottom - ry,
    );
    content.line_to(left, top + ry);
    content.cubic_to(left, top + ry - ky, left + rx - kx, top, left + rx, top);
    content.close_path();
}

fn circle(content: &mut Content, [x, y]: [f32; 2], radius: f32) {
    rounded(
        content,
        [x - radius, y - radius, x + radius, y + radius],
        [radius; 2],
    );
}

/// The text each of `count` glyphs shows: a cluster's text goes to its first glyph, and a
/// ligature's later clusters to the glyph before them.
fn glyph_text(
    text: &str,
    clusters: &mut dyn Iterator<Item = (std::ops::Range<usize>, usize)>,
    count: usize,
) -> Vec<String> {
    let mut texts = vec![String::new(); count];
    let mut at = 0_usize;
    for (range, glyphs) in clusters {
        let shown = text.get(range).unwrap_or_default();
        if glyphs == 0 {
            if let Some(previous) = at.checked_sub(1).and_then(|at| texts.get_mut(at)) {
                previous.push_str(shown);
            }
            continue;
        }
        if let Some(first) = texts.get_mut(at) {
            first.push_str(shown);
        }
        at += glyphs;
    }
    texts
}

impl Document {
    fn reserve(&mut self) -> Ref {
        let reference = Ref::new(self.next);
        self.next += 1;
        reference
    }

    fn sheet(&mut self, sheet: &Sheet<'_>, content_ref: Ref) -> Result<Used, RenderError> {
        let mut used = Used {
            content: content_ref,
            fonts: Vec::new(),
            images: Vec::new(),
            states: Vec::new(),
        };
        let mut content = Content::new();
        // Layers measure down from the top left, as the renderer's do.
        content.transform([1.0, 0.0, 0.0, -1.0, 0.0, sheet.size[1]]);
        for layer in &sheet.layers {
            content.save_state();
            if let Some([left, top, right, bottom]) = layer.clip {
                content
                    .rect(left, top, right - left, bottom - top)
                    .clip_nonzero()
                    .end_path();
            }
            // Lines of text outside a clipping layer are left out, so that no sheet's text
            // layer holds text it does not show.
            let rows = layer
                .clip
                .map(|clip| [clip[1], clip[3]].map(|y| (y - layer.origin[1]) / layer.scale));
            content.transform([
                layer.scale,
                0.0,
                0.0,
                layer.scale,
                layer.origin[0],
                layer.origin[1],
            ]);
            for primitive in layer.primitives {
                self.primitive(&mut content, &mut used, primitive, rows)?;
            }
            content.restore_state();
        }
        let data = compress(&content.finish());
        self.pdf
            .stream(content_ref, &data)
            .filter(Filter::FlateDecode);
        Ok(used)
    }

    /// Sets the fill, or with `stroke` the stroke, to `color` with its opacity.
    fn paint(&mut self, content: &mut Content, used: &mut Used, color: [f32; 4], stroke: bool) {
        let [red, green, blue] = encoded(color);
        if stroke {
            content.set_stroke_rgb(red, green, blue);
        } else {
            content.set_fill_rgb(red, green, blue);
        }
        if color[3] < 1.0 {
            self.opacity(content, used, color[3], false);
        }
    }

    /// Paints at `opacity`, multiplying what lies beneath if `multiply`.
    fn opacity(&mut self, content: &mut Content, used: &mut Used, opacity: f32, multiply: bool) {
        let opacity = opacity.clamp(0.0, 1.0);
        let key = (opacity.to_bits(), multiply);
        let state = match self.states.get(&key) {
            Some(state) => *state,
            None => {
                let state = self.reserve();
                let mut writer = self.pdf.ext_graphics(state);
                writer.non_stroking_alpha(opacity).stroking_alpha(opacity);
                if multiply {
                    writer.blend_mode(BlendMode::Multiply);
                }
                writer.finish();
                self.states.insert(key, state);
                state
            }
        };
        Used::add(&mut used.states, state);
        content.set_parameters(Name(format!("G{}", state.get()).as_bytes()));
    }

    fn primitive(
        &mut self,
        content: &mut Content,
        used: &mut Used,
        primitive: &Primitive<'_>,
        rows: Option<[f32; 2]>,
    ) -> Result<(), RenderError> {
        match primitive {
            Primitive::Text {
                text,
                origin,
                clip,
                ink,
            } => {
                content.save_state();
                if let Some([left, top, right, bottom]) = *clip {
                    content
                        .rect(left, top, right - left, bottom - top)
                        .clip_nonzero()
                        .end_path();
                }
                text.runs(&mut |run| self.run(content, used, run, *origin, *ink, rows))?;
                content.restore_state();
            }
            Primitive::Icon {
                sources,
                origin,
                size,
                tint,
                palette,
            } => {
                // Icons are small art; eight pixels a point keeps them sharp on paper.
                let side = (size * 8.0).ceil().clamp(16.0, 512.0) as u32;
                let key = ImageKey::Icon {
                    sources: sources.as_ptr() as usize,
                    size: side,
                    tint: [tint[0], tint[1], tint[2]].map(f32::to_bits),
                    palette: palette.bits(),
                };
                let image = match self.images.get(&key) {
                    Some(image) => *image,
                    None => {
                        let raster =
                            icon::rasterize(sources, side, [tint[0], tint[1], tint[2]], palette);
                        let image = self.image([side; 2], &raster.data, false);
                        self.images.insert(key, image);
                        image
                    }
                };
                content.save_state();
                if tint[3] < 1.0 {
                    self.opacity(content, used, tint[3], false);
                }
                self.place(
                    content,
                    used,
                    image,
                    [origin[0], origin[1], origin[0] + size, origin[1] + size],
                );
                content.restore_state();
            }
            Primitive::Path {
                data,
                origin,
                style,
                colors,
            } => {
                let fill = matches!(style, PathStyle::Fill);
                let (PathStyle::Fill | PathStyle::Stroke(_)) = style else {
                    return Ok(());
                };
                content.save_state();
                content.transform([1.0, 0.0, 0.0, 1.0, origin[0], origin[1]]);
                let average = std::array::from_fn(|at| (colors[0][at] + colors[1][at]) / 2.0);
                self.paint(content, used, average, !fill);
                let mut last = [0.0_f32; 2];
                for command in data.commands() {
                    match command {
                        Command::MoveTo(to) => {
                            content.move_to(to.x, to.y);
                            last = [to.x, to.y];
                        }
                        Command::LineTo(to) => {
                            content.line_to(to.x, to.y);
                            last = [to.x, to.y];
                        }
                        Command::CurveTo(one, two, to) => {
                            content.cubic_to(one.x, one.y, two.x, two.y, to.x, to.y);
                            last = [to.x, to.y];
                        }
                        Command::QuadTo(control, to) => {
                            let near =
                                |from: f32, control: f32| from + (control - from) * 2.0 / 3.0;
                            content.cubic_to(
                                near(last[0], control.x),
                                near(last[1], control.y),
                                near(to.x, control.x),
                                near(to.y, control.y),
                                to.x,
                                to.y,
                            );
                            last = [to.x, to.y];
                        }
                        Command::Close => {
                            content.close_path();
                        }
                    }
                }
                match style {
                    PathStyle::Stroke(width) => {
                        content
                            .set_line_width(*width)
                            .set_line_cap(LineCapStyle::RoundCap)
                            .set_line_join(LineJoinStyle::RoundJoin)
                            .stroke();
                    }
                    _ => {
                        content.fill_nonzero();
                    }
                }
                content.restore_state();
            }
            Primitive::Rect { rect, color } => {
                content.save_state();
                self.paint(content, used, *color, false);
                content
                    .rect(rect[0], rect[1], rect[2] - rect[0], rect[3] - rect[1])
                    .fill_nonzero();
                content.restore_state();
            }
            Primitive::RoundedRect {
                rect,
                radius,
                stroke,
                color,
            } => {
                content.save_state();
                self.paint(content, used, *color, stroke.is_some());
                rounded(content, *rect, *radius);
                match stroke {
                    None => {
                        content.fill_nonzero();
                    }
                    Some(Stroke::Solid(width)) => {
                        content.set_line_width(*width).stroke();
                    }
                    Some(Stroke::Dashed(width)) => {
                        content
                            .set_line_width(*width)
                            .set_dash_pattern([width * 2.0, width * 2.0], 0.0)
                            .stroke();
                    }
                }
                content.restore_state();
            }
            Primitive::Gradient {
                rect,
                radius,
                colors,
            } => {
                content.save_state();
                let average = std::array::from_fn(|at| (colors[0][at] + colors[1][at]) / 2.0);
                self.paint(content, used, average, false);
                rounded(content, *rect, *radius);
                content.fill_nonzero();
                content.restore_state();
            }
            // Soft shadows are for the screen.
            Primitive::Shadow { .. } => {}
            Primitive::Image { image, rect } => {
                let key = ImageKey::Raster(image.id());
                let reference = match self.images.get(&key) {
                    Some(reference) => *reference,
                    None => {
                        let reference = self.raster(image);
                        self.images.insert(key, reference);
                        reference
                    }
                };
                self.place(content, used, reference, *rect);
            }
            Primitive::Segment {
                from,
                to,
                width,
                round,
                color,
            } => {
                content.save_state();
                self.paint(content, used, *color, true);
                content
                    .set_line_width(*width)
                    .set_line_cap(if *round {
                        LineCapStyle::RoundCap
                    } else {
                        LineCapStyle::ProjectingSquareCap
                    })
                    .move_to(from[0], from[1])
                    .line_to(to[0], to[1])
                    .stroke();
                content.restore_state();
            }
            Primitive::Taper {
                from,
                to,
                widths,
                round: false,
                color,
            } => {
                content.save_state();
                self.paint(content, used, *color, false);
                // Square ends reach half their width past each point, as a square tip does.
                let [from_radius, to_radius] = widths.map(|width| width / 2.0);
                let [dx, dy] = [to[0] - from[0], to[1] - from[1]];
                let length = dx.hypot(dy);
                let [ux, uy] = if length > 0.0 {
                    [dx / length, dy / length]
                } else {
                    [1.0, 0.0]
                };
                let corner = |center: &[f32; 2], radius: f32, ahead: f32, side: f32| {
                    [
                        center[0] + radius * (ux * ahead - uy * side),
                        center[1] + radius * (uy * ahead + ux * side),
                    ]
                };
                let corners = [
                    corner(from, from_radius, -1.0, 1.0),
                    corner(to, to_radius, 1.0, 1.0),
                    corner(to, to_radius, 1.0, -1.0),
                    corner(from, from_radius, -1.0, -1.0),
                ];
                content.move_to(corners[0][0], corners[0][1]);
                for corner in &corners[1..] {
                    content.line_to(corner[0], corner[1]);
                }
                content.close_path().fill_nonzero();
                content.restore_state();
            }
            Primitive::Taper {
                from,
                to,
                widths,
                color,
                ..
            } => {
                content.save_state();
                self.paint(content, used, *color, false);
                let [from_radius, to_radius] = widths.map(|width| width / 2.0);
                // Each part fills on its own, as their windings differ.
                circle(content, *from, from_radius);
                content.fill_nonzero();
                circle(content, *to, to_radius);
                content.fill_nonzero();
                let [dx, dy] = [to[0] - from[0], to[1] - from[1]];
                let length = dx.hypot(dy);
                if length > (from_radius - to_radius).abs() {
                    // The lines touching both ends' circles.
                    let [ux, uy] = [dx / length, dy / length];
                    let sine = (from_radius - to_radius) / length;
                    let cosine = (1.0 - sine * sine).sqrt();
                    let touch = |center: &[f32; 2], radius: f32, side: f32| {
                        [
                            center[0] + radius * (-uy * side * cosine + ux * sine),
                            center[1] + radius * (ux * side * cosine + uy * sine),
                        ]
                    };
                    let corners = [
                        touch(from, from_radius, 1.0),
                        touch(to, to_radius, 1.0),
                        touch(to, to_radius, -1.0),
                        touch(from, from_radius, -1.0),
                    ];
                    content.move_to(corners[0][0], corners[0][1]);
                    for corner in &corners[1..] {
                        content.line_to(corner[0], corner[1]);
                    }
                    content.close_path().fill_nonzero();
                }
                content.restore_state();
            }
            Primitive::Highlight {
                from,
                to,
                width,
                color,
            } => {
                content.save_state();
                self.opacity(content, used, color[3], true);
                let [red, green, blue] = encoded(*color);
                content
                    .set_stroke_rgb(red, green, blue)
                    .set_line_width(*width)
                    .set_line_cap(LineCapStyle::ProjectingSquareCap)
                    .move_to(from[0], from[1])
                    .line_to(to[0], to[1])
                    .stroke();
                content.restore_state();
            }
        }
        Ok(())
    }

    /// Draws image XObject `image` filling `rect`.
    fn place(&mut self, content: &mut Content, used: &mut Used, image: Ref, rect: [f32; 4]) {
        Used::add(&mut used.images, image);
        content.save_state();
        // Image space runs up from the bottom left.
        content.transform([
            rect[2] - rect[0],
            0.0,
            0.0,
            rect[1] - rect[3],
            rect[0],
            rect[3],
        ]);
        content.x_object(Name(format!("I{}", image.get()).as_bytes()));
        content.restore_state();
    }

    /// A picture as an image XObject: opaque photographs as JPEG, others losslessly.
    fn raster(&mut self, image: &RasterImage) -> Ref {
        // Pixels are premultiplied in linear light.
        let straight: Vec<u8> = image
            .pixels()
            .chunks_exact(4)
            .flat_map(|pixel| {
                let alpha = pixel[3];
                if alpha == 255 || alpha == 0 {
                    return [pixel[0], pixel[1], pixel[2], alpha];
                }
                let [red, green, blue] = [pixel[0], pixel[1], pixel[2]].map(|byte| {
                    let linear = crate::linear(f32::from(byte) / 255.0);
                    srgb_byte((linear * 255.0 / f32::from(alpha)).min(1.0))
                });
                [red, green, blue, alpha]
            })
            .collect();
        let size = image.size();
        let opaque = straight.chunks_exact(4).all(|pixel| pixel[3] == 255);
        if opaque && size[0] * size[1] >= 64 * 64 {
            let rgb: Vec<u8> = straight
                .chunks_exact(4)
                .flat_map(|pixel| [pixel[0], pixel[1], pixel[2]])
                .collect();
            let mut jpeg = Vec::new();
            let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 92);
            if image::ImageEncoder::write_image(
                encoder,
                &rgb,
                size[0],
                size[1],
                image::ExtendedColorType::Rgb8,
            )
            .is_ok()
            {
                let reference = self.reserve();
                let mut writer = self.pdf.image_xobject(reference, &jpeg);
                writer.filter(Filter::DctDecode);
                writer
                    .width(size[0] as i32)
                    .height(size[1] as i32)
                    .color_space_name(Name(b"DeviceRGB"))
                    .bits_per_component(8)
                    .interpolate(true);
                return reference;
            }
        }
        self.image(size, &straight, true)
    }

    /// Straight sRGB RGBA pixels as a losslessly compressed image XObject.
    fn image(&mut self, [width, height]: [u32; 2], rgba: &[u8], interpolate: bool) -> Ref {
        let rgb: Vec<u8> = rgba
            .chunks_exact(4)
            .flat_map(|pixel| [pixel[0], pixel[1], pixel[2]])
            .collect();
        let mask = rgba.chunks_exact(4).any(|pixel| pixel[3] != 255).then(|| {
            let alpha: Vec<u8> = rgba.chunks_exact(4).map(|pixel| pixel[3]).collect();
            let mask = self.reserve();
            let data = compress(&alpha);
            let mut writer = self.pdf.image_xobject(mask, &data);
            writer.filter(Filter::FlateDecode);
            writer
                .width(width as i32)
                .height(height as i32)
                .color_space_name(Name(b"DeviceGray"))
                .bits_per_component(8)
                .interpolate(interpolate);
            mask
        });
        let reference = self.reserve();
        let data = compress(&rgb);
        let mut writer = self.pdf.image_xobject(reference, &data);
        writer.filter(Filter::FlateDecode);
        writer
            .width(width as i32)
            .height(height as i32)
            .color_space_name(Name(b"DeviceRGB"))
            .bits_per_component(8)
            .interpolate(interpolate);
        if let Some(mask) = mask {
            writer.s_mask(mask);
        }
        reference
    }

    /// The document's face for a run's font, added on first use.
    fn font(&mut self, data: &Blob<u8>, index: u32) -> Result<usize, RenderError> {
        if let Some(found) = self
            .fonts
            .iter()
            .position(|font| font.data.id() == data.id() && font.index == index)
        {
            return Ok(found);
        }
        let face = FontRef::from_index(data.as_ref(), index as usize)
            .ok_or(RenderError::UnreadableFont)?;
        let outlined = face.table(tag_from_bytes(b"glyf")).is_some()
            || face.table(tag_from_bytes(b"CFF ")).is_some();
        let reference = self.reserve();
        self.fonts.push(Font {
            reference,
            data: data.clone(),
            index,
            outlined,
            glyphs: subsetter::GlyphRemapper::new(),
            text: BTreeMap::new(),
        });
        Ok(self.fonts.len() - 1)
    }

    fn run(
        &mut self,
        content: &mut Content,
        used: &mut Used,
        run: GlyphRun<'_>,
        origin: [f32; 2],
        ink: [f32; 4],
        rows: Option<[f32; 2]>,
    ) -> Result<(), RenderError> {
        let [top, bottom] = [
            origin[1] + run.line[0],
            origin[1] + run.line[0] + run.line[1],
        ];
        if rows.is_some_and(|[first, last]| bottom <= first || top >= last) {
            return Ok(());
        }
        let font = self.font(run.font, run.index)?;
        let color = run.color.unwrap_or(ink);
        let glyphs: Vec<_> = run.glyphs.collect();
        let texts = glyph_text(run.text, run.clusters, glyphs.len());
        content.save_state();
        if self.fonts[font].outlined {
            Used::add(&mut used.fonts, font);
            self.paint(content, used, color, false);
            let face = FontRef::from_index(run.font.as_ref(), run.index as usize)
                .ok_or(RenderError::UnreadableFont)?;
            let units = f32::from(face.metrics(&[]).units_per_em.max(1));
            let advances = face.glyph_metrics(&[]);
            let name = format!("F{font}");
            content
                .begin_text()
                .set_font(Name(name.as_bytes()), run.size);
            if run.embolden {
                self.paint(content, used, color, true);
                content
                    .set_text_rendering_mode(TextRenderingMode::FillStroke)
                    .set_line_width(run.size / 24.0);
            }
            let shear = run.skew.map_or(0.0, |degrees| degrees.to_radians().tan());
            let mut shown: Vec<(u16, f32)> = Vec::new();
            let mut pen: Option<[f32; 2]> = None;
            let flush = |content: &mut Content, shown: &mut Vec<(u16, f32)>| {
                if shown.is_empty() {
                    return;
                }
                let mut positioned = content.show_positioned();
                let mut items = positioned.items();
                for (glyph, adjust) in shown.drain(..) {
                    if adjust != 0.0 {
                        items.adjust(adjust);
                    }
                    items.show(Str(&glyph.to_be_bytes()));
                }
            };
            for (glyph, text) in glyphs.iter().zip(texts) {
                let Ok(id) = u16::try_from(glyph.id) else {
                    continue;
                };
                let face = &mut self.fonts[font];
                let cid = face.glyphs.remap(id);
                let [x, y] = [origin[0] + glyph.x, origin[1] + glyph.y];
                let conflict = !text.is_empty()
                    && match face.text.entry(cid) {
                        Entry::Vacant(entry) => {
                            entry.insert(text.clone());
                            false
                        }
                        Entry::Occupied(entry) => *entry.get() != text,
                    };
                let advance = advances.advance_width(id) / units * run.size;
                match pen {
                    Some([at, line]) if !conflict && (line - y).abs() < 0.001 => {
                        shown.push((cid, (at - x) * 1000.0 / run.size));
                    }
                    _ => {
                        flush(content, &mut shown);
                        content.set_text_matrix([1.0, 0.0, shear, -1.0, x, y]);
                        if conflict {
                            // A glyph shared by different text says what it shows here.
                            content
                                .begin_marked_content_with_properties(Name(b"Span"))
                                .properties()
                                .actual_text(TextStr(&text));
                            content.show(Str(&cid.to_be_bytes()));
                            content.end_marked_content();
                            pen = None;
                            continue;
                        }
                        shown.push((cid, 0.0));
                    }
                }
                pen = Some([x + advance, y]);
            }
            flush(content, &mut shown);
            content.end_text();
        } else {
            for glyph in &glyphs {
                self.glyph_picture(content, used, &run, *glyph, origin, color);
            }
        }
        for decoration in run.decorations {
            self.paint(content, used, decoration.color.unwrap_or(color), false);
            content
                .rect(
                    origin[0] + decoration.x,
                    origin[1] + decoration.y,
                    decoration.width,
                    decoration.thickness,
                )
                .fill_nonzero();
        }
        content.restore_state();
        Ok(())
    }

    /// A glyph of a face without outlines the PDF can embed, such as a colour emoji, drawn
    /// as a picture four pixels a point.
    fn glyph_picture(
        &mut self,
        content: &mut Content,
        used: &mut Used,
        run: &GlyphRun<'_>,
        glyph: super::Glyph,
        origin: [f32; 2],
        color: [f32; 4],
    ) {
        const DENSITY: f32 = 4.0;
        let Ok(id) = u16::try_from(glyph.id) else {
            return;
        };
        let Some(face) = FontRef::from_index(run.font.as_ref(), run.index as usize) else {
            return;
        };
        let size = run.size * DENSITY;
        let mut scaler = self
            .scale
            .builder(face)
            .size(size)
            .normalized_coords(run.coords.iter().copied())
            .build();
        let Some(rendered) = Render::new(&[
            Source::ColorOutline(0),
            Source::ColorBitmap(StrikeWith::BestFit),
            Source::Outline,
        ])
        .render(&mut scaler, id) else {
            return;
        };
        let placement = rendered.placement;
        if placement.width == 0 || placement.height == 0 {
            return;
        }
        let key = ImageKey::Glyph {
            font: run.font.id(),
            index: run.index,
            glyph: glyph.id,
            size: size.to_bits(),
            color: [color[0], color[1], color[2]].map(f32::to_bits),
        };
        let image = match self.images.get(&key) {
            Some(image) => *image,
            None => {
                let [red, green, blue] = encoded(color).map(|value| (value * 255.0).round() as u8);
                let rgba: Vec<u8> = match rendered.content {
                    Pixels::Mask => rendered
                        .data
                        .iter()
                        .flat_map(|coverage| [red, green, blue, *coverage])
                        .collect(),
                    Pixels::Color | Pixels::SubpixelMask => rendered.data.clone(),
                };
                let image = self.image([placement.width, placement.height], &rgba, true);
                self.images.insert(key, image);
                image
            }
        };
        let [x, y] = [
            origin[0] + glyph.x + placement.left as f32 / DENSITY,
            origin[1] + glyph.y - placement.top as f32 / DENSITY,
        ];
        self.place(
            content,
            used,
            image,
            [
                x,
                y,
                x + placement.width as f32 / DENSITY,
                y + placement.height as f32 / DENSITY,
            ],
        );
    }

    /// Embeds `font`'s subset as a CID-keyed Type 0 font with a ToUnicode map.
    fn write_font(&mut self, font: Font) -> Result<(), RenderError> {
        let data = font.data.as_ref();
        let face =
            FontRef::from_index(data, font.index as usize).ok_or(RenderError::UnreadableFont)?;
        let subset = subsetter::subset(data, font.index, &font.glyphs)
            .map_err(|_| RenderError::UnreadableFont)?;
        let cff = face.table(tag_from_bytes(b"CFF ")).is_some();
        let metrics = face.metrics(&[]);
        let scale = 1000.0 / f32::from(metrics.units_per_em.max(1));
        let advances = face.glyph_metrics(&[]);
        let widths: Vec<f32> = font
            .glyphs
            .remapped_gids()
            .map(|old| advances.advance_width(old) * scale)
            .collect();
        let postscript: String = face
            .localized_strings()
            .find_by_id(StringId::PostScript, None)
            .map(|name| name.chars().filter(char::is_ascii_alphanumeric).collect())
            .filter(|name: &String| !name.is_empty())
            .unwrap_or_else(|| "Font".to_owned());
        // A subset's name starts with six capitals naming the subset.
        let mut hash = font.reference.get() as u32;
        let tag: String = (0..6)
            .map(|_| {
                hash = hash.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                char::from(b'A' + (hash >> 16) as u8 % 26)
            })
            .collect();
        let base = format!("{tag}+{postscript}");
        let [cid, descriptor, file, map] = [(); 4].map(|_| self.reserve());
        let info = SystemInfo {
            registry: Str(b"Adobe"),
            ordering: Str(b"Identity"),
            supplement: 0,
        };
        self.pdf
            .type0_font(font.reference)
            .base_font(Name(base.as_bytes()))
            .encoding_predefined(Name(b"Identity-H"))
            .descendant_font(cid)
            .to_unicode(map);
        let mut writer = self.pdf.cid_font(cid);
        writer
            .subtype(if cff {
                CidFontType::Type0
            } else {
                CidFontType::Type2
            })
            .base_font(Name(base.as_bytes()))
            .system_info(info)
            .font_descriptor(descriptor)
            .default_width(0.0);
        if !cff {
            writer.cid_to_gid_map_predefined(Name(b"Identity"));
        }
        writer.widths().consecutive(0, widths);
        writer.finish();
        let mut writer = self.pdf.font_descriptor(descriptor);
        writer
            .name(Name(base.as_bytes()))
            .flags(FontFlags::SYMBOLIC)
            .bbox(Rect::new(
                0.0,
                -metrics.descent * scale,
                metrics.max_width * scale,
                metrics.ascent * scale,
            ))
            .italic_angle(0.0)
            .ascent(metrics.ascent * scale)
            .descent(-metrics.descent * scale)
            .cap_height(metrics.cap_height * scale)
            .stem_v(80.0);
        if cff {
            writer.font_file3(file);
        } else {
            writer.font_file2(file);
        }
        writer.finish();
        let compressed = compress(&subset);
        let mut stream = self.pdf.stream(file, &compressed);
        stream.filter(Filter::FlateDecode);
        if cff {
            stream.pair(Name(b"Subtype"), Name(b"OpenType"));
        }
        stream.finish();
        let mut cmap = UnicodeCmap::new(Name(b"Custom"), info);
        for (glyph, text) in &font.text {
            cmap.pair_with_multiple(*glyph, text.chars());
        }
        let cmap = compress(&cmap.finish());
        self.pdf.stream(map, &cmap).filter(Filter::FlateDecode);
        Ok(())
    }
}
