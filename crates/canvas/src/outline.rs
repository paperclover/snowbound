use crate::{
    layout::{LayoutError, TextEngine, TextLayout},
    page::{Definition, Outline, PageParagraph, Title},
    text::{Paragraph, TextProjection},
};
use onestore::{
    ExGuid,
    document::{Format, Kind},
};
use std::collections::{BTreeMap, BTreeSet};

pub struct OutlineLayout {
    pub paragraphs: Vec<ParagraphLayout>,
    pub size: [f32; 2],
}

pub struct ParagraphLayout {
    pub id: ExGuid,
    pub origin: [f32; 2],
    pub projection: TextProjection,
    pub text: TextLayout,
    /// Marker x is outline-local; y is paragraph-local so reflow cannot accumulate rounding drift.
    pub markers: Vec<(TextLayout, [f32; 2])>,
    pub tags: Vec<ParagraphTag>,
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum TagIcon {
    CheckBox { checked: bool },
    Question,
    Music,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParagraphTag {
    pub icon: TagIcon,
    /// Coordinates are outline-local in x and paragraph-local in y.
    pub origin: [f32; 2],
    pub label: String,
    pub disabled: bool,
}

impl ParagraphTag {
    pub const SIZE: f32 = 12.0;
}

impl ParagraphLayout {
    pub(crate) fn shape(
        engine: &mut TextEngine,
        paragraph: &PageParagraph,
        width: f32,
        indents: &[f32],
        definitions: &BTreeMap<ExGuid, Definition>,
    ) -> Result<Self, LayoutError> {
        if !width.is_finite() || width <= 0.0 {
            return Err(LayoutError::InvalidWidth);
        }
        if indents.len() < 2
            || indents.iter().any(|v| !v.is_finite() || *v < 0.0)
            || paragraph.level == 0
        {
            return Err(LayoutError::InvalidIndentation);
        }
        if !paragraph.unsupported.is_empty() {
            return Err(LayoutError::UnsupportedContent);
        }
        let known = (paragraph.level as usize).min(indents.len() - 1);
        let indent = (indents[1..=known]
            .iter()
            .map(|v| f64::from(*v))
            .sum::<f64>()
            + f64::from(paragraph.level - known as u32) * f64::from(*indents.last().unwrap()))
            as f32;
        if !indent.is_finite() || indent >= width {
            return Err(LayoutError::InvalidIndentation);
        }
        let projection = paragraph
            .combined_text()
            .project()
            .map_err(|_| LayoutError::InvalidSourceRange)?;
        let format = &projection.text().spans()[0].format;
        let spacing = format.list_spacing.unwrap_or(7.2);
        if !spacing.is_finite() || spacing < 0.0 {
            return Err(LayoutError::InvalidSpacing);
        }
        let mut text = engine.layout(projection.text(), width - indent)?;
        let mut markers = Vec::new();
        let mut marker_x = indent;
        for id in paragraph.lists.iter().rev() {
            let definition = definitions.get(id).ok_or(LayoutError::InvalidList)?;
            let Kind::List {
                font,
                format: Some(value),
                ..
            } = &definition.kind
            else {
                return Err(LayoutError::InvalidList);
            };
            if value.contains('\u{fffd}') {
                return Err(LayoutError::UnsupportedContent);
            }
            let marker = Paragraph::new(
                value.clone(),
                Format {
                    font: font.clone().or_else(|| definition.format.font.clone()),
                    font_size: definition.format.font_size.or(format.font_size),
                    color: definition.format.color,
                    ..Format::default()
                },
            );
            let layout = engine.layout(&marker, width)?;
            if layout.lines().count() != 1 {
                return Err(LayoutError::InvalidList);
            }
            let (line, metrics) = layout.lines().next().unwrap();
            marker_x -= spacing + line.metrics().advance;
            if !marker_x.is_finite() {
                return Err(LayoutError::InvalidSpacing);
            }
            let y = text.lines().next().unwrap().1.baseline - metrics.baseline;
            markers.push((layout, [marker_x, y]));
        }
        text.minimum_line_height(format.line_spacing.unwrap_or(0.0))?;
        let mut tags = Vec::new();
        for tag in paragraph
            .tags
            .iter()
            .chain(paragraph.text.iter().flat_map(|text| &text.tags))
        {
            let Some(Definition {
                kind:
                    Kind::TagDefinition {
                        shape,
                        label,
                        color,
                        highlight,
                        ..
                    },
                ..
            }) = tag.definition.as_ref().and_then(|id| definitions.get(id))
            else {
                return Err(LayoutError::UnsupportedContent);
            };
            if color.is_some() || highlight.is_some() || tag.status & 4 != 0 {
                return Err(LayoutError::UnsupportedContent);
            }
            let icon = match shape {
                Some(0) => continue,
                Some(3) => TagIcon::CheckBox {
                    checked: tag.status & 1 != 0,
                },
                Some(15) => TagIcon::Question,
                Some(121) => TagIcon::Music,
                _ => return Err(LayoutError::UnsupportedContent),
            };
            marker_x -= 20.25;
            tags.push(ParagraphTag {
                icon,
                origin: [marker_x, 0.0],
                label: label.clone().unwrap_or_default(),
                disabled: tag.status & 2 != 0,
            });
        }
        Ok(Self {
            id: paragraph.id,
            origin: [indent, 0.0],
            projection,
            text,
            markers,
            tags,
        })
    }
}

pub(crate) fn visible_paragraphs(nodes: &[PageParagraph]) -> impl Iterator<Item = &PageParagraph> {
    let mut hidden = BTreeSet::new();
    nodes.iter().filter(move |paragraph| {
        let invisible = paragraph
            .parent
            .is_some_and(|parent| hidden.contains(&parent));
        if invisible || paragraph.collapsed {
            hidden.insert(paragraph.id);
        }
        !invisible
    })
}

pub(crate) fn arrange<'a>(
    paragraphs: impl Iterator<Item = &'a ParagraphLayout>,
    width: f32,
    width_set_by_user: bool,
) -> Result<(Vec<f32>, [f32; 2]), LayoutError> {
    let mut origins = Vec::new();
    let mut bottom = 0.0_f64;
    let mut previous_after = 0.0_f32;
    let mut content_width = 0.0_f32;
    for paragraph in paragraphs {
        let format = &paragraph.projection.text().spans()[0].format;
        let before = format.space_before.unwrap_or(0.0);
        let after = format.space_after.unwrap_or(0.0);
        if [before, after].iter().any(|v| !v.is_finite() || *v < 0.0) {
            return Err(LayoutError::InvalidSpacing);
        }
        if !origins.is_empty() {
            bottom += f64::from(previous_after.max(before));
        }
        origins.push(bottom as f32);
        bottom += f64::from(
            paragraph
                .markers
                .iter()
                .map(|(m, _)| m.height())
                .fold(paragraph.text.height(), f32::max),
        );
        previous_after = after;
        if !(bottom as f32).is_finite() {
            return Err(LayoutError::InvalidSpacing);
        }
        content_width = content_width.max(
            paragraph.origin[0]
                + paragraph
                    .text
                    .lines()
                    .map(|(line, _)| line.metrics().advance)
                    .fold(0.0, f32::max),
        );
    }
    let height = bottom as f32;
    if !height.is_finite() || !content_width.is_finite() {
        return Err(LayoutError::InvalidSpacing);
    }
    Ok((
        origins,
        [
            if width_set_by_user {
                width
            } else {
                content_width.max(36.0)
            },
            height,
        ],
    ))
}

impl OutlineLayout {
    pub(crate) fn new(
        mut paragraphs: Vec<ParagraphLayout>,
        width: f32,
        width_set_by_user: bool,
    ) -> Result<Self, LayoutError> {
        if !width.is_finite() || width <= 0.0 {
            return Err(LayoutError::InvalidWidth);
        }
        let (origins, size) = arrange(paragraphs.iter(), width, width_set_by_user)?;
        for (paragraph, y) in paragraphs.iter_mut().zip(origins) {
            paragraph.origin[1] = y;
        }
        Ok(Self { paragraphs, size })
    }
}

impl Outline {
    pub fn layout(
        &self,
        engine: &mut TextEngine,
        definitions: &BTreeMap<ExGuid, Definition>,
    ) -> Result<OutlineLayout, LayoutError> {
        let width = self
            .layout
            .reserved_width
            .or(self.layout.max_width)
            .ok_or(LayoutError::InvalidWidth)?;
        self.layout_with_width(engine, definitions, width)
    }

    fn layout_with_width(
        &self,
        engine: &mut TextEngine,
        definitions: &BTreeMap<ExGuid, Definition>,
        width: f32,
    ) -> Result<OutlineLayout, LayoutError> {
        if self.indents.len() < 2 || self.indents.iter().any(|v| !v.is_finite() || *v < 0.0) {
            return Err(LayoutError::InvalidIndentation);
        }
        if !self.unsupported.is_empty() {
            return Err(LayoutError::UnsupportedContent);
        }
        let paragraphs = visible_paragraphs(&self.paragraphs)
            .map(|p| ParagraphLayout::shape(engine, p, width, &self.indents, definitions))
            .collect::<Result<_, _>>()?;
        OutlineLayout::new(
            paragraphs,
            width,
            self.layout.width_set_by_user == Some(true),
        )
    }
}

impl Title {
    pub fn layout(
        &self,
        engine: &mut TextEngine,
        definitions: &BTreeMap<ExGuid, Definition>,
    ) -> Result<Vec<([f32; 2], OutlineLayout)>, LayoutError> {
        if [self.layout.x, self.layout.y]
            .into_iter()
            .flatten()
            .any(|v| !v.is_finite())
        {
            return Err(LayoutError::InvalidSpacing);
        }
        let mut layouts = Vec::new();
        let mut bottom = 0.0_f32;
        for outline in &self.outlines {
            let width = outline
                .layout
                .reserved_width
                .or(outline.layout.max_width)
                .unwrap_or(f32::MAX);
            let layout = outline.layout_with_width(engine, definitions, width)?;
            let origin = [
                outline.layout.x.unwrap_or(0.0),
                bottom + outline.layout.y.unwrap_or(0.0),
            ];
            let height = outline.layout.max_height.unwrap_or(0.0);
            if origin.iter().any(|v| !v.is_finite()) || !height.is_finite() || height < 0.0 {
                return Err(LayoutError::InvalidSpacing);
            }
            bottom = origin[1] + layout.size[1].max(height);
            if !bottom.is_finite() {
                return Err(LayoutError::InvalidSpacing);
            }
            layouts.push((origin, layout));
        }
        Ok(layouts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page::{PageParagraph, TextObject};
    use onestore::document::Layout;

    fn paragraph(n: u32, text: &str, level: u32, parent: Option<u32>) -> PageParagraph {
        PageParagraph {
            id: ExGuid {
                n,
                ..ExGuid::default()
            },
            parent: parent.map(|n| ExGuid {
                n,
                ..ExGuid::default()
            }),
            level,
            format: Format::default(),
            text: vec![TextObject {
                id: ExGuid {
                    n: n + 100,
                    ..ExGuid::default()
                },
                text: Paragraph::new(text.into(), Format::default()),
                tags: Vec::new(),
            }],
            unsupported: Vec::new(),
            lists: Vec::new(),
            tags: Vec::new(),
            collapsed: false,
        }
    }

    #[test]
    fn tags_preserve_line_geometry_and_follow_list_indentation() {
        use onestore::document::Tag;
        let id = ExGuid {
            n: 500,
            ..ExGuid::default()
        };
        let mut node = paragraph(1, "Text that wraps across several lines", 2, None);
        let mut engine = TextEngine::default();
        let mut definitions = BTreeMap::new();
        let plain =
            ParagraphLayout::shape(&mut engine, &node, 120.0, &[18.0, 0.0, 27.0], &definitions)
                .unwrap();
        node.text[0].tags.push(Tag {
            definition: Some(id),
            status: 3,
            action_type: None,
            created: None,
            completed: None,
            start: None,
            due: None,
            task_id: None,
            extra_set: 0,
        });
        for (shape, expected) in [
            (3, TagIcon::CheckBox { checked: true }),
            (15, TagIcon::Question),
            (121, TagIcon::Music),
        ] {
            definitions.insert(
                id,
                Definition {
                    kind: Kind::TagDefinition {
                        shape: Some(shape),
                        label: Some("Label".into()),
                        action_type: None,
                        color: None,
                        highlight: None,
                    },
                    format: Format::default(),
                },
            );
            let tagged =
                ParagraphLayout::shape(&mut engine, &node, 120.0, &[18.0, 0.0, 27.0], &definitions)
                    .unwrap();
            assert_eq!(tagged.tags[0].icon, expected);
            assert_eq!(tagged.tags[0].origin, [6.75, 0.0]);
            assert_eq!(tagged.tags[0].label, "Label");
            assert!(tagged.tags[0].disabled);
            assert_eq!(tagged.text.height(), plain.text.height());
            assert_eq!(
                tagged
                    .text
                    .lines()
                    .map(|(_, b)| (&b.source, b.baseline))
                    .collect::<Vec<_>>(),
                plain
                    .text
                    .lines()
                    .map(|(_, b)| (&b.source, b.baseline))
                    .collect::<Vec<_>>()
            );
        }
        let list = ExGuid {
            n: 501,
            ..ExGuid::default()
        };
        definitions.insert(
            list,
            Definition {
                kind: Kind::List {
                    font: Some("Arial".into()),
                    format: Some("•".into()),
                    bullet: None,
                    restart: None,
                },
                format: Format::default(),
            },
        );
        node.lists.push(list);
        let tagged =
            ParagraphLayout::shape(&mut engine, &node, 120.0, &[18.0, 0.0, 27.0], &definitions)
                .unwrap();
        assert_eq!(tagged.tags[0].origin[0], tagged.markers[0].1[0] - 20.25);
        assert_eq!(tagged.tags[0].origin[1], 0.0);
        let Kind::TagDefinition { shape, .. } = &mut definitions.get_mut(&id).unwrap().kind else {
            unreachable!()
        };
        *shape = Some(999);
        assert!(matches!(
            ParagraphLayout::shape(&mut engine, &node, 120.0, &[18.0, 0.0, 27.0], &definitions),
            Err(LayoutError::UnsupportedContent)
        ));
    }

    #[test]
    fn title_accepts_intrinsic_width_and_keeps_date_after_wrapped_title() {
        let mut title = Title {
            id: ExGuid::default(),
            layout: Layout::default(),
            outlines: ["A title with enough words to wrap", "A date"]
                .into_iter()
                .enumerate()
                .map(|(index, text)| Outline {
                    id: ExGuid {
                        n: index as u32,
                        ..ExGuid::default()
                    },
                    layout: Layout {
                        max_height: Some(21.6),
                        ..Layout::default()
                    },
                    indents: vec![18.0, 0.0],
                    paragraphs: vec![paragraph(index as u32, text, 1, None)],
                    unsupported: Vec::new(),
                })
                .collect(),
        };
        let mut engine = TextEngine::default();
        let definitions = BTreeMap::new();
        assert!(matches!(
            title.outlines[0].layout(&mut engine, &definitions),
            Err(LayoutError::InvalidWidth)
        ));
        let layouts = title.layout(&mut engine, &definitions).unwrap();
        assert_eq!(layouts[0].1.paragraphs[0].text.lines().count(), 1);
        assert_eq!(layouts[1].0, [0.0, 21.6]);
        title.outlines[0].layout.max_width = Some(70.0);
        let layouts = title.layout(&mut engine, &definitions).unwrap();
        assert!(layouts[0].1.paragraphs[0].text.lines().count() > 1);
        assert_eq!(layouts[1].0[1], layouts[0].1.size[1]);
        for value in [f32::NAN, f32::INFINITY, -1.0] {
            title.outlines[0].layout.max_height = Some(value);
            assert!(matches!(
                title.layout(&mut engine, &definitions),
                Err(LayoutError::InvalidSpacing)
            ));
        }
        title.outlines[0].layout.max_height = Some(21.6);
        title.layout.x = Some(f32::NAN);
        assert!(matches!(
            title.layout(&mut engine, &definitions),
            Err(LayoutError::InvalidSpacing)
        ));
    }

    #[test]
    fn collapses_descendants_and_uses_the_larger_adjacent_spacing() {
        let mut first = paragraph(1, "First", 1, None);
        first.text[0].text = Paragraph::new(
            "First".into(),
            Format {
                space_after: Some(4.0),
                ..Format::default()
            },
        );
        let mut second = paragraph(2, "Second", 2, Some(1));
        second.text[0].text = Paragraph::new(
            "Second".into(),
            Format {
                space_before: Some(7.0),
                space_after: Some(3.0),
                ..Format::default()
            },
        );
        second.collapsed = true;
        let mut outline = Outline {
            id: ExGuid::default(),
            layout: Layout {
                max_width: Some(300.0),
                ..Layout::default()
            },
            indents: vec![18.0, 0.0, 27.0],
            paragraphs: vec![
                first,
                second,
                paragraph(3, "Hidden", 6, Some(2)),
                paragraph(4, "Last", 1, None),
            ],
            unsupported: Vec::new(),
        };
        let mut engine = TextEngine::default();
        let layout = outline.layout(&mut engine, &BTreeMap::new()).unwrap();
        assert_eq!(
            layout.paragraphs.iter().map(|p| p.id.n).collect::<Vec<_>>(),
            [1, 2, 4]
        );
        assert_eq!(
            layout.paragraphs[1].origin,
            [27.0, layout.paragraphs[0].text.height() + 7.0]
        );
        assert_eq!(
            layout.paragraphs[2].origin[1],
            layout.paragraphs[1].origin[1] + layout.paragraphs[1].text.height() + 3.0
        );
        outline.paragraphs[1].collapsed = false;
        let expanded = outline.layout(&mut engine, &BTreeMap::new()).unwrap();
        assert_eq!(expanded.paragraphs[2].origin[0], 135.0);
        assert!(expanded.size[1] > layout.size[1]);
        outline.indents[1] = f32::NAN;
        assert!(matches!(
            outline.layout(&mut engine, &BTreeMap::new()),
            Err(LayoutError::InvalidIndentation)
        ));
    }

    #[test]
    fn marker_height_expands_the_paragraph_box_without_changing_text_line_advances() {
        let marker_id = ExGuid {
            n: 99,
            ..ExGuid::default()
        };
        let mut short = paragraph(1, "Short", 2, None);
        short.lists.push(marker_id);
        let mut long = paragraph(
            2,
            "A long paragraph wraps onto several separate lines of text.",
            2,
            None,
        );
        long.lists.push(marker_id);
        let outline = Outline {
            id: ExGuid::default(),
            layout: Layout {
                max_width: Some(110.0),
                width_set_by_user: Some(true),
                ..Layout::default()
            },
            indents: vec![18.0, 0.0, 27.0],
            paragraphs: vec![short, long, paragraph(3, "Last", 1, None)],
            unsupported: Vec::new(),
        };
        let definitions = BTreeMap::from([(
            marker_id,
            Definition {
                kind: Kind::List {
                    font: Some("Courier New".into()),
                    format: Some("○".into()),
                    restart: None,
                    bullet: Some(4),
                },
                format: Format {
                    font_size: Some(22.0),
                    ..Format::default()
                },
            },
        )]);
        let mut engine = TextEngine::default();
        let layout = outline.layout(&mut engine, &definitions).unwrap();
        let short = &layout.paragraphs[0];
        let long = &layout.paragraphs[1];
        assert!(short.markers[0].0.height() > short.text.height());
        assert!(long.text.height() > long.markers[0].0.height());
        assert_eq!(long.origin[1], short.markers[0].0.height());
        assert_eq!(
            layout.paragraphs[2].origin[1],
            long.origin[1] + long.text.height()
        );
        let plain = engine.layout(long.projection.text(), 83.0).unwrap();
        assert_eq!(
            long.text
                .lines()
                .map(|(_, l)| l.baseline)
                .collect::<Vec<_>>(),
            plain.lines().map(|(_, l)| l.baseline).collect::<Vec<_>>()
        );
        assert_eq!(
            short.origin[1]
                + short.markers[0].1[1]
                + short.markers[0].0.lines().next().unwrap().1.baseline,
            short.origin[1] + short.text.lines().next().unwrap().1.baseline
        );
        assert_eq!(layout.size[0], 110.0);
    }
}
