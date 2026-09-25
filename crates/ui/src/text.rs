use draw::{GlyphRun, Glyphs, RenderError};
use parley::{
    Affinity, FontContext, FontFamily, GenericFamily, Layout, LayoutContext, PositionedLayoutItem,
    StyleProperty, editing::Cursor,
};
use std::{collections::HashMap, rc::Rc};

/// A shaped single line of interface text, in logical pixels.
pub(crate) struct Label {
    layout: Layout<()>,
    pub size: [f32; 2],
}

impl Label {
    /// The caret's left edge before byte `index`.
    pub fn caret_x(&self, index: usize) -> f32 {
        Cursor::from_byte_index(&self.layout, index, Affinity::Downstream)
            .geometry(&self.layout, 1.0)
            .x0 as f32
    }

    /// The byte index whose caret position is nearest `x`.
    pub fn index_at(&self, x: f32) -> usize {
        Cursor::from_point(&self.layout, x, self.size[1] / 2.0).index()
    }
}

/// Labels shaped this frame or the previous one, keyed by their text and size.
#[derive(Default)]
pub(crate) struct Texts {
    fonts: FontContext,
    context: LayoutContext<()>,
    cache: HashMap<(String, u32), (Rc<Label>, u64)>,
}

impl Texts {
    /// `size` is in logical pixels.
    pub fn label(&mut self, text: &str, size: f32, frame: u64) -> Rc<Label> {
        let key = (text.to_owned(), size.to_bits());
        if let Some((label, touched)) = self.cache.get_mut(&key) {
            *touched = frame;
            return label.clone();
        }
        let mut builder = self
            .context
            .ranged_builder(&mut self.fonts, text, 1.0, false);
        builder.push_default(StyleProperty::FontFamily(FontFamily::from(
            GenericFamily::SystemUi,
        )));
        builder.push_default(StyleProperty::FontSize(size));
        let mut layout = builder.build(text);
        layout.break_all_lines(None);
        let size = [layout.full_width(), layout.height()];
        let label = Rc::new(Label { layout, size });
        self.cache.insert(key, (label.clone(), frame));
        label
    }

    pub fn prune(&mut self, frame: u64) {
        self.cache.retain(|_, (_, touched)| *touched + 1 >= frame);
    }
}

/// A label painted in one colour this frame.
pub(crate) struct Painted {
    pub label: Rc<Label>,
    pub color: [f32; 4],
}

impl Glyphs for Painted {
    fn runs(
        &self,
        paint: &mut dyn FnMut(GlyphRun<'_>) -> Result<(), RenderError>,
    ) -> Result<(), RenderError> {
        for line in self.label.layout.lines() {
            let metrics = line.metrics();
            for item in line.items() {
                if let PositionedLayoutItem::GlyphRun(run) = item {
                    draw::paint_parley_run(
                        &run,
                        metrics.baseline,
                        0.0,
                        [metrics.block_min_coord, metrics.line_height],
                        |_| self.color,
                        paint,
                    )?;
                }
            }
        }
        Ok(())
    }
}
