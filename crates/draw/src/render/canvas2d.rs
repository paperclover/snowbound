//! Submission through the browser's 2D canvas, where it has neither WebGPU nor WebGL2. Each
//! quad of the frame becomes the canvas's own calls: shapes as paths, glyphs, icons and paths
//! from an atlas canvas, pictures and groups by `drawImage`. The canvas blends sRGB-encoded,
//! so a translucent colour's opacity is corrected toward what linear light shows.
//!
//! A group leaning back in perspective, which the canvas's affine transforms can't draw,
//! paints into a canvas of its own laid over the target and leaned by a CSS 3D transform
//! with the same projection; what paints after it goes into a canvas above that.
use super::*;
use std::{
    cell::{Cell, RefCell},
    f64::consts::{FRAC_PI_2, PI},
};
use wasm_bindgen::{Clamped, JsCast};
use web_sys::{
    CanvasRenderingContext2d as Context, CanvasWindingRule, HtmlCanvasElement as Canvas, ImageData,
};

/// The widest canvas every browser keeps whole.
const MAX_SIDE: u32 = 8192;
/// Most colours glyphs are kept coloured in at once.
const MAX_TINTS: usize = 16;
/// Most shadows kept drawn at once.
const MAX_SHADOWS: usize = 8;

/// A canvas and its 2D context, which a frame is drawn into.
#[derive(Clone)]
pub struct Target {
    canvas: Canvas,
    context: Context,
}

impl Target {
    pub fn new(canvas: Canvas) -> Result<Self, String> {
        let context = canvas
            .get_context("2d")
            .map_err(|error| format!("{error:?}"))?
            .ok_or("The canvas has no 2D context")?
            .dyn_into()
            .map_err(|_| "The canvas's context is not 2D")?;
        Ok(Self { canvas, context })
    }

    /// A canvas of its own, in no page, `size` pixels large.
    fn detached(size: [u32; 2]) -> Result<Self, String> {
        let canvas: Canvas = web_sys::window()
            .and_then(|window| window.document())
            .ok_or("No document")?
            .create_element("canvas")
            .map_err(|error| format!("{error:?}"))?
            .unchecked_into();
        let target = Self::new(canvas)?;
        target.resize(size);
        Ok(target)
    }

    pub fn size(&self) -> [u32; 2] {
        [self.canvas.width(), self.canvas.height()]
    }

    /// Resizes the canvas, which clears it, if it is another size.
    fn resize(&self, size: [u32; 2]) {
        if self.size() != size {
            self.canvas.set_width(size[0]);
            self.canvas.set_height(size[1]);
        }
    }
}

/// A picture as straight-alpha pixels in a canvas of its own, unless the browser had no
/// canvas to spare.
pub(super) struct Image(Option<Canvas>);

impl AsRef<Image> for Image {
    fn as_ref(&self) -> &Image {
        self
    }
}

/// Glyphs of one colour, coloured once in a sheet of their own that frames copy them from:
/// the browser copies a whole canvas drawn into after it was last drawn from, so colouring each
/// run of glyphs afresh, or the whole atlas, would copy a large canvas a frame.
struct Tint {
    color: [u32; 4],
    sheet: Target,
    /// Where each atlas rectangle `[left, top, right, bottom]` lies coloured in the sheet.
    glyphs: HashMap<[u32; 4], [u32; 2]>,
    /// Where the sheet's next glyph goes, and the height of the row it goes in.
    pen: [u32; 2],
    row: u32,
    /// The last frame drawing with it.
    used: u64,
}

impl Tint {
    /// Colours atlas rectangle `glyph` into the sheet, growing it or else emptying it when
    /// full; where even an empty sheet can't hold it, the glyph takes its colour as it draws.
    fn add(&mut self, atlas: &Canvas, glyph: [u32; 4]) {
        let color = css(self.color.map(f32::from_bits));
        let [width, height] = [glyph[2] - glyph[0], glyph[3] - glyph[1]];
        loop {
            let side = self.sheet.size()[0];
            if self.pen[0] + width > side {
                self.pen = [0, self.pen[1] + self.row + 1];
                self.row = 0;
            }
            if self.pen[1] + height <= side {
                break;
            }
            if side >= MAX_SIDE / 2 {
                if self.glyphs.is_empty() {
                    return;
                }
                self.glyphs.clear();
                self.sheet
                    .context
                    .clear_rect(0.0, 0.0, side.into(), side.into());
                (self.pen, self.row) = ([0; 2], 0);
                continue;
            }
            let Ok(grown) = Target::detached([2 * side; 2]) else {
                return;
            };
            let _ = grown
                .context
                .draw_image_with_html_canvas_element(&self.sheet.canvas, 0.0, 0.0);
            self.sheet = grown;
        }
        let [x, y] = self.pen.map(f64::from);
        let [w, h] = [width, height].map(f64::from);
        let context = &self.sheet.context;
        let [u, v] = [glyph[0], glyph[1]].map(f64::from);
        let _ = context
            .draw_image_with_html_canvas_element_and_sw_and_sh_and_dx_and_dy_and_dw_and_dh(
                atlas, u, v, w, h, x, y, w, h,
            );
        context.set_fill_style_str(&color);
        color_in(context, [x, y, w, h]);
        self.glyphs.insert(glyph, self.pen);
        self.pen[0] += width + 1;
        self.row = self.row.max(height);
    }
}

/// A shadow drawn once and copied while it stays the same, as a popup's does while it opens:
/// the browser may blur in software, slowly.
struct Shadow {
    /// The rectangle's half size, corner radius and blur, then the colour, as bits.
    key: Vec<u32>,
    sheet: Target,
    /// The last frame drawing it.
    used: u64,
}

pub(super) struct Gpu {
    atlas: Target,
    /// Whether the atlas was emptied since the last frame, as writing its white texel at the
    /// origin shows (`Renderer::clear_glyph_cache`): the tints' glyphs are gone.
    emptied: Cell<bool>,
    tints: Vec<Tint>,
    shadows: RefCell<Vec<Shadow>>,
    /// Frames submitted.
    frames: u64,
    /// Where glyphs of a colour no tint holds take it, together, before reaching the target.
    scratch: Target,
    /// Offscreen pictures for groups, the target's size.
    groups: Vec<Target>,
    /// Canvases over the target: in turn a group leaning back, then what paints above it.
    overlays: Vec<Target>,
}

impl Drop for Gpu {
    fn drop(&mut self) {
        for overlay in &self.overlays {
            overlay.canvas.remove();
        }
    }
}

impl Renderer {
    /// Draws with the browser's 2D canvas, into `Target`s.
    pub fn canvas2d() -> Result<Self, String> {
        Ok(Self::with_gpu(Gpu {
            atlas: Target::detached([ATLAS_SIZE; 2])?,
            emptied: Cell::new(false),
            tints: Vec::new(),
            shadows: RefCell::new(Vec::new()),
            frames: 0,
            scratch: Target::detached([1; 2])?,
            groups: Vec::new(),
            overlays: Vec::new(),
        }))
    }
}

impl Gpu {
    /// Blends sRGB-encoded, as the canvas does.
    pub(super) fn blends_linear(&self) -> bool {
        false
    }

    pub(super) fn flipped(&self) -> bool {
        false
    }

    pub(super) fn max_texture_dimension(&self) -> u32 {
        MAX_SIDE
    }

    pub(super) fn atlas_side(&self) -> u32 {
        self.atlas.canvas.width()
    }

    pub(super) fn new_atlas(&mut self, side: u32) {
        self.atlas.resize([side; 2]);
        self.tints.clear();
    }

    pub(super) fn write_atlas(&self, origin: [u32; 2], size: [u32; 2], rgba: &[u8]) {
        put(&self.atlas.context, origin, size, rgba);
        if origin == [0; 2] {
            self.emptied.set(true);
        }
    }

    /// Colours the glyphs `frame` draws in a colour into that colour's tint, keeping tints
    /// for as many colours as `MAX_TINTS` allows.
    fn tint(&mut self, frame: &Frame<'_>) {
        self.frames += 1;
        if self.emptied.take() {
            self.tints.clear();
        }
        let side = self.atlas_side() as f32;
        let batches = (frame.batches.iter())
            .chain(frame.groups.iter().flat_map(|group| &group.batches))
            .filter(|batch| batch.blend == Blend::Over);
        for batch in batches {
            let range = batch.vertices.start as usize..batch.vertices.end as usize;
            for quad in frame.vertices[range].chunks_exact(6) {
                let Some(color) = tinted(quad) else {
                    continue;
                };
                let key = color.map(f32::to_bits);
                let at = match self.tints.iter().position(|tint| tint.color == key) {
                    Some(at) => at,
                    None => {
                        let sheet = if self.tints.len() < MAX_TINTS {
                            match Target::detached([256; 2]) {
                                Ok(sheet) => sheet,
                                Err(error) => return report(&error),
                            }
                        } else if let Some(at) = (0..self.tints.len())
                            .filter(|at| self.tints[*at].used != self.frames)
                            .min_by_key(|at| self.tints[*at].used)
                        {
                            let sheet = self.tints.swap_remove(at).sheet;
                            let [width, height] = sheet.size().map(f64::from);
                            sheet.context.clear_rect(0.0, 0.0, width, height);
                            sheet
                        } else {
                            continue;
                        };
                        self.tints.push(Tint {
                            color: key,
                            sheet,
                            glyphs: HashMap::new(),
                            pen: [0; 2],
                            row: 0,
                            used: 0,
                        });
                        self.tints.len() - 1
                    }
                };
                let tint = &mut self.tints[at];
                tint.used = self.frames;
                let glyph = source(quad, side).map(|value| value as u32);
                if !tint.glyphs.contains_key(&glyph) {
                    tint.add(&self.atlas.canvas, glyph);
                }
            }
        }
    }

    /// The canvas holding shadow `key`, `size` pixels with the shadow in its middle, which
    /// `cast` draws where it isn't kept; none where `MAX_SHADOWS` are kept for this frame.
    fn shadow(
        &self,
        key: Vec<u32>,
        size: [u32; 2],
        cast: impl FnOnce(&Context, [f64; 2]),
    ) -> Option<Canvas> {
        let mut shadows = self.shadows.borrow_mut();
        if let Some(shadow) = shadows.iter_mut().find(|shadow| shadow.key == key) {
            shadow.used = self.frames;
            return Some(shadow.sheet.canvas.clone());
        }
        let sheet = if shadows.len() < MAX_SHADOWS {
            Target::detached(size).ok()?
        } else {
            let at = (0..shadows.len())
                .filter(|at| shadows[*at].used != self.frames)
                .min_by_key(|at| shadows[*at].used)?;
            let sheet = shadows.swap_remove(at).sheet;
            sheet.resize(size);
            let [width, height] = size.map(f64::from);
            sheet.context.clear_rect(0.0, 0.0, width, height);
            sheet
        };
        cast(&sheet.context, size.map(|side| f64::from(side) / 2.0));
        let canvas = sheet.canvas.clone();
        shadows.push(Shadow {
            key,
            sheet,
            used: self.frames,
        });
        Some(canvas)
    }

    pub(super) fn upload_image(&self, image: &RasterImage) -> Image {
        // The canvas takes straight alpha; the picture's colour was premultiplied in linear
        // light.
        let mut pixels = image.pixels().to_vec();
        for pixel in pixels.chunks_exact_mut(4) {
            let alpha = f32::from(pixel[3]) / 255.0;
            if alpha > 0.0 && alpha < 1.0 {
                for byte in &mut pixel[..3] {
                    *byte = srgb_byte(crate::linear(f32::from(*byte) / 255.0) / alpha);
                }
            }
        }
        match Target::detached(image.size) {
            Ok(picture) => {
                put(&picture.context, [0; 2], image.size, &pixels);
                Image(Some(picture.canvas))
            }
            Err(error) => {
                report(&error);
                Image(None)
            }
        }
    }

    /// The target's sRGB RGBA rows, top first.
    pub(super) fn read_pixels(&self, target: &Target) -> Result<Vec<u8>, String> {
        let [width, height] = target.size();
        let data = target
            .context
            .get_image_data(0.0, 0.0, width.into(), height.into())
            .map_err(|error| format!("{error:?}"))?;
        Ok(data.data().0)
    }

    /// Clears `target`, `size` device pixels, to linear `clear` and draws the frame.
    pub(super) fn submit(
        &mut self,
        frame: &Frame<'_>,
        target: &Target,
        size: [u32; 2],
        clear: [f32; 4],
    ) {
        target.resize(size);
        self.tint(frame);
        // Laid over the target only where it is in the page.
        let onscreen = target.canvas.parent_node().is_some();
        let leaning = |batch: &&Batch| matches!(batch.blend, Blend::Group(index) if frame.groups[index].motion.tilt != 0.0);
        let overlays = if onscreen {
            2 * frame.batches.iter().filter(leaning).count()
        } else {
            0
        };
        while self.groups.len() < frame.groups.len() || self.overlays.len() < overlays {
            let made = if self.groups.len() < frame.groups.len() {
                Target::detached(size).map(|picture| self.groups.push(picture))
            } else {
                overlay(self.overlays.last().unwrap_or(target)).map(|o| self.overlays.push(o))
            };
            if let Err(error) = made {
                report(&error);
                return;
            }
        }
        let gpu = &*self;
        let light = clear[3] == 0.0 || crate::encode(luminance(clear)) >= 0.5;
        let mut painter = Painter::new(gpu, frame, target.context.clone(), size, light);
        if clear[3] < 1.0 {
            painter
                .context
                .clear_rect(0.0, 0.0, size[0].into(), size[1].into());
        }
        if clear[3] > 0.0 {
            painter.context.set_fill_style_str(&css(clear));
            painter
                .context
                .fill_rect(0.0, 0.0, size[0].into(), size[1].into());
        }
        let mut used = 0;
        for batch in frame.batches {
            match batch.blend {
                Blend::Group(index) if onscreen && leaning(&batch) => {
                    painter.finish();
                    let [leaned, above] = [used, used + 1].map(|at| &gpu.overlays[at]);
                    leaned.resize(size);
                    let mut group = Painter::new(gpu, frame, leaned.context.clone(), size, light);
                    group
                        .context
                        .clear_rect(0.0, 0.0, size[0].into(), size[1].into());
                    group.batches(&frame.groups[index].batches);
                    group.finish();
                    lean(leaned, target, Some(frame.groups[index].motion), size);
                    above.resize(size);
                    lean(above, target, None, size);
                    painter = Painter::new(gpu, frame, above.context.clone(), size, light);
                    painter
                        .context
                        .clear_rect(0.0, 0.0, size[0].into(), size[1].into());
                    used += 2;
                }
                _ => painter.batch(batch),
            }
        }
        painter.finish();
        for overlay in &gpu.overlays[used..] {
            let style = overlay.canvas.style();
            if style.get_property_value("display").as_deref() != Ok("none") {
                let _ = style.set_property("display", "none");
                let _ = style.remove_property("transform");
                let _ = style.remove_property("opacity");
            }
        }
    }
}

/// The atlas rectangle `[left, top, right, bottom]` a quad copies, the atlas `side` texels
/// square.
fn source(quad: &[Vertex], side: f32) -> [f32; 4] {
    let [u0, v0] = quad[0].uv.map(|value| (value * side).round());
    let [u1, v1] = quad[2].uv.map(|value| (value * side).round());
    [u0, v0, u1, v1]
}

/// The one colour a quad copying a mask from the atlas paints it in, other than white, which
/// copies as it is.
fn tinted(quad: &[Vertex]) -> Option<[f32; 4]> {
    let vertex = &quad[0];
    let shape = vertex.blur.abs() > 1e-6 || (vertex.shape[0] > 0.0 && vertex.shape[1] > 0.0);
    let solid = quad[0].uv == quad[2].uv;
    let color = quad[0].color;
    (!shape && !solid && quad[1].color == color && color[..3] != [1.0; 3]).then_some(color)
}

/// Colours what `context` holds within `[x, y, width, height]` in its fill. Clipped there, as
/// colouring "source-in" clears the whole canvas outside what it fills.
fn color_in(context: &Context, [x, y, width, height]: [f64; 4]) {
    context.save();
    context.begin_path();
    context.rect(x, y, width, height);
    context.clip();
    context.set_global_composite_operation("source-in").ok();
    context.fill_rect(x, y, width, height);
    context.restore();
}

/// Writes straight-alpha `rgba`, `size` pixels, into `context` at `origin`.
fn put(context: &Context, origin: [u32; 2], size: [u32; 2], rgba: &[u8]) {
    let written = ImageData::new_with_u8_clamped_array_and_sh(Clamped(rgba), size[0], size[1])
        .and_then(|data| context.put_image_data(&data, origin[0].into(), origin[1].into()));
    if let Err(error) = written {
        report(&format!("{error:?}"));
    }
}

fn report(error: &str) {
    web_sys::console::error_1(&format!("Canvas 2D: {error}").into());
}

/// A canvas laid just after `previous` in the page, over it, taking no input.
fn overlay(previous: &Target) -> Result<Target, String> {
    let overlay = Target::detached([1; 2])?;
    let canvas = &overlay.canvas;
    canvas
        .set_attribute("aria-hidden", "true")
        .map_err(|error| format!("{error:?}"))?;
    let style = canvas.style();
    for (name, value) in [
        ("position", "fixed"),
        ("pointer-events", "none"),
        ("transform-origin", "0 0"),
        ("display", "none"),
    ] {
        style
            .set_property(name, value)
            .map_err(|error| format!("{error:?}"))?;
    }
    previous
        .canvas
        .after_with_node_1(canvas)
        .map_err(|error| format!("{error:?}"))?;
    Ok(overlay)
}

/// Shows `overlay` exactly over `target`, leaned back by `motion` as `Motion::project` leans a
/// group, or flat without one.
fn lean(overlay: &Target, target: &Target, motion: Option<Motion>, size: [u32; 2]) {
    let rect = target.canvas.get_bounding_client_rect();
    let [width, height] = [rect.width(), rect.height()];
    let style = overlay.canvas.style();
    let mut properties = vec![
        ("display", "block".to_owned()),
        ("left", format!("{}px", rect.left())),
        ("top", format!("{}px", rect.top())),
        ("width", format!("{width}px")),
        ("height", format!("{height}px")),
    ];
    if let Some(motion) = motion.filter(|_| width > 0.0 && height > 0.0) {
        // CSS pixels per device pixel, on each axis.
        let ratio = [width / f64::from(size[0]), height / f64::from(size[1])];
        let [x, y] = [0, 1].map(|axis| f64::from(motion.pivot[axis]) * ratio[axis]);
        let distance = f64::from(Motion::DISTANCE) * height;
        properties.push((
            "transform",
            format!(
                "translate({x}px, {y}px) perspective({distance}px) rotateX({}rad) \
                 translate({}px, {}px)",
                -motion.tilt, -x, -y
            ),
        ));
        properties.push(("opacity", motion.opacity.to_string()));
    } else {
        properties.push(("transform", "none".to_owned()));
        properties.push(("opacity", "1".to_owned()));
    }
    for (name, value) in properties {
        let _ = style.set_property(name, &value);
    }
}

/// Where a quad paints: within a scissor rectangle and, where it has one, a rounded rectangle's
/// middle, half size and corner radius.
#[derive(Clone, Copy, PartialEq)]
struct Clip {
    scissor: [u32; 4],
    round: Option<[f32; 5]>,
}

/// Glyph quads of one colour waiting to take it together: their device bounds, and each one's
/// atlas and device rectangles.
struct Run {
    colors: [[f32; 4]; 2],
    bounds: [f32; 4],
    quads: Vec<([f32; 4], [f32; 4])>,
}

/// Draws batches into one context, setting only the state that changes.
struct Painter<'a> {
    gpu: &'a Gpu,
    frame: &'a Frame<'a>,
    context: Context,
    size: [u32; 2],
    /// The clip in force; none paints everywhere.
    clip: Option<Clip>,
    composite: &'static str,
    alpha: f32,
    run: Option<Run>,
    /// Whether the frame clears to a light colour, which faded pictures most likely lie on.
    light: bool,
}

impl<'a> Painter<'a> {
    fn new(
        gpu: &'a Gpu,
        frame: &'a Frame<'a>,
        context: Context,
        size: [u32; 2],
        light: bool,
    ) -> Self {
        context.save();
        let _ = context.set_transform(1.0, 0.0, 0.0, 1.0, 0.0, 0.0);
        context.set_global_composite_operation("source-over").ok();
        context.set_global_alpha(1.0);
        context.set_image_smoothing_enabled(true);
        // Kept between clips: `restore` returns to this state.
        context.save();
        Self {
            gpu,
            frame,
            context,
            size,
            clip: None,
            composite: "source-over",
            alpha: 1.0,
            run: None,
            light,
        }
    }

    fn finish(&mut self) {
        self.flush();
        self.context.restore();
        self.context.restore();
    }

    fn batches(&mut self, batches: &[Batch]) {
        for batch in batches {
            self.batch(batch);
        }
    }

    fn batch(&mut self, batch: &Batch) {
        let range = batch.vertices.start as usize..batch.vertices.end as usize;
        let vertices = &self.frame.vertices[range];
        if let Blend::Group(index) = batch.blend {
            self.flush();
            self.set_clip(None);
            return self.group(index, vertices);
        }
        for quad in vertices.chunks_exact(6) {
            let clip = self.clip_of(batch.scissor, quad);
            self.set_clip(clip);
            match batch.blend {
                Blend::Image(id) => self.image(id, quad),
                Blend::Over => self.quad(quad, "source-over"),
                Blend::Erase => self.quad(quad, "destination-out"),
                Blend::Multiply => self.quad(quad, "multiply"),
                Blend::Group(_) => unreachable!("Groups paint above"),
            }
        }
    }

    /// What `quad` paints within: none where its scissor and rounded rectangle hold it all,
    /// as changing the canvas's clip costs.
    fn clip_of(&self, scissor: [u32; 4], quad: &[Vertex]) -> Option<Clip> {
        let corners = [&quad[0], &quad[1], &quad[2], &quad[5]].map(|vertex| self.device(vertex));
        let [mut left, mut top, mut right, mut bottom] = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
        for [x, y] in corners {
            [left, top] = [left.min(x), top.min(y)];
            [right, bottom] = [right.max(x), bottom.max(y)];
        }
        let within =
            |[x0, y0, x1, y1]: [f32; 4]| left >= x0 && top >= y0 && right <= x1 && bottom <= y1;
        let full = [0, 0, self.size[0], self.size[1]];
        let [x, y, width, height] = scissor.map(|value| value as f32);
        let scissor = if within([x, y, x + width, y + height]) {
            full
        } else {
            scissor
        };
        let vertex = &quad[0];
        let round = (vertex.clip[0] > 0.0)
            .then(|| {
                let [x, y] = self.device(vertex);
                [
                    x - vertex.clip_local[0],
                    y - vertex.clip_local[1],
                    vertex.clip[0],
                    vertex.clip[1],
                    vertex.clip[2],
                ]
                .map(snap)
            })
            .filter(|[x, y, half_x, half_y, radius]| {
                !within([
                    x - half_x,
                    y - half_y + radius,
                    x + half_x,
                    y + half_y - radius,
                ]) && !within([
                    x - half_x + radius,
                    y - half_y,
                    x + half_x - radius,
                    y + half_y,
                ])
            });
        (scissor != full || round.is_some()).then_some(Clip { scissor, round })
    }

    fn set_clip(&mut self, clip: Option<Clip>) {
        if self.clip == clip {
            return;
        }
        self.flush();
        self.context.restore();
        self.context.save();
        self.composite = "source-over";
        self.alpha = 1.0;
        self.clip = clip;
        let Some(Clip { scissor, round }) = clip else {
            return;
        };
        let context = &self.context;
        if scissor != [0, 0, self.size[0], self.size[1]] {
            context.begin_path();
            let [x, y, width, height] = scissor.map(f64::from);
            context.rect(x, y, width, height);
            context.clip();
        }
        if let Some([x, y, half_x, half_y, radius]) = round {
            let _ = context.set_transform(1.0, 0.0, 0.0, 1.0, x.into(), y.into());
            context.begin_path();
            rounded(context, [half_x, half_y], [radius; 2]);
            let _ = context.set_transform(1.0, 0.0, 0.0, 1.0, 0.0, 0.0);
            context.clip();
        }
    }

    fn set_blend(&mut self, composite: &'static str, alpha: f32) {
        if self.composite != composite {
            self.context.set_global_composite_operation(composite).ok();
            self.composite = composite;
        }
        if self.alpha != alpha {
            self.context.set_global_alpha(alpha.into());
            self.alpha = alpha;
        }
    }

    /// Device position of `vertex`, whole pixels kept whole.
    fn device(&self, vertex: &Vertex) -> [f32; 2] {
        let [width, height] = self.size.map(|side| side as f32);
        [
            snap((vertex.position[0] + 1.0) * width / 2.0),
            snap((1.0 - vertex.position[1]) * height / 2.0),
        ]
    }

    /// The device rectangle an axis-aligned quad covers.
    fn rect(&self, quad: &[Vertex]) -> [f32; 4] {
        let [left, top] = self.device(&quad[0]);
        let [right, bottom] = self.device(&quad[2]);
        [left, top, right, bottom]
    }

    fn image(&mut self, id: u64, quad: &[Vertex]) {
        self.flush();
        self.set_blend("source-over", 1.0);
        let Image(Some(picture)) = self.frame.image::<Image>(id) else {
            return;
        };
        let [left, top, right, bottom] = self.rect(quad).map(f64::from);
        let [width, height] = [picture.width(), picture.height()].map(f64::from);
        let _ = self
            .context
            .draw_image_with_html_canvas_element_and_sw_and_sh_and_dx_and_dy_and_dw_and_dh(
                picture,
                0.0,
                0.0,
                width,
                height,
                left,
                top,
                right - left,
                bottom - top,
            );
    }

    fn quad(&mut self, quad: &[Vertex], composite: &'static str) {
        let vertex = &quad[0];
        let colors = [quad[0].color, quad[1].color];
        let solid = quad[0].uv == quad[2].uv;
        let shape = vertex.blur.abs() > 1e-6 || (vertex.shape[0] > 0.0 && vertex.shape[1] > 0.0);
        if !shape && !solid {
            return self.glyph(quad, colors, composite);
        }
        self.flush();
        if shape {
            return self.shape(quad, colors, composite);
        }
        let [left, top, right, bottom] = self.rect(quad).map(f64::from);
        self.set_blend(composite, 1.0);
        self.fill(colors, [0.0, top, 0.0, bottom]);
        self.context
            .fill_rect(left, top, right - left, bottom - top);
    }

    /// Sets the fill and stroke to `colors`, shading from `colors[0]` at `span`'s top to
    /// `colors[1]` at its bottom.
    fn fill(&self, colors: [[f32; 4]; 2], span: [f64; 4]) {
        if colors[0] == colors[1] {
            let color = css(colors[0]);
            self.context.set_fill_style_str(&color);
            self.context.set_stroke_style_str(&color);
        } else {
            let gradient = self
                .context
                .create_linear_gradient(0.0, span[1], 0.0, span[3]);
            for (stop, color) in [0.0, 1.0].into_iter().zip(colors) {
                let _ = gradient.add_color_stop(stop, &css(color));
            }
            self.context.set_fill_style_canvas_gradient(&gradient);
            self.context.set_stroke_style_canvas_gradient(&gradient);
        }
    }

    /// A rounded rectangle, filled, stroked or dashed, a tapered capsule or a shadow, drawn in
    /// the quad's own frame.
    fn shape(&mut self, quad: &[Vertex], colors: [[f32; 4]; 2], composite: &'static str) {
        let [a, b, d] = [&quad[0], &quad[1], &quad[5]];
        let [corner, below, across] = [a, b, d].map(|vertex| self.device(vertex));
        let [span_x, span_y] = [d.local[0] - a.local[0], b.local[1] - a.local[1]];
        if span_x == 0.0 || span_y == 0.0 {
            return;
        }
        let x_axis = [0, 1].map(|axis| f64::from((across[axis] - corner[axis]) / span_x));
        let y_axis = [0, 1].map(|axis| f64::from((below[axis] - corner[axis]) / span_y));
        let origin = [0, 1].map(|axis| {
            f64::from(corner[axis])
                - x_axis[axis] * f64::from(a.local[0])
                - y_axis[axis] * f64::from(a.local[1])
        });
        self.set_blend(composite, 1.0);
        let context = &self.context;
        let _ = context.set_transform(
            x_axis[0], x_axis[1], y_axis[0], y_axis[1], origin[0], origin[1],
        );
        let span = [0.0, a.local[1], 0.0, b.local[1]].map(f64::from);
        let [half_x, half_y, radius_x, radius_y] = a.shape;
        context.begin_path();
        if a.blur > 1e-6 {
            let _ = context.set_transform(1.0, 0.0, 0.0, 1.0, 0.0, 0.0);
            let color = colors[0];
            // A wide blur loses nothing drawn smaller and enlarged, which a browser blurring
            // in software does many times faster.
            let scale = (a.blur / 3.0).floor().clamp(1.0, 4.0);
            let [half, radius, sigma] = [[half_x, half_y], [radius_x; 2], [a.blur; 2]]
                .map(|values| values.map(|value| value / scale));
            let reach = (3.0 * sigma[0]).ceil() + 2.0;
            let size = half.map(|half| (2.0 * (half + reach)).ceil() as u32);
            let key = (half.into_iter().chain([radius[0], sigma[0]]).chain(color))
                .map(f32::to_bits)
                .collect::<Vec<_>>();
            let small = |context: &Context, middle: [f64; 2]| {
                cast(context, middle, half, radius[0], sigma[0], color);
            };
            match self.gpu.shadow(key, size, small) {
                Some(sheet) => {
                    let [width, height] = size.map(|side| f64::from(side as f32 * scale));
                    let _ = context.draw_image_with_html_canvas_element_and_dw_and_dh(
                        &sheet,
                        origin[0] - width / 2.0,
                        origin[1] - height / 2.0,
                        width,
                        height,
                    );
                }
                None => cast(context, origin, [half_x, half_y], radius_x, a.blur, color),
            }
        } else if a.blur < -0.5 {
            self.fill(colors, span);
            taper(context, half_x, half_y, radius_x);
            context.fill();
        } else {
            self.fill(colors, span);
            let width = a.stroke.abs();
            let radii = [radius_x, radius_y];
            if a.stroke < -1e-6 {
                let middle = radii.map(|radius| (radius - width / 2.0).max(0.0));
                let middle = if middle.contains(&0.0) {
                    [0.0; 2]
                } else {
                    middle
                };
                rounded(
                    context,
                    [half_x - width / 2.0, half_y - width / 2.0],
                    middle,
                );
                let dash = js_sys::Array::of2(&(2.0 * width).into(), &(2.0 * width).into());
                let _ = context.set_line_dash(&dash);
                context.set_line_dash_offset(-f64::from(width));
                context.set_line_width(width.into());
                context.stroke();
                let _ = context.set_line_dash(&js_sys::Array::new());
            } else {
                rounded(context, [half_x, half_y], radii);
                if width > 1e-6 && width < half_x.min(half_y) {
                    let inner = radii.map(|radius| (radius - width).max(0.0));
                    rounded(context, [half_x - width, half_y - width], inner);
                    context.fill_with_canvas_winding_rule(CanvasWindingRule::Evenodd);
                } else {
                    context.fill();
                }
            }
        }
        let _ = context.set_transform(1.0, 0.0, 0.0, 1.0, 0.0, 0.0);
    }

    /// A glyph, icon or path from the atlas: drawn as it is where white, else gathered with
    /// glyphs of its colour.
    fn glyph(&mut self, quad: &[Vertex], colors: [[f32; 4]; 2], composite: &'static str) {
        let source = source(quad, self.gpu.atlas_side() as f32);
        let rect = self.rect(quad);
        let white = colors[0] == colors[1] && colors[0][..3] == [1.0; 3];
        if composite != "source-over" || white {
            self.flush();
            // A faded icon's colours are its own; it fades as a mid grey would over the
            // frame's light or dark colour.
            let grey = if self.light { 0.18 } else { 0.5 };
            let alpha = if white {
                opacity([grey, grey, grey, colors[0][3]])
            } else {
                colors[0][3]
            };
            self.set_blend(composite, alpha);
            return self.copy(&self.gpu.atlas.canvas, &self.context, source, rect);
        }
        if let Some(color) = tinted(quad)
            && let Some(tint) =
                (self.gpu.tints.iter()).find(|tint| tint.color == color.map(f32::to_bits))
            && let Some([x, y]) = tint.glyphs.get(&source.map(|value| value as u32))
        {
            self.flush();
            self.set_blend(composite, 1.0);
            let [x, y] = [*x, *y].map(|value| value as f32);
            let colored = [x, y, x + source[2] - source[0], y + source[3] - source[1]];
            return self.copy(&tint.sheet.canvas, &self.context, colored, rect);
        }
        if let Some(run) = &mut self.run
            && run.colors == colors
            && colors[0] == colors[1]
        {
            run.bounds = [
                run.bounds[0].min(rect[0]),
                run.bounds[1].min(rect[1]),
                run.bounds[2].max(rect[2]),
                run.bounds[3].max(rect[3]),
            ];
            run.quads.push((source, rect));
            return;
        }
        self.flush();
        self.run = Some(Run {
            colors,
            bounds: rect,
            quads: vec![(source, rect)],
        });
    }

    /// Copies `source` of `sheet`, the atlas or a tint of it, to `rect` of `context`.
    fn copy(&self, sheet: &Canvas, context: &Context, source: [f32; 4], rect: [f32; 4]) {
        let [u0, v0, u1, v1] = source.map(f64::from);
        let [left, top, right, bottom] = rect.map(f64::from);
        let _ = context
            .draw_image_with_html_canvas_element_and_sw_and_sh_and_dx_and_dy_and_dw_and_dh(
                sheet,
                u0,
                v0,
                u1 - u0,
                v1 - v0,
                left,
                top,
                right - left,
                bottom - top,
            );
    }

    /// Paints the glyphs waiting for their colour: their coverage gathered in the scratch
    /// canvas, coloured there, then laid over the target.
    fn flush(&mut self) {
        let Some(run) = self.run.take() else {
            return;
        };
        let [width, height] = self.size.map(|side| side as f32);
        let [left, top] = [
            run.bounds[0].max(0.0).floor(),
            run.bounds[1].max(0.0).floor(),
        ];
        let [right, bottom] = [
            run.bounds[2].min(width).ceil(),
            run.bounds[3].min(height).ceil(),
        ];
        if right <= left || bottom <= top {
            return;
        }
        let area = [left, top, right - left, bottom - top].map(f64::from);
        // As small as the run, as the browser copies the whole scratch canvas each time it is
        // drawn from after being drawn into.
        let [wide, tall] = [right - left, bottom - top].map(|side| side as u32);
        let scratch = &self.gpu.scratch;
        let [have_wide, have_tall] = scratch.size();
        if have_wide < wide || have_tall < tall {
            scratch.resize(
                [wide.max(have_wide), tall.max(have_tall)].map(|side| side.next_multiple_of(64)),
            );
        }
        let scratch = &scratch.context;
        scratch.clear_rect(0.0, 0.0, area[2], area[3]);
        let _ = scratch.set_transform(1.0, 0.0, 0.0, 1.0, -area[0], -area[1]);
        for (source, rect) in &run.quads {
            self.copy(&self.gpu.atlas.canvas, scratch, *source, *rect);
        }
        let [top_color, bottom_color] = run.colors;
        if top_color == bottom_color {
            scratch.set_fill_style_str(&css(top_color));
        } else {
            let gradient = scratch.create_linear_gradient(
                0.0,
                run.bounds[1].into(),
                0.0,
                run.bounds[3].into(),
            );
            let _ = gradient.add_color_stop(0.0, &css(top_color));
            let _ = gradient.add_color_stop(1.0, &css(bottom_color));
            scratch.set_fill_style_canvas_gradient(&gradient);
        }
        color_in(scratch, area);
        let _ = scratch.set_transform(1.0, 0.0, 0.0, 1.0, 0.0, 0.0);
        self.set_blend("source-over", 1.0);
        let _ = self
            .context
            .draw_image_with_html_canvas_element_and_sw_and_sh_and_dx_and_dy_and_dw_and_dh(
                &self.gpu.scratch.canvas,
                0.0,
                0.0,
                area[2],
                area[3],
                area[0],
                area[1],
                area[2],
                area[3],
            );
    }

    /// Paints `group` offscreen, then its picture over the target, faded. One leaning back
    /// leans in a canvas of its own (`lean`) where the target is in the page, and lies flat on
    /// one that isn't, as the canvas has no perspective.
    fn group(&mut self, index: usize, vertices: &[Vertex]) {
        let group = &self.frame.groups[index];
        let picture = &self.gpu.groups[index];
        picture.resize(self.size);
        let [width, height] = self.size.map(|side| side as f32);
        // The picture's painted bounds, as the group's batch samples it.
        let [mut left, mut top, mut right, mut bottom] = [width, height, 0.0, 0.0];
        for vertex in vertices {
            let [x, y] = [vertex.uv[0] * width, vertex.uv[1] * height];
            [left, top] = [left.min(x), top.min(y)];
            [right, bottom] = [right.max(x), bottom.max(y)];
        }
        if right <= left || bottom <= top {
            return;
        }
        let mut painter = Painter::new(
            self.gpu,
            self.frame,
            picture.context.clone(),
            self.size,
            self.light,
        );
        painter
            .context
            .clear_rect(0.0, 0.0, width.into(), height.into());
        painter.batches(&group.batches);
        painter.finish();
        self.set_blend("source-over", group.motion.opacity);
        let [x, y, w, h] = [left, top, right - left, bottom - top].map(f64::from);
        let _ = self
            .context
            .draw_image_with_html_canvas_element_and_sw_and_sh_and_dx_and_dy_and_dw_and_dh(
                &picture.canvas,
                x,
                y,
                w,
                h,
                x,
                y,
                w,
                h,
            );
    }
}

/// `value`, made whole within a thousandth of a whole number, as quads placed on whole pixels
/// are, so pictures on them copy without resampling.
fn snap(value: f32) -> f32 {
    if (value - value.round()).abs() < 1e-3 {
        value.round()
    } else {
        value
    }
}

/// Draws the shadow a rounded rectangle about `middle`, `half` its half size, casts: blurred
/// by `sigma` device pixels, in `color`.
fn cast(
    context: &Context,
    middle: [f64; 2],
    half: [f32; 2],
    radius: f32,
    sigma: f32,
    color: [f32; 4],
) {
    // The canvas's shadow is a gaussian of half its blur as deviation, as `sigma` is here; its
    // offset and blur ignore the transform. The rectangle itself lies far off the canvas.
    let side = context
        .canvas()
        .map_or(0, |canvas| canvas.width().max(canvas.height()));
    let away = 4.0 * f64::from(side) + 8.0 * f64::from(sigma);
    let _ = context.set_transform(1.0, 0.0, 0.0, 1.0, middle[0] - away, middle[1]);
    context.begin_path();
    rounded(context, half, [radius; 2]);
    context.set_shadow_color(&css(color));
    context.set_shadow_blur(2.0 * f64::from(sigma));
    context.set_shadow_offset_x(away);
    context.set_fill_style_str("#000");
    context.fill();
    context.set_shadow_color("transparent");
    context.set_shadow_blur(0.0);
    context.set_shadow_offset_x(0.0);
    let _ = context.set_transform(1.0, 0.0, 0.0, 1.0, 0.0, 0.0);
}

/// Adds a rounded rectangle about the origin, `half` its half size, to the path: clockwise
/// from the middle of its right edge, which is where a dashed outline's pattern starts.
fn rounded(context: &Context, half: [f32; 2], radius: [f32; 2]) {
    let [x, y] = half.map(f64::from);
    if radius.iter().any(|radius| *radius <= 0.0) {
        context.rect(-x, -y, 2.0 * x, 2.0 * y);
        return;
    }
    let [rx, ry] = [0, 1].map(|axis| f64::from(radius[axis].min(half[axis])));
    context.move_to(x, 0.0);
    for (corner, from) in [
        ([x - rx, y - ry], 0.0),
        ([-x + rx, y - ry], FRAC_PI_2),
        ([-x + rx, -y + ry], PI),
        ([x - rx, -y + ry], 3.0 * FRAC_PI_2),
    ] {
        let _ = context.ellipse(corner[0], corner[1], rx, ry, 0.0, from, from + FRAC_PI_2);
    }
    context.close_path();
}

/// Adds the tapered capsule from `(-half, 0)` to `(half, 0)`, round ends `radii` across, to
/// the path: the two circles and the lines touching both.
fn taper(context: &Context, half: f32, start: f32, end: f32) {
    let [half, start, end] = [half, start, end].map(f64::from);
    let slope = (start - end) / (2.0 * half).max(1e-6);
    if slope.abs() >= 1.0 {
        let (x, radius) = if start >= end {
            (-half, start)
        } else {
            (half, end)
        };
        let _ = context.arc(x, 0.0, radius, 0.0, 2.0 * PI);
        return;
    }
    // Where the touching lines meet each circle, by their outward normal's angle.
    let angle = (1.0 - slope * slope).sqrt().atan2(slope);
    context.move_to(-half + start * angle.cos(), start * angle.sin());
    let _ = context.arc_with_anticlockwise(half, 0.0, end, angle, -angle, true);
    let _ = context.arc_with_anticlockwise(-half, 0.0, start, -angle, angle, true);
    context.close_path();
}

/// A CSS colour for linear `color`, its opacity corrected for the canvas's sRGB blending.
fn css(color: [f32; 4]) -> String {
    let [red, green, blue] = srgb_bytes(color);
    format!("rgba({red},{green},{blue},{:.4})", opacity(color))
}

/// The opacity that, blended sRGB-encoded as the canvas blends, shows `color` as linear light
/// blends it over what it stands out against most: white under a dark colour, black under a
/// light one.
fn opacity([red, green, blue, alpha]: [f32; 4]) -> f32 {
    if alpha <= 0.0 || alpha >= 1.0 {
        return alpha.clamp(0.0, 1.0);
    }
    let ink = luminance([red, green, blue, alpha]);
    let shown = crate::encode(ink);
    let corrected = if shown < 0.5 {
        (1.0 - crate::encode(1.0 - alpha * (1.0 - ink))) / (1.0 - shown)
    } else {
        crate::encode(alpha * ink) / shown
    };
    corrected.clamp(0.0, 1.0)
}

fn luminance([red, green, blue, _]: [f32; 4]) -> f32 {
    0.2126 * red + 0.7152 * green + 0.0722 * blue
}
