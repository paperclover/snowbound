use crate::RenderError;
use parley::fontique::Blob;

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

/// Paints one parley glyph run with its line's baseline moved to `baseline`, its glyphs
/// raised by `rise` and `backdrop` behind them.
pub fn paint_parley_run<B: parley::Brush>(
    run: &parley::GlyphRun<'_, B>,
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
    })
}
