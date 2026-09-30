use crate::RenderError;
use parley::fontique::Blob;
use std::ops::Range;

/// Text the renderer can paint, as glyph runs in layer units. Owners recover their
/// concrete text from a primitive through `Any`.
pub trait Glyphs: std::any::Any {
    fn runs(
        &self,
        paint: &mut dyn FnMut(GlyphRun<'_>) -> Result<(), RenderError>,
    ) -> Result<(), RenderError>;
}

/// Glyphs sharing one font, size and colour.
pub struct GlyphRun<'a> {
    pub font: &'a Blob<u8>,
    /// The face within a font collection.
    pub index: u32,
    pub size: f32,
    /// Variation coordinates in F2Dot14 bits.
    pub coords: &'a [i16],
    pub embolden: bool,
    /// Synthesized oblique angle in degrees.
    pub skew: Option<f32>,
    /// `None` paints in the primitive's ink.
    pub color: Option<[f32; 4]>,
    /// The colour painted behind the glyphs, such as a highlight; `None` for the layer's
    /// backdrop.
    pub backdrop: Option<[f32; 4]>,
    /// Top and height of the run's line: lines outside the target are skipped, and
    /// colour glyphs shrink to fit.
    pub line: [f32; 2],
    pub glyphs: &'a mut dyn Iterator<Item = Glyph>,
    /// Underline and strikethrough, painted after the glyphs.
    pub decorations: &'a [Decoration],
    /// The source the glyphs show, for a document's text layer; the renderer ignores it.
    pub text: &'a str,
    /// Each cluster's bytes of `text` and how many glyphs show it, in `glyphs`' order; a
    /// ligature's later clusters take none.
    pub clusters: &'a mut dyn Iterator<Item = (Range<usize>, usize)>,
}

/// A glyph's baseline position.
#[derive(Clone, Copy)]
pub struct Glyph {
    pub id: u32,
    pub x: f32,
    pub y: f32,
}

/// A decoration line whose top edge is `y`; it stays at least one device pixel thick.
#[derive(Clone, Copy)]
pub struct Decoration {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub thickness: f32,
    /// `None` paints in the primitive's ink.
    pub color: Option<[f32; 4]>,
}

/// Paints one parley glyph run of `text` laid out, with its line's baseline moved to
/// `baseline`, its glyphs raised by `rise` and `backdrop` behind them.
#[allow(clippy::too_many_arguments)]
pub fn paint_parley_run<B: parley::Brush>(
    run: &parley::GlyphRun<'_, B>,
    text: &str,
    baseline: f32,
    rise: f32,
    line: [f32; 2],
    color: impl Fn(&B) -> Option<[f32; 4]>,
    backdrop: Option<[f32; 4]>,
    paint: &mut dyn FnMut(GlyphRun<'_>) -> Result<(), RenderError>,
) -> Result<(), RenderError> {
    let shaped = run.run();
    let font = &shaped.font().font;
    let coords: Vec<i16> = shaped
        .normalized_coords()
        .iter()
        .map(|c| c.to_bits())
        .collect();
    let synthesis = shaped.synthesis();
    let style = run.style();
    let metrics = shaped.font_metrics();
    let decorations: Vec<_> = [
        (
            &style.underline,
            metrics.underline_offset,
            metrics.underline_size,
        ),
        (
            &style.strikethrough,
            metrics.strikethrough_offset,
            metrics.strikethrough_size,
        ),
    ]
    .into_iter()
    .filter_map(|(decoration, offset, thickness)| {
        let decoration = decoration.as_ref()?;
        Some(Decoration {
            x: run.offset(),
            y: baseline - rise - decoration.offset.unwrap_or(offset),
            width: run.advance(),
            thickness: decoration.size.unwrap_or(thickness),
            color: color(&decoration.brush),
        })
    })
    .collect();
    let line_baseline = run.baseline();
    let mut glyphs = run.positioned_glyphs().map(|glyph| Glyph {
        id: glyph.id,
        x: glyph.x,
        y: glyph.y + baseline - line_baseline - rise,
    });
    // The run's clusters are those of its style within its span of the line; walked only
    // when a text layer asks.
    let [start, end] = [run.offset(), run.offset() + run.advance()];
    let mut at = None;
    let mut clusters = run.run().visual_clusters().filter_map(|cluster| {
        let x = *at.get_or_insert_with(|| cluster.visual_offset().unwrap_or(start));
        at = Some(x + cluster.advance());
        let within = x > start - 0.01 && x < end - 0.01;
        (cluster.style_index() == run.style_index() && within)
            .then(|| (cluster.text_range(), cluster.glyphs().count()))
    });
    paint(GlyphRun {
        font: &font.data,
        index: font.index,
        size: shaped.font_size(),
        coords: &coords,
        embolden: synthesis.embolden(),
        skew: synthesis.skew(),
        color: color(&style.brush),
        backdrop,
        line,
        glyphs: &mut glyphs,
        decorations: &decorations,
        text,
        clusters: &mut clusters,
    })
}
