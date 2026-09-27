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

type LabelKey = (String, u32, bool, Option<String>);

/// Labels shaped this frame or the previous one, keyed by their text and size.
#[derive(Default)]
pub(crate) struct Texts {
    fonts: FontContext,
    context: LayoutContext<()>,
    /// A family registered in place of the system's interface font.
    family: Option<String>,
    /// By text, size, weight and preferred family, with the frame each was last used.
    cache: HashMap<LabelKey, (Rc<Label>, u64)>,
}

impl Texts {
    /// `size` is in logical pixels; `font` names a family to prefer over the interface's.
    pub fn label(
        &mut self,
        text: &str,
        size: f32,
        bold: bool,
        font: Option<&str>,
        frame: u64,
    ) -> Rc<Label> {
        let key = (text.to_owned(), size.to_bits(), bold, font.map(str::to_owned));
        if let Some((label, touched)) = self.cache.get_mut(&key) {
            *touched = frame;
            return label.clone();
        }
        let mut builder = self
            .context
            .ranged_builder(&mut self.fonts, text, 1.0, false);
        // Fontconfig configurations without a system-ui alias still have a sans-serif.
        let system =
            [GenericFamily::SystemUi, GenericFamily::SansSerif].map(FontFamilyName::Generic);
        let families: Vec<_> = font
            .into_iter()
            .chain(self.family.as_deref())
            .map(FontFamilyName::named)
            .chain(system)
            .collect();
        builder.push_default(StyleProperty::FontFamily(FontFamily::List(Cow::Owned(
            families,
        ))));
        builder.push_default(StyleProperty::FontSize(size));
        if bold {
            builder.push_default(StyleProperty::FontWeight(parley::FontWeight::SEMI_BOLD));
        }
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

    pub fn preview_font(&mut self, data: Blob<u8>, family: &str) {
        self.fonts.collection.register_fonts(
            data,
            Some(parley::fontique::FontInfoOverride {
                family_name: Some(family),
                ..Default::default()
            }),
        );
        self.cache.clear();
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
                        None,
                        paint,
                    )?;
                }
            }
        }
        Ok(())
    }
}
