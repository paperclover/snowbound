use onestore::page::text::Paragraph;
use parley::{
    Affinity, Alignment, AlignmentOptions, BoundingBox, FontContext, FontFamily, FontFamilyName,
    FontStyle, FontWeight, GenericFamily, Layout, LayoutContext, OverflowWrap,
    PositionedLayoutItem, StyleProperty,
    editing::{Cursor, Selection},
};
use skrifa::{FontRef, MetadataProvider, raw::TableProvider, string::StringId};
use std::{
    collections::BTreeSet,
    fmt,
    ops::Range,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

#[derive(Default)]
pub struct TextEngine {
    pub fonts: FontContext,
    context: LayoutContext<TextBrush>,
    arial_substitutes: BTreeSet<u64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TextBrush {
    pub color: u32,
    pub highlight: Option<u32>,
}

#[derive(Clone, Debug)]
pub struct LineBox {
    pub source: Range<usize>,
    pub top: f32,
    pub baseline: f32,
    pub height: f32,
}

#[derive(Clone)]
pub struct TextLayout {
    id: u64,
    pub(crate) shaped: Arc<Layout<TextBrush>>,
    lines: Vec<LineBox>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutError {
    InvalidWidth,
    InvalidFontSize,
    InvalidSourceRange,
    InvalidFontMetrics,
    InvalidIndentation,
    InvalidSpacing,
    InvalidList,
    UnsupportedContent,
    UnsupportedSubstituteFont,
}

impl fmt::Display for LayoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedSubstituteFont => write!(f, "Choose an Arimo or Carlito font file."),
            _ => write!(f, "Text layout failed: {self:?}"),
        }
    }
}

impl std::error::Error for LayoutError {}

impl TextEngine {
    /// Register Arimo as Arial or Carlito as Calibri before creating layouts.
    pub fn register_substitute(
        &mut self,
        data: parley::fontique::Blob<u8>,
    ) -> Result<&'static str, LayoutError> {
        let font =
            FontRef::new(data.as_ref()).map_err(|_| LayoutError::UnsupportedSubstituteFont)?;
        let family = font
            .localized_strings(StringId::TYPOGRAPHIC_FAMILY_NAME)
            .english_or_first()
            .or_else(|| {
                font.localized_strings(StringId::FAMILY_NAME)
                    .english_or_first()
            })
            .map(|name| name.to_string())
            .ok_or(LayoutError::UnsupportedSubstituteFont)?;
        let target = match family.as_str() {
            "Arimo" => "Arial",
            "Carlito" => "Calibri",
            _ => return Err(LayoutError::UnsupportedSubstituteFont),
        };
        let id = data.id();
        if target == "Arial" {
            let head = font.head().map_err(|_| LayoutError::InvalidFontMetrics)?;
            let hhea = font.hhea().map_err(|_| LayoutError::InvalidFontMetrics)?;
            if head.units_per_em() == 0
                || hhea.ascender().to_i16() <= 0
                || hhea.descender().to_i16() > 0
            {
                return Err(LayoutError::InvalidFontMetrics);
            }
        }
        let registered = self.fonts.collection.register_fonts(
            data,
            Some(parley::fontique::FontInfoOverride {
                family_name: Some(target),
                ..Default::default()
            }),
        );
        if registered.is_empty() {
            return Err(LayoutError::UnsupportedSubstituteFont);
        }
        if target == "Arial" {
            self.arial_substitutes.insert(id);
        }
        Ok(target)
    }

    fn shape(
        &mut self,
        paragraph: &Paragraph,
        width: f32,
    ) -> Result<Layout<TextBrush>, LayoutError> {
        if !width.is_finite() || width <= 0.0 {
            return Err(LayoutError::InvalidWidth);
        }
        let text = paragraph.text();
        let mut builder = self
            .context
            .ranged_builder(&mut self.fonts, text, 1.0, false);
        builder.push_default(StyleProperty::OverflowWrap(OverflowWrap::BreakWord));
        // OneNote 2010 uses unkerned advances for text and table widths.
        builder.push_default(StyleProperty::FontFeatures(r#""kern" 0"#.into()));
        let mut start = 0;
        for (index, span) in paragraph.spans().iter().enumerate() {
            let format = &span.format;
            let link = format.hyperlink == Some(true);
            let size = format.font_size.unwrap_or(11.0);
            if !size.is_finite() || size <= 0.0 {
                return Err(LayoutError::InvalidFontSize);
            }
            let properties = [
                StyleProperty::FontFamily(FontFamily::List(
                    [
                        Some(FontFamilyName::named(
                            format.font.as_deref().unwrap_or("Arial"),
                        )),
                        // macOS ships STIX Two Math where Windows has Cambria Math.
                        (format.font.as_deref() == Some("Cambria Math"))
                            .then_some(FontFamilyName::named("STIX Two Math")),
                        // A missing family otherwise falls back per script and can put digits in an emoji font.
                        Some(GenericFamily::SansSerif.into()),
                    ]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .into(),
                )),
                StyleProperty::FontSize(size),
                StyleProperty::FontWeight(if format.bold == Some(true) {
                    FontWeight::BOLD
                } else {
                    FontWeight::NORMAL
                }),
                StyleProperty::FontStyle(if format.italic == Some(true) {
                    FontStyle::Italic
                } else {
                    FontStyle::Normal
                }),
                StyleProperty::Brush(TextBrush {
                    // Links take OneNote's blue unless given a colour of their own.
                    color: match format.color {
                        Some(color) if color != 0xff000000 => color,
                        _ if link => 0x00ff0000,
                        _ => 0,
                    },
                    highlight: format.highlight.filter(|color| *color != 0xff000000),
                }),
                StyleProperty::Underline(format.underline.unwrap_or(false) || link),
                StyleProperty::Strikethrough(format.strike.unwrap_or(false)),
            ];
            for property in properties {
                if index == 0 {
                    builder.push_default(property.clone());
                }
                builder.push(property, start..span.end);
            }
            start = span.end;
        }
        let mut shaped = builder.build(text);
        shaped.break_all_lines(Some(width));
        let alignment = match paragraph.spans()[0].format.alignment {
            Some(1) => Alignment::Center,
            Some(2) => Alignment::End,
            _ => Alignment::Start,
        };
        shaped.align(alignment, AlignmentOptions::default());
        Ok(shaped)
    }

    fn font_extents(
        &self,
        run: parley::layout::Run<'_, TextBrush>,
    ) -> Result<(f32, f32), LayoutError> {
        let data = &run.font().font;
        let font = FontRef::from_index(data.data.as_ref(), data.index)
            .map_err(|_| LayoutError::InvalidFontMetrics)?;
        Ok(if self.arial_substitutes.contains(&data.data.id()) {
            // Arimo's hhea extents match Arial's Windows extents; omit hhea line gap.
            let head = font.head().map_err(|_| LayoutError::InvalidFontMetrics)?;
            let hhea = font.hhea().map_err(|_| LayoutError::InvalidFontMetrics)?;
            let scale = run.font_size() / f32::from(head.units_per_em());
            (
                f32::from(hhea.ascender().to_i16()) * scale,
                -f32::from(hhea.descender().to_i16()) * scale,
            )
        } else if let (Ok(head), Ok(os2)) = (font.head(), font.os2())
            && head.units_per_em() != 0
            && (os2.us_win_ascent() != 0 || os2.us_win_descent() != 0)
        {
            let scale = run.font_size() / f32::from(head.units_per_em());
            (
                f32::from(os2.us_win_ascent()) * scale,
                f32::from(os2.us_win_descent()) * scale,
            )
        } else {
            let metrics = run.font_metrics();
            (metrics.ascent, metrics.descent)
        })
    }

    pub fn layout(&mut self, paragraph: &Paragraph, width: f32) -> Result<TextLayout, LayoutError> {
        let shaped = self.shape(paragraph, width)?;
        let text = paragraph.text();
        let mut lines = Vec::with_capacity(shaped.len());
        let mut top = 0.0_f64;
        let mut source_end = 0;
        for line in shaped.lines() {
            if !line.metrics().advance.is_finite()
                || !line.metrics().trailing_whitespace.is_finite()
            {
                return Err(LayoutError::InvalidFontMetrics);
            }
            // Parley gives its synthetic empty line a 0..1 range.
            let range = if text.is_empty() {
                0..0
            } else {
                line.text_range()
            };
            if range.start != source_end
                || range.end < range.start
                || !text.is_char_boundary(range.end)
            {
                return Err(LayoutError::InvalidSourceRange);
            }
            source_end = range.end;
            let mut ascent = 0.0_f32;
            let mut descent = 0.0_f32;
            for run in line.runs() {
                let data = &run.font().font;
                let font = FontRef::from_index(data.data.as_ref(), data.index)
                    .map_err(|_| LayoutError::InvalidFontMetrics)?;
                // Color glyphs fit the source font's line box instead of shifting annotation rows.
                let (a, d) = if font.colr().is_ok() || font.sbix().is_ok() || font.cbdt().is_ok() {
                    let format = &paragraph
                        .spans()
                        .iter()
                        .find(|span| span.end > run.text_range().start)
                        .unwrap_or_else(|| paragraph.spans().last().unwrap())
                        .format;
                    let sample =
                        self.shape(&Paragraph::new("Mg".into(), format.clone()), f32::MAX)?;
                    sample.lines().flat_map(|line| line.runs()).try_fold(
                        (0.0_f32, 0.0_f32),
                        |(a, d), run| {
                            let (next_a, next_d) = self.font_extents(run)?;
                            Ok::<_, LayoutError>((a.max(next_a), d.max(next_d)))
                        },
                    )?
                } else {
                    self.font_extents(run)?
                };
                if !a.is_finite() || !d.is_finite() || a < 0.0 || d < 0.0 {
                    return Err(LayoutError::InvalidFontMetrics);
                }
                ascent = ascent.max(a);
                descent = descent.max(d);
            }
            let height = ascent + descent;
            if height <= 0.0 || !height.is_finite() {
                return Err(LayoutError::InvalidFontMetrics);
            }
            lines.push(LineBox {
                source: range,
                top: top as f32,
                baseline: (top + f64::from(ascent)) as f32,
                height,
            });
            top += f64::from(height);
            if !(top as f32).is_finite() {
                return Err(LayoutError::InvalidFontMetrics);
            }
        }
        if source_end != text.len() || lines.is_empty() {
            return Err(LayoutError::InvalidSourceRange);
        }
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        let id = NEXT_ID
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .expect("Text layout identities exhausted");
        Ok(TextLayout {
            id,
            shaped: Arc::new(shaped),
            lines,
        })
    }
}

impl TextLayout {
    /// Process-unique identity for caches of an externally immutable layout.
    pub fn id(&self) -> u64 {
        self.id
    }

    pub(crate) fn minimum_line_height(&mut self, minimum: f32) -> Result<(), LayoutError> {
        if !minimum.is_finite() || minimum < 0.0 {
            return Err(LayoutError::InvalidSpacing);
        }
        if minimum == 0.0 {
            return Ok(());
        }
        let mut top = 0.0_f64;
        for line in &mut self.lines {
            let ascent = line.baseline - line.top;
            line.height = line.height.max(minimum);
            line.top = top as f32;
            line.baseline = (top + f64::from(ascent)) as f32;
            top += f64::from(line.height);
            if !(top as f32).is_finite() {
                return Err(LayoutError::InvalidSpacing);
            }
        }
        Ok(())
    }

    pub fn lines(&self) -> impl Iterator<Item = (parley::Line<'_, TextBrush>, &LineBox)> {
        self.shaped.lines().zip(&self.lines)
    }

    pub fn backgrounds(&self) -> impl Iterator<Item = (BoundingBox, u32)> {
        self.lines().flat_map(|(line, bounds)| {
            line.items().filter_map(move |item| {
                let PositionedLayoutItem::GlyphRun(run) = item else {
                    return None;
                };
                let color = run.style().brush.highlight?;
                Some((
                    BoundingBox {
                        x0: f64::from(run.offset()),
                        y0: f64::from(bounds.top),
                        x1: f64::from(run.offset() + run.advance()),
                        y1: f64::from(bounds.top + bounds.height),
                    },
                    color,
                ))
            })
        })
    }

    pub fn height(&self) -> f32 {
        let last = self.lines.last().unwrap();
        last.top + last.height
    }

    pub fn cursor(&self, byte: usize, affinity: Affinity) -> Cursor {
        Cursor::from_byte_index(&self.shaped, byte, affinity)
    }

    pub(crate) fn word_cursor(&self, cursor: Cursor, backward: bool) -> Cursor {
        let mut current = cursor;
        let mut visited = BTreeSet::new();
        loop {
            if !visited.insert((current.index(), current.affinity() == Affinity::Upstream)) {
                // Parley visual cursors can cycle at soft-wrapped bidi boundaries.
                let [left, right] = cursor.visual_clusters(&self.shaped);
                let rtl = if backward {
                    left.or(right)
                } else {
                    right.or(left)
                }
                .is_some_and(|cluster| cluster.is_rtl());
                return if backward != rtl {
                    cursor.previous_logical_word(&self.shaped)
                } else {
                    cursor.next_logical_word(&self.shaped)
                };
            }
            let next = if backward {
                current.previous_visual(&self.shaped)
            } else {
                current.next_visual(&self.shaped)
            };
            if next == current {
                return current;
            }
            current = next;
            let [Some(left), Some(right)] = current.visual_clusters(&self.shaped) else {
                return current;
            };
            let boundary = if left.is_rtl() {
                left.is_word_boundary()
                    && if backward {
                        left.is_space_or_nbsp()
                            || (right.is_word_boundary() && !right.is_space_or_nbsp())
                    } else {
                        !left.is_space_or_nbsp()
                    }
            } else {
                right.is_word_boundary()
                    && if backward {
                        !right.is_space_or_nbsp()
                    } else {
                        !left.is_space_or_nbsp()
                    }
            };
            if boundary {
                return current;
            }
        }
    }

    pub fn hit_test(&self, x: f32, y: f32) -> Cursor {
        let index = self
            .lines
            .partition_point(|line| line.top + line.height <= y)
            .min(self.lines.len() - 1);
        let metrics = *self.shaped.get(index).unwrap().metrics();
        Cursor::from_point(
            &self.shaped,
            x,
            (metrics.block_min_coord + metrics.block_max_coord) * 0.5,
        )
    }

    pub fn caret(&self, cursor: Cursor, width: f32) -> BoundingBox {
        let mut rect = cursor.geometry(&self.shaped, width);
        let index = self
            .shaped
            .lines()
            .position(|line| rect.y0 < f64::from(line.metrics().block_max_coord))
            .unwrap_or(self.lines.len() - 1);
        let line = &self.lines[index];
        rect.y0 = f64::from(line.top);
        rect.y1 = f64::from(line.top + line.height);
        rect
    }

    pub fn selection(&self, selection: Selection) -> Vec<BoundingBox> {
        selection
            .geometry(&self.shaped)
            .into_iter()
            .map(|(mut rect, index)| {
                let line = &self.lines[index];
                rect.y0 = f64::from(line.top);
                rect.y1 = f64::from(line.top + line.height);
                rect
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use onestore::document::Format;

    #[test]
    #[ignore = "requires Arial and CANVAS_TEST_SUBSTITUTE pointing to Carlito"]
    fn native_unkerned_advances() {
        let mut engine = TextEngine::default();
        engine
            .register_substitute(parley::fontique::Blob::new(Arc::new(
                std::fs::read(std::env::var_os("CANVAS_TEST_SUBSTITUTE").unwrap()).unwrap(),
            )))
            .unwrap();
        for (font, expected) in [("Calibri", 149.63843), ("Arial", 184.58305)] {
            let paragraph = Paragraph::new(
                "AVATAR AVATAR SECOND OFFICE".into(),
                onestore::document::Format {
                    font: Some(font.into()),
                    font_size: Some(11.0),
                    ..Default::default()
                },
            );
            let layout = engine.layout(&paragraph, 400.0).unwrap();
            assert_eq!(layout.lines().count(), 1);
            assert!((layout.lines().next().unwrap().0.metrics().advance - expected).abs() < 0.001);
        }
    }

    #[test]
    fn rejected_substitute_leaves_font_selection_unchanged() {
        let mut engine = TextEngine::default();
        let paragraph = Paragraph::new("first words wrap here".into(), Format::default());
        let before = engine.layout(&paragraph, 72.0).unwrap();
        for data in [Vec::new(), vec![0; 512], b"not a font".to_vec()] {
            let result =
                engine.register_substitute(parley::fontique::Blob::new(std::sync::Arc::new(data)));
            assert_eq!(result.unwrap_err(), LayoutError::UnsupportedSubstituteFont);
        }
        assert!(engine.arial_substitutes.is_empty());
        let after = engine.layout(&paragraph, 72.0).unwrap();
        assert_eq!(before.height(), after.height());
        let lines = |layout: &TextLayout| {
            layout
                .lines()
                .map(|(_, line)| (line.source.clone(), line.baseline))
                .collect::<Vec<_>>()
        };
        assert_eq!(lines(&before), lines(&after));
        assert_eq!(
            before
                .lines()
                .next()
                .unwrap()
                .0
                .runs()
                .next()
                .unwrap()
                .font()
                .font
                .data
                .id(),
            after
                .lines()
                .next()
                .unwrap()
                .0
                .runs()
                .next()
                .unwrap()
                .font()
                .font
                .data
                .id()
        );
    }

    #[test]
    fn missing_family_keeps_digits_in_the_letters_face() {
        let paragraph = Paragraph::new(
            "Monday, August 10, 2026".into(),
            Format {
                font: Some("Snowbound Missing Family".into()),
                ..Format::default()
            },
        );
        let layout = TextEngine::default().layout(&paragraph, 468.0).unwrap();
        let fonts = layout
            .lines()
            .flat_map(|(line, _)| {
                line.runs()
                    .map(|run| run.font().font.data.id())
                    .collect::<Vec<_>>()
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(fonts.len(), 1);
    }

    #[test]
    fn links_draw_blue_and_underlined_unless_coloured() {
        let mut engine = TextEngine::default();
        let link = Format {
            hyperlink: Some(true),
            ..Format::default()
        };
        let layout = engine
            .layout(
                &Paragraph::from_runs([
                    ("plain ".into(), Format::default()),
                    ("link".into(), link.clone()),
                    (
                        " red".into(),
                        Format {
                            color: Some(0x0000_00ff),
                            ..link
                        },
                    ),
                ]),
                300.0,
            )
            .unwrap();
        let runs: Vec<_> = layout
            .lines()
            .flat_map(|(line, _)| line.items().collect::<Vec<_>>())
            .filter_map(|item| match item {
                PositionedLayoutItem::GlyphRun(run) => {
                    let style = run.style();
                    Some((style.brush.color, style.underline.is_some()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(runs, [(0, false), (0x00ff_0000, true), (0x0000_00ff, true)]);
    }

    #[test]
    fn paragraph_alignment_offsets_lines_within_the_wrap_width() {
        let mut engine = TextEngine::default();
        let mut offset = |alignment| {
            let layout = engine
                .layout(
                    &Paragraph::new(
                        "Short".into(),
                        Format {
                            alignment,
                            ..Format::default()
                        },
                    ),
                    200.0,
                )
                .unwrap();
            let (line, _) = layout.lines().next().unwrap();
            let advance = line.metrics().advance;
            let x = line
                .items()
                .find_map(|item| match item {
                    PositionedLayoutItem::GlyphRun(run) => Some(run.offset()),
                    _ => None,
                })
                .unwrap();
            (x, advance)
        };
        let (left, advance) = offset(None);
        assert_eq!(left, 0.0);
        assert!(((offset(Some(1)).0) - (200.0 - advance) / 2.0).abs() < 0.01);
        assert!(((offset(Some(2)).0) - (200.0 - advance)).abs() < 0.01);
    }

    #[test]
    fn highlights_share_line_geometry_without_changing_wraps() {
        let paragraph = Paragraph::from_runs([
            (
                "A highlighted ".into(),
                Format {
                    highlight: Some(0x0000ffff),
                    ..Format::default()
                },
            ),
            (
                "phrase and ".into(),
                Format {
                    highlight: Some(0),
                    ..Format::default()
                },
            ),
            (
                "automatic background".into(),
                Format {
                    highlight: Some(0xff000000),
                    ..Format::default()
                },
            ),
        ]);
        let mut engine = TextEngine::default();
        let layout = engine.layout(&paragraph, 80.0).unwrap();
        let plain = engine
            .layout(
                &Paragraph::new(paragraph.text().into(), Format::default()),
                80.0,
            )
            .unwrap();
        assert_eq!(
            layout
                .lines()
                .map(|(_, l)| l.source.clone())
                .collect::<Vec<_>>(),
            plain
                .lines()
                .map(|(_, l)| l.source.clone())
                .collect::<Vec<_>>()
        );
        let backgrounds = layout.backgrounds().collect::<Vec<_>>();
        assert!(backgrounds.iter().any(|(_, color)| *color == 0));
        assert!(backgrounds.iter().any(|(_, color)| *color == 0x0000ffff));
        assert!(!backgrounds.iter().any(|(_, color)| *color == 0xff000000));
        for (rect, _) in backgrounds {
            assert!(rect.width() > 0.0);
            assert!(
                layout
                    .lines()
                    .any(|(_, line)| rect.y0 == f64::from(line.top)
                        && rect.y1 == f64::from(line.top + line.height))
            );
        }
    }

    #[test]
    fn color_boundaries_preserve_contextual_shaping_and_wrapping() {
        let mut engine = TextEngine::default();
        for text in ["office affinity", "العربية سلام", "ae\u{301}👩🏽‍💻z"] {
            let plain = Paragraph::new(text.into(), Format::default());
            let colored = Paragraph::from_runs(text.chars().enumerate().map(|(index, ch)| {
                (
                    ch.to_string(),
                    Format {
                        color: Some(if index % 2 == 0 { 0xff } else { 0xff0000 }),
                        ..Format::default()
                    },
                )
            }));
            for width in [1.0, 40.0, 110.0, 1000.0] {
                let plain = engine.layout(&plain, width).unwrap();
                let colored = engine.layout(&colored, width).unwrap();
                assert_eq!(plain.lines.len(), colored.lines.len());
                for ((plain_line, plain_box), (colored_line, colored_box)) in
                    plain.lines().zip(colored.lines())
                {
                    assert_eq!(plain_box.source, colored_box.source);
                    assert_eq!(plain_box.baseline, colored_box.baseline);
                    assert_eq!(plain_box.height, colored_box.height);
                    let glyphs = |line: parley::Line<'_, TextBrush>| {
                        line.items()
                            .flat_map(|item| match item {
                                PositionedLayoutItem::GlyphRun(run) => run
                                    .positioned_glyphs()
                                    .map(|glyph| {
                                        (
                                            run.run().font().font.data.id(),
                                            glyph.id,
                                            glyph.x,
                                            glyph.y,
                                        )
                                    })
                                    .collect::<Vec<_>>(),
                                PositionedLayoutItem::InlineBox(_) => unreachable!(),
                            })
                            .collect::<Vec<_>>()
                    };
                    let plain_glyphs = glyphs(plain_line);
                    let colored_glyphs = glyphs(colored_line);
                    assert_eq!(plain_glyphs.len(), colored_glyphs.len());
                    for (a, b) in plain_glyphs.into_iter().zip(colored_glyphs) {
                        assert_eq!((a.0, a.1), (b.0, b.1), "{text}, width {width}");
                        assert!((a.2 - b.2).abs() < 0.001 && (a.3 - b.3).abs() < 0.001);
                    }
                }
            }
        }
    }

    #[test]
    fn empty_paragraph_has_source_position_and_caret() {
        let paragraph = Paragraph::new(String::new(), Format::default());
        let layout = TextEngine::default().layout(&paragraph, 100.0).unwrap();
        assert_eq!(layout.lines().next().unwrap().1.source, 0..0);
        assert!(layout.height() > 0.0);
        let cursor = layout.hit_test(50.0, 50.0);
        assert_eq!(cursor.index(), 0);
        let caret = layout.caret(cursor, 1.0);
        assert_eq!(caret.y0, 0.0);
        assert_eq!(caret.y1, f64::from(layout.height()));
    }

    #[test]
    fn long_words_wrap_with_complete_source_and_usable_carets() {
        let paragraph = Paragraph::new(
            "Wrapped HAMBURGEFONTS abcdefghijklmnopqrstuvwxyz 0123456789".into(),
            Format::default(),
        );
        let width = 110.0;
        let layout = TextEngine::default().layout(&paragraph, width).unwrap();
        let mut end = 0;
        for (line, bounds) in layout.lines() {
            assert!(line.metrics().advance - line.metrics().trailing_whitespace <= width);
            assert_eq!(bounds.source.start, end);
            end = bounds.source.end;
        }
        assert_eq!(end, paragraph.text().len());
        for byte in 0..=end {
            for affinity in [Affinity::Upstream, Affinity::Downstream] {
                let rect = layout.caret(layout.cursor(byte, affinity), 0.0);
                let hit = layout.hit_test(rect.x0 as f32, ((rect.y0 + rect.y1) * 0.5) as f32);
                assert_eq!(hit.index(), byte, "byte {byte}, {affinity:?}, {rect:?}");
            }
        }
    }

    #[test]
    fn wrapped_carets_hit_the_same_source_position() {
        let paragraph = Paragraph::new(
            "A quiet evening under the trees, with notes beside every line.".into(),
            Format::default(),
        );
        let layout = TextEngine::default().layout(&paragraph, 110.0).unwrap();
        assert!(layout.lines.len() > 1);
        for byte in 0..=paragraph.text().len() {
            for affinity in [Affinity::Upstream, Affinity::Downstream] {
                let cursor = layout.cursor(byte, affinity);
                let rect = layout.caret(cursor, 0.0);
                let hit = layout.hit_test(rect.x0 as f32, ((rect.y0 + rect.y1) * 0.5) as f32);
                assert_eq!(hit.index(), byte, "byte {byte}, {affinity:?}, {rect:?}");
            }
        }
    }

    #[test]
    fn visual_navigation_keeps_combining_and_emoji_sequences_intact() {
        let graphemes = ["a", "e\u{301}", "👩🏽‍💻", "🇨🇦", "👩‍👩‍👧‍👦", "z"];
        let text = graphemes.concat();
        let paragraph = Paragraph::new(text, Format::default());
        let layout = TextEngine::default().layout(&paragraph, 1000.0).unwrap();
        let mut byte = 0;
        let mut cursor = layout.cursor(0, Affinity::Downstream);
        for grapheme in graphemes {
            byte += grapheme.len();
            cursor = cursor.next_visual(&layout.shaped);
            assert_eq!(cursor.index(), byte);
        }
        for grapheme in graphemes.into_iter().rev() {
            byte -= grapheme.len();
            cursor = cursor.previous_visual(&layout.shaped);
            assert_eq!(cursor.index(), byte);
        }
    }

    #[test]
    fn emoji_only_lines_have_nonzero_metrics() {
        let paragraph = Paragraph::new("👩‍👩‍👧‍👦".into(), Format::default());
        let layout = TextEngine::default().layout(&paragraph, 100.0).unwrap();
        assert!(layout.height() > 0.0);
        let caret = layout.caret(layout.cursor(0, Affinity::Downstream), 1.0);
        assert!(caret.height() > 0.0);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn color_emoji_preserve_text_baselines_and_respect_explicit_font_size() {
        let mut engine = TextEngine::default();
        for size in [11.0, 22.0] {
            let format = Format {
                font_size: Some(size),
                ..Default::default()
            };
            let base = engine
                .layout(&Paragraph::new("Mg".into(), format.clone()), 1000.0)
                .unwrap();
            for text in ["🌳", "👩‍👩‍👧‍👦", "before 🌳 after", "Mg\n🌳\nMg"] {
                let layout = engine
                    .layout(&Paragraph::new(text.into(), format.clone()), 1000.0)
                    .unwrap();
                for (_, line) in layout.lines() {
                    assert_eq!(line.height, base.lines[0].height, "{text}");
                    assert!(
                        (line.baseline - line.top - base.lines[0].baseline).abs() < 0.0001,
                        "{text}"
                    );
                }
            }
        }
    }

    #[test]
    fn bidi_hit_testing_stays_on_grapheme_boundaries() {
        let graphemes = [
            "a",
            " ",
            "ש",
            "ל",
            "ו",
            "ם",
            " ",
            "1",
            "2",
            "3",
            " ",
            "👩🏽‍💻",
            " ",
            "e\u{301}",
        ];
        let mut boundaries = vec![0];
        for grapheme in graphemes {
            boundaries.push(boundaries.last().unwrap() + grapheme.len());
        }
        let paragraph = Paragraph::new(graphemes.concat(), Format::default());
        let mut engine = TextEngine::default();
        for width in [1.0, 10.0, 40.0, 80.0, 1000.0] {
            let layout = engine.layout(&paragraph, width).unwrap();
            for (_, line) in layout.lines() {
                assert!(boundaries.contains(&line.source.start));
                assert!(boundaries.contains(&line.source.end));
            }
            for y in (-10..layout.height() as i32 + 10).step_by(3) {
                for x in (-10..width as i32 + 10).step_by(3) {
                    let cursor = layout.hit_test(x as f32, y as f32);
                    assert!(
                        boundaries.contains(&cursor.index()),
                        "{width}: {x}, {y} -> {cursor:?}"
                    );
                    let caret = layout.caret(cursor, 1.0);
                    assert!(caret.x0.is_finite() && caret.y0.is_finite() && caret.height() > 0.0);
                }
            }
        }
    }

    #[test]
    fn selection_uses_the_same_line_boxes_as_hit_testing() {
        let paragraph = Paragraph::new(
            "one two three four five six seven".into(),
            Format::default(),
        );
        let layout = TextEngine::default().layout(&paragraph, 60.0).unwrap();
        let selection = Selection::from(layout.cursor(0, Affinity::Downstream))
            .extend(layout.cursor(paragraph.text().len(), Affinity::Upstream));
        let rects = layout.selection(selection);
        assert_eq!(rects.len(), layout.lines.len());
        for (rect, line) in rects.iter().zip(&layout.lines) {
            assert_eq!(rect.y0, f64::from(line.top));
            assert_eq!(rect.y1, f64::from(line.top + line.height));
            let cursor = layout.hit_test(rect.x0 as f32 + 0.01, ((rect.y0 + rect.y1) * 0.5) as f32);
            assert_eq!(cursor.index(), line.source.start);
        }
    }
}
