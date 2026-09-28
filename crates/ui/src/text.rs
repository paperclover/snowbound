use draw::{GlyphRun, Glyphs, RenderError};
use parley::{
    Alignment, AlignmentOptions, Cluster, FontContext, FontFamily, FontFamilyName, GenericFamily,
    Layout, LayoutContext, OverflowWrap, PositionedLayoutItem, StyleProperty, fontique::Blob,
};
use std::{borrow::Cow, collections::HashMap, rc::Rc};

/// Shaped interface text, in logical pixels: a single line unless fitted to a width.
pub(crate) struct Label {
    pub layout: Layout<()>,
    pub size: [f32; 2],
    pub key: LabelKey,
}

type LabelKey = (String, u32, bool, Option<String>);

/// Labels shaped this frame or the previous one, keyed by their text and size.
#[derive(Default)]
pub(crate) struct Texts {
    fonts: FontContext,
    context: LayoutContext<()>,
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
        let key = (
            text.to_owned(),
            size.to_bits(),
            bold,
            font.map(str::to_owned),
        );
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
            .map(FontFamilyName::named)
            .chain(system)
            .collect();
        builder.push_default(StyleProperty::FontFamily(FontFamily::List(Cow::Owned(
            families,
        ))));
        builder.push_default(StyleProperty::FontSize(size));
        builder.push_default(StyleProperty::OverflowWrap(OverflowWrap::Anywhere));
        if bold {
            builder.push_default(StyleProperty::FontWeight(parley::FontWeight::SEMI_BOLD));
        }
        let mut layout = builder.build(text);
        layout.break_all_lines(None);
        let size = [layout.full_width(), layout.height()];
        let label = Rc::new(Label {
            layout,
            size,
            key: key.clone(),
        });
        self.cache.insert(key, (label.clone(), frame));
        label
    }

    /// `label` cut short to fit `width` with an ellipsis.
    pub fn ellipsis(&mut self, label: &Label, width: f32, frame: u64) -> Rc<Label> {
        let (text, size, bold, font) = &label.key;
        let (size, font) = (f32::from_bits(*size), font.as_deref());
        let room = width - self.label("…", size, *bold, font, frame).size[0];
        let mut end = 0;
        let mut advance = 0.0;
        let mut cluster = Cluster::from_byte_index(&label.layout, 0);
        while let Some(next) = cluster {
            advance += next.advance();
            if advance > room {
                break;
            }
            end = next.text_range().end;
            cluster = next.next_logical();
        }
        let cut = format!("{}…", text[..end].trim_end());
        self.label(&cut, size, *bold, font, frame)
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

impl Label {
    /// The text broken into lines no wider than `width` and aligned across it.
    pub fn wrapped(&self, width: f32, center: bool) -> Rc<Label> {
        let mut layout = self.layout.clone();
        layout.break_all_lines(Some(width));
        let alignment = if center {
            Alignment::Center
        } else {
            Alignment::Start
        };
        layout.align(alignment, AlignmentOptions::default());
        Rc::new(Label {
            size: [width, layout.height()],
            layout,
            key: self.key.clone(),
        })
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
