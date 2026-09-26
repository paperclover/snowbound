use draw::{GlyphRun, Glyphs, RenderError};
use parley::{
    FontContext, FontFamily, FontFamilyName, GenericFamily, Layout, LayoutContext,
    PositionedLayoutItem, StyleProperty, fontique::Blob,
};
use std::{borrow::Cow, collections::HashMap, rc::Rc, sync::Arc};

/// A shaped single line of interface text, in logical pixels.
pub(crate) struct Label {
    pub layout: Layout<()>,
    pub size: [f32; 2],
}

/// Labels shaped this frame or the previous one, keyed by their text and size.
#[derive(Default)]
pub(crate) struct Texts {
    fonts: FontContext,
    context: LayoutContext<()>,
    /// A family registered in place of the system's interface font.
    family: Option<String>,
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
        let system = FontFamilyName::Generic(GenericFamily::SystemUi);
        builder.push_default(StyleProperty::FontFamily(match &self.family {
            Some(family) => {
                FontFamily::List(Cow::Owned(vec![FontFamilyName::named(family), system]))
            }
            None => FontFamily::Single(system),
        }));
        builder.push_default(StyleProperty::FontSize(size));
        let mut layout = builder.build(text);
        layout.break_all_lines(None);
        let size = [layout.full_width(), layout.height()];
        let label = Rc::new(Label { layout, size });
        self.cache.insert(key, (label.clone(), frame));
        label
    }

    /// Shapes labels in the family `files` define, falling back to the system's
    /// interface font; returns the family's name, or None when the files hold no font.
    pub fn use_fonts(&mut self, files: impl IntoIterator<Item = Vec<u8>>) -> Option<String> {
        let collection = &mut self.fonts.collection;
        let families: Vec<_> = files
            .into_iter()
            .flat_map(|file| collection.register_fonts(Blob::new(Arc::new(file)), None))
            .collect();
        let family = collection.family_name(families.first()?.0)?.to_owned();
        self.family = Some(family.clone());
        self.cache.clear();
        Some(family)
    }

    pub fn prune(&mut self, frame: u64) {
        self.cache.retain(|_, (_, touched)| *touched + 1 >= frame);
    }
}

impl Glyphs for Label {
    fn runs(
        &self,
        paint: &mut dyn FnMut(GlyphRun<'_>) -> Result<(), RenderError>,
    ) -> Result<(), RenderError> {
        for line in self.layout.lines() {
            let metrics = line.metrics();
            for item in line.items() {
                if let PositionedLayoutItem::GlyphRun(run) = item {
                    draw::paint_parley_run(
                        &run,
                        metrics.baseline,
                        0.0,
                        [metrics.block_min_coord, metrics.line_height],
                        |_| None,
                        paint,
                    )?;
                }
            }
        }
        Ok(())
    }
}
