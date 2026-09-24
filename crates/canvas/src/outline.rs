use crate::{
    document::{DocumentEdit, edited_nodes},
    layout::{LayoutError, TextEngine, TextLayout},
};
use onestore::page::text::{Paragraph, TextProjection};
use onestore::page::{Definition, Outline, PageParagraph, ParagraphContent, Table, Title};
use onestore::{
    ExGuid,
    document::{Format, Kind},
};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) const TITLE_WIDTH: f32 = 468.0;

#[derive(Clone)]
pub struct OutlineLayout {
    pub paragraphs: Vec<ParagraphLayout>,
    pub tables: Vec<TableLayout>,
    /// Pictures, files and handwriting that occupy a paragraph of their own.
    pub objects: Vec<ObjectLayout>,
    pub size: [f32; 2],
}

#[derive(Clone)]
pub struct ObjectLayout {
    /// The object's identity, which keys a picture's or file icon's decoded image.
    pub id: ExGuid,
    /// Where the picture, file icon, drawing or placeholder draws, outline-local.
    pub rect: [f32; 4],
    pub kind: ObjectKind,
    /// Outline-local bottom of the whole object, label included.
    pub bottom: f32,
}

#[derive(Clone)]
pub enum ObjectKind {
    Picture,
    /// A file's icon with its name centered below.
    File(ParagraphLayout),
    /// Handwriting, whose strokes are relative to the rect's top-left.
    Ink(onestore::page::Ink),
    /// Content the canvas cannot draw, marked by a labelled box.
    Unsupported(ParagraphLayout),
}

impl ObjectLayout {
    pub fn label(&self) -> Option<&ParagraphLayout> {
        match &self.kind {
            ObjectKind::File(label) | ObjectKind::Unsupported(label) => Some(label),
            ObjectKind::Picture | ObjectKind::Ink(_) => None,
        }
    }
}

/// OneNote centers a file's icon and name in a column this wide.
const ATTACHMENT_WIDTH: f32 = 54.0;

#[derive(Clone)]
pub struct TableLayout {
    pub id: ExGuid,
    /// Cells are in paragraph order with disjoint visible ranges.
    pub cells: Vec<CellLayout>,
    pub borders: bool,
}

#[derive(Clone)]
pub struct CellLayout {
    pub id: ExGuid,
    pub rect: [f32; 4],
    /// Visible paragraph indices, including paragraphs in nested tables.
    pub paragraphs: std::ops::Range<usize>,
}

impl CellLayout {
    /// Native ink gutters extend beyond the cell borders.
    pub fn text_bounds(&self) -> [f32; 4] {
        [
            self.rect[0] - 2.7,
            self.rect[1],
            self.rect[2] + 4.62,
            self.rect[3],
        ]
    }

    pub fn clip(&self, rect: parley::BoundingBox) -> Option<parley::BoundingBox> {
        let [left, top, right, bottom] = self.text_bounds().map(f64::from);
        let rect = parley::BoundingBox::new(
            rect.x0.max(left),
            rect.y0.max(top),
            rect.x1.min(right),
            rect.y1.min(bottom),
        );
        (rect.width() > 0.0 && rect.height() > 0.0).then_some(rect)
    }
}

#[derive(Clone)]
pub struct ParagraphLayout {
    pub id: ExGuid,
    pub origin: [f32; 2],
    pub projection: TextProjection,
    pub text: TextLayout,
    /// Marker x is outline-local; y is paragraph-local so reflow cannot accumulate rounding drift.
    pub markers: Vec<(TextLayout, [f32; 2])>,
    pub tags: Vec<ParagraphTag>,
    /// An equation draws in two dimensions in place of its linear text.
    pub math: Option<crate::math::MathLayout>,
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum TagIcon {
    CheckBox { checked: bool },
    Question,
    Music,
    Exclamation,
    RedSquare,
    YellowSquare,
    BlueSquare,
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

/// A one-run paragraph standing in for an object's caption, so it lays out through the same
/// shaping (and caching) as the outline's text.
fn caption(id: ExGuid, text_id: ExGuid, text: &str, format: Format) -> PageParagraph {
    PageParagraph {
        id,
        parent: None,
        level: 1,
        style: None,
        format: Format::default(),
        content: ParagraphContent::Text(onestore::page::TextObject {
            id: text_id,
            date_field: None,
            text: Paragraph::new(text.into(), format),
            tags: Vec::new(),
        }),
        lists: Vec::new(),
        tags: Vec::new(),
        media: Default::default(),
        collapsed: false,
    }
}

/// A picture's displayed size: the user-set layout size, else its intrinsic size.
pub(crate) fn image_size(image: &onestore::page::Image) -> Option<[f32; 2]> {
    let size = [
        image.layout.max_width.or(image.size.map(|s| s[0]))?,
        image.layout.max_height.or(image.size.map(|s| s[1]))?,
    ];
    size.iter()
        .all(|v| v.is_finite() && *v > 0.0)
        .then_some(size)
}

pub(crate) fn indentation(level: u32, indents: &[f32], width: f32) -> Result<f32, LayoutError> {
    if indents.is_empty() || indents.iter().any(|v| !v.is_finite() || *v < 0.0) || level == 0 {
        return Err(LayoutError::InvalidIndentation);
    }
    let known = (level as usize).min(indents.len() - 1);
    let indent = (indents[1..=known]
        .iter()
        .map(|v| f64::from(*v))
        .sum::<f64>()
        + f64::from(level - known as u32) * f64::from(*indents.last().unwrap()))
        as f32;
    if !indent.is_finite() || indent >= width {
        return Err(LayoutError::InvalidIndentation);
    }
    Ok(indent)
}

fn spacing(
    bottom: &mut f64,
    previous: &mut Option<f32>,
    format: &Format,
) -> Result<f32, LayoutError> {
    let before = format.space_before.unwrap_or(0.0);
    let after = format.space_after.unwrap_or(0.0);
    if [before, after].iter().any(|v| !v.is_finite() || *v < 0.0) {
        return Err(LayoutError::InvalidSpacing);
    }
    if let Some(previous) = previous {
        *bottom += f64::from(previous.max(before));
    }
    *previous = Some(after);
    Ok(*bottom as f32)
}

impl ParagraphLayout {
    pub(crate) fn reset_origin(&mut self, x: f32) {
        let offset = x - self.origin[0];
        self.origin = [x, 0.0];
        for (_, origin) in &mut self.markers {
            origin[0] += offset;
        }
        for tag in &mut self.tags {
            tag.origin[0] += offset;
        }
    }

    fn size(&self) -> [f32; 2] {
        if let Some(math) = &self.math {
            return [self.origin[0] + math.size[0], math.size[1]];
        }
        [
            self.origin[0] + self.text.shaped.width(),
            self.markers
                .iter()
                .map(|(layout, _)| layout.height())
                .fold(self.text.height(), f32::max),
        ]
    }

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
        let indent = indentation(paragraph.level, indents, width)?;
        let source = paragraph.text().ok_or(LayoutError::UnsupportedContent)?;
        let projection = source
            .text
            .project()
            .map_err(|_| LayoutError::InvalidSourceRange)?;
        let format = &projection.text().spans()[0].format;
        let spacing = format.list_spacing.unwrap_or(7.2);
        if !spacing.is_finite() || spacing < 0.0 {
            return Err(LayoutError::InvalidSpacing);
        }
        // Stored newest first; OneNote lists and paints tags oldest first.
        let mut tag_definitions = paragraph
            .tags
            .iter()
            .chain(&source.tags)
            .map(
                |tag| match tag.definition.as_ref().and_then(|id| definitions.get(id)) {
                    Some(Definition {
                        kind:
                            Kind::TagDefinition {
                                shape,
                                label,
                                color,
                                highlight,
                                ..
                            },
                        ..
                    }) if tag.status & 4 == 0 => Ok((tag, *shape, label, *color, *highlight)),
                    _ => Err(LayoutError::UnsupportedContent),
                },
            )
            .collect::<Result<Vec<_>, _>>()?;
        tag_definitions.reverse();
        // The newest tag that sets a colour wins.
        let color = tag_definitions.iter().rev().find_map(|tag| tag.3);
        let highlight = tag_definitions.iter().rev().find_map(|tag| tag.4);
        let equation = onestore::page::Math::is_equation(&source.text);
        let mut text = if color.is_none() && highlight.is_none() && !equation {
            engine.layout(projection.text(), width - indent)?
        } else {
            let visible = projection.text();
            let mut start = 0;
            let runs = visible.spans().iter().map(|span| {
                let mut format = span.format.clone();
                format.color = color.or(format.color);
                format.highlight = highlight.or(format.highlight);
                // An equation draws in two dimensions; its linear text only holds the caret,
                // which the body font sizes like a line of text.
                if equation {
                    format.font = None;
                }
                let run = (visible.text()[start..span.end].to_owned(), format);
                start = span.end;
                run
            });
            engine.layout(
                &Paragraph::from_runs(runs.collect::<Vec<_>>()),
                width - indent,
            )?
        };
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
        for tag in &tag_definitions {
            let (tag, shape, label) = (tag.0, tag.1, tag.2);
            let icon = match shape {
                Some(0) => continue,
                Some(3) => TagIcon::CheckBox {
                    checked: tag.status & 1 != 0,
                },
                Some(15) => TagIcon::Question,
                Some(17) => TagIcon::Exclamation,
                Some(100) => TagIcon::RedSquare,
                Some(101) => TagIcon::YellowSquare,
                Some(102) => TagIcon::BlueSquare,
                Some(121) => TagIcon::Music,
                _ => return Err(LayoutError::UnsupportedContent),
            };
            // Later tags follow the first to its right, toward the text.
            let x = marker_x - 20.25 + 12.0 * tags.len() as f32;
            tags.push(ParagraphTag {
                icon,
                origin: [x, 0.0],
                label: label.clone().unwrap_or_default(),
                disabled: tag.status & 2 != 0,
            });
        }
        let math = equation
            .then(|| crate::math::layout(engine, &source.text))
            .transpose()?;
        Ok(Self {
            id: paragraph.id,
            origin: [indent, 0.0],
            projection,
            text,
            markers,
            tags,
            math,
        })
    }
}

pub(crate) fn visible_paragraphs<'a>(
    nodes: impl Iterator<Item = &'a PageParagraph>,
) -> impl Iterator<Item = &'a PageParagraph> {
    let mut hidden = BTreeSet::new();
    nodes.filter(move |paragraph| {
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
    let mut previous_after = None;
    let mut content_width = 0.0_f32;
    for paragraph in paragraphs {
        let format = &paragraph.projection.text().spans()[0].format;
        origins.push(spacing(&mut bottom, &mut previous_after, format)?);
        let size = paragraph.size();
        bottom += f64::from(size[1]);
        if !(bottom as f32).is_finite() {
            return Err(LayoutError::InvalidSpacing);
        }
        content_width = content_width.max(size[0]);
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
    /// OneNote gives an outline one tag column, as wide as its most-tagged paragraph needs; each
    /// paragraph's tags start at the column's left edge. Tag origins assume a one-tag column.
    pub fn tag_column_offset(&self) -> f32 {
        let widest = self
            .paragraphs
            .iter()
            .map(|p| p.tags.len())
            .max()
            .unwrap_or(0);
        -12.0 * widest.saturating_sub(1) as f32
    }

    /// Innermost table cell containing a visible paragraph index.
    pub fn paragraph_cell(&self, index: usize) -> Option<&CellLayout> {
        self.tables.iter().rev().find_map(|table| {
            let cell = table
                .cells
                .partition_point(|cell| cell.paragraphs.end <= index);
            table
                .cells
                .get(cell)
                .filter(|cell| cell.paragraphs.contains(&index))
        })
    }

    fn append(&mut self, mut child: Self, origin: [f32; 2]) {
        for paragraph in &mut child.paragraphs {
            paragraph.origin[0] += origin[0];
            paragraph.origin[1] += origin[1];
            for (_, marker) in &mut paragraph.markers {
                marker[0] += origin[0];
            }
            for tag in &mut paragraph.tags {
                tag.origin[0] += origin[0];
            }
        }
        for table in &mut child.tables {
            for cell in &mut table.cells {
                cell.paragraphs.start += self.paragraphs.len();
                cell.paragraphs.end += self.paragraphs.len();
                for (value, offset) in cell.rect.iter_mut().zip(origin.into_iter().cycle()) {
                    *value += offset;
                }
            }
        }
        for object in &mut child.objects {
            for (value, offset) in object.rect.iter_mut().zip(origin.into_iter().cycle()) {
                *value += offset;
            }
            object.bottom += origin[1];
            if let ObjectKind::File(label) | ObjectKind::Unsupported(label) = &mut object.kind {
                label.reset_origin(label.origin[0] + origin[0]);
                label.origin[1] += origin[1];
            }
        }
        self.paragraphs.extend(child.paragraphs);
        self.tables.extend(child.tables);
        self.objects.extend(child.objects);
    }

    pub(crate) fn flow<'a>(
        nodes: impl Iterator<Item = &'a PageParagraph>,
        indents: &[f32],
        width: f32,
        fixed_width: bool,
        depth: usize,
        edit: Option<&DocumentEdit>,
        shape: &mut impl FnMut(&PageParagraph, f32, &[f32]) -> Result<ParagraphLayout, LayoutError>,
    ) -> Result<Self, LayoutError> {
        if !width.is_finite() || width <= 0.0 {
            return Err(LayoutError::InvalidWidth);
        }
        if depth > 64 {
            return Err(LayoutError::UnsupportedContent);
        }
        let mut result = Self {
            paragraphs: Vec::new(),
            tables: Vec::new(),
            objects: Vec::new(),
            size: [36.0, 0.0],
        };
        let mut bottom = 0.0;
        let mut previous = None;
        for node in visible_paragraphs(nodes) {
            match &node.content {
                ParagraphContent::Text(_) => {
                    let mut paragraph = shape(node, width, indents)?;
                    let y = spacing(
                        &mut bottom,
                        &mut previous,
                        &paragraph.projection.text().spans()[0].format,
                    )?;
                    let size = paragraph.size();
                    paragraph.origin[1] = y;
                    result.paragraphs.push(paragraph);
                    result.size[0] = result.size[0].max(size[0]);
                    bottom += f64::from(size[1]);
                }
                ParagraphContent::Table(table) => {
                    let x = indentation(node.level, indents, width)?;
                    let y = spacing(&mut bottom, &mut previous, &node.format)?;
                    let child = Self::table(table, depth + 1, edit, shape)?;
                    result.size[0] = result.size[0].max(x + child.size[0]);
                    bottom += f64::from(child.size[1]);
                    result.append(child, [x, y]);
                }
                ParagraphContent::Image(image) => {
                    let x = indentation(node.level, indents, width)?;
                    let y = spacing(&mut bottom, &mut previous, &node.format)?;
                    let [w, h] = image_size(image).ok_or(LayoutError::UnsupportedContent)?;
                    result.objects.push(ObjectLayout {
                        id: image.id,
                        rect: [x, y, x + w, y + h],
                        kind: ObjectKind::Picture,
                        bottom: y + h,
                    });
                    result.size[0] = result.size[0].max(x + w);
                    bottom += f64::from(h);
                }
                ParagraphContent::Attachment(file) => {
                    let x = indentation(node.level, indents, width)?;
                    let y = spacing(&mut bottom, &mut previous, &node.format)?;
                    let [w, h] = file.size.unwrap_or([24.0, 24.0]);
                    let icon = [x + (ATTACHMENT_WIDTH - w) / 2.0, y + 6.0];
                    // OneNote shows the name without its extension.
                    let name = std::path::Path::new(&file.filename)
                        .file_stem()
                        .and_then(|stem| stem.to_str())
                        .unwrap_or(&file.filename);
                    let format = Format {
                        alignment: Some(1),
                        ..node.format.clone()
                    };
                    let mut label = shape(
                        &caption(node.id, file.id, name, format),
                        ATTACHMENT_WIDTH,
                        &[0.0],
                    )?;
                    label.reset_origin(x);
                    label.origin[1] = icon[1] + h + 10.5;
                    let end = label.origin[1] + label.text.height() + 9.0;
                    result.objects.push(ObjectLayout {
                        id: file.id,
                        rect: [icon[0], icon[1], icon[0] + w, icon[1] + h],
                        kind: ObjectKind::File(label),
                        bottom: end,
                    });
                    result.size[0] = result.size[0].max(x + ATTACHMENT_WIDTH);
                    bottom += f64::from(end - y);
                }
                ParagraphContent::Ink(ink) => {
                    let x = indentation(node.level, indents, width)?;
                    let y = spacing(&mut bottom, &mut previous, &node.format)?;
                    // The paragraph reaches from its origin to the farthest stroke point.
                    let [w, h] = ink
                        .bounds()
                        .map(|[x, y, w, h]| [x + w, y + h])
                        .filter(|size| size.iter().all(|v| v.is_finite() && *v >= 0.0))
                        .ok_or(LayoutError::UnsupportedContent)?;
                    result.objects.push(ObjectLayout {
                        id: ink.id,
                        rect: [x, y, x + w, y + h],
                        kind: ObjectKind::Ink(ink.clone()),
                        bottom: y + h,
                    });
                    result.size[0] = result.size[0].max(x + w);
                    bottom += f64::from(h);
                }
                ParagraphContent::Unsupported(unsupported) => {
                    let x = indentation(node.level, indents, width)?;
                    let y = spacing(&mut bottom, &mut previous, &node.format)?;
                    let w = unsupported.layout.max_width.unwrap_or(160.0).max(160.0);
                    let mut label = shape(
                        &caption(
                            node.id,
                            unsupported.id,
                            "Unsupported content",
                            Format::default(),
                        ),
                        w - 16.0,
                        &[0.0],
                    )?;
                    label.reset_origin(x + 8.0);
                    label.origin[1] = y + 8.0;
                    let h = unsupported
                        .layout
                        .max_height
                        .unwrap_or(0.0)
                        .max(label.text.height() + 16.0);
                    result.objects.push(ObjectLayout {
                        id: unsupported.id,
                        rect: [x, y, x + w, y + h],
                        kind: ObjectKind::Unsupported(label),
                        bottom: y + h,
                    });
                    result.size[0] = result.size[0].max(x + w);
                    bottom += f64::from(h);
                }
            }
            if !(bottom as f32).is_finite() || !result.size[0].is_finite() {
                return Err(LayoutError::InvalidSpacing);
            }
        }
        result.size[1] = bottom as f32;
        result.size[0] = if fixed_width { width } else { result.size[0] }.max(result.table_width());
        Ok(result)
    }

    pub(crate) fn table_width(&self) -> f32 {
        // Stored column widths extend 1.77pt past the final cell border.
        self.tables
            .iter()
            .filter_map(|table| table.cells.last())
            .map(|cell| cell.rect[2] + 1.77)
            .fold(0.0, f32::max)
    }

    fn table(
        table: &Table,
        depth: usize,
        edit: Option<&DocumentEdit>,
        shape: &mut impl FnMut(&PageParagraph, f32, &[f32]) -> Result<ParagraphLayout, LayoutError>,
    ) -> Result<Self, LayoutError> {
        let widths = edit.and_then(|edit| edit.columns.get(&table.id));
        if widths.is_some_and(|widths| widths.len() != table.columns.len()) {
            return Err(LayoutError::InvalidWidth);
        }
        let width =
            |index: usize| widths.map_or(table.columns[index].width, |widths| widths[index]);
        if table.columns.is_empty()
            || table.rows.is_empty()
            || table
                .columns
                .iter()
                .enumerate()
                .any(|(index, _)| !width(index).is_finite() || width(index) < 36.0)
        {
            return Err(LayoutError::InvalidWidth);
        }
        if !table.tags.is_empty() {
            return Err(LayoutError::UnsupportedContent);
        }
        let mut result = Self {
            paragraphs: Vec::new(),
            tables: vec![TableLayout {
                id: table.id,
                cells: Vec::new(),
                borders: table.borders.unwrap_or(true),
            }],
            objects: Vec::new(),
            size: [
                (0..table.columns.len())
                    .map(|index| width(index) + 4.98)
                    .sum::<f32>()
                    - 1.83,
                3.54,
            ],
        };
        let mut y = 0.0;
        for row in &table.rows {
            if row.cells.len() != table.columns.len() {
                return Err(LayoutError::InvalidWidth);
            }
            let start = result.tables[0].cells.len();
            let mut x = 0.0;
            let mut height = 0.0_f32;
            for (index, cell) in row.cells.iter().enumerate() {
                let width = width(index);
                if cell.paragraphs.is_empty() || !cell.unsupported.is_empty() {
                    return Err(LayoutError::UnsupportedContent);
                }
                let child = Self::flow(
                    edited_nodes(&cell.paragraphs, Some(cell.id), edit),
                    &cell.indents,
                    width,
                    false,
                    depth,
                    edit,
                    shape,
                )?;
                height = height.max(child.size[1]);
                let paragraph_start = result.paragraphs.len();
                result.append(child, [x, y + 3.54]);
                result.tables[0].cells.push(CellLayout {
                    id: cell.id,
                    rect: [x - 3.6, y + 1.86, x + width + 1.38, 0.0],
                    paragraphs: paragraph_start..result.paragraphs.len(),
                });
                x += width + 4.98;
            }
            y += height + 4.98;
            for cell in &mut result.tables[0].cells[start..] {
                cell.rect[3] = y + 1.86;
            }
        }
        result.size[1] += y;
        if result.size.iter().any(|v| !v.is_finite()) {
            return Err(LayoutError::InvalidSpacing);
        }
        Ok(result)
    }

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
        Ok(Self {
            paragraphs,
            tables: Vec::new(),
            objects: Vec::new(),
            size,
        })
    }
}

/// Shapes a page object into positioned paragraph layouts.
pub trait Arrange {
    type Output;
    fn layout(
        &self,
        engine: &mut TextEngine,
        definitions: &BTreeMap<ExGuid, Definition>,
    ) -> Result<Self::Output, LayoutError>;
}

impl Arrange for Outline {
    type Output = OutlineLayout;

    fn layout(
        &self,
        engine: &mut TextEngine,
        definitions: &BTreeMap<ExGuid, Definition>,
    ) -> Result<OutlineLayout, LayoutError> {
        let width = self
            .layout
            .reserved_width
            .or(self.layout.max_width)
            .ok_or(LayoutError::InvalidWidth)?;
        outline_layout(self, engine, definitions, width)
    }
}

/// Outlines of plain paragraphs stack them; tables and pictures, files or ink need the full flow.
pub(crate) fn all_text<'a>(nodes: impl IntoIterator<Item = &'a PageParagraph>) -> bool {
    nodes
        .into_iter()
        .all(|node| matches!(node.content, ParagraphContent::Text(_)))
}

pub(crate) fn outline_layout(
    outline: &Outline,
    engine: &mut TextEngine,
    definitions: &BTreeMap<ExGuid, Definition>,
    width: f32,
) -> Result<OutlineLayout, LayoutError> {
    if !outline.unsupported.is_empty() {
        return Err(LayoutError::UnsupportedContent);
    }
    if !all_text(&outline.paragraphs) {
        return OutlineLayout::flow(
            outline.paragraphs.iter(),
            &outline.indents,
            width,
            outline.layout.width_set_by_user == Some(true),
            0,
            None,
            &mut |node, width, indents| {
                ParagraphLayout::shape(engine, node, width, indents, definitions)
            },
        );
    }
    let paragraphs = visible_paragraphs(outline.paragraphs.iter())
        .map(|p| ParagraphLayout::shape(engine, p, width, &outline.indents, definitions))
        .collect::<Result<_, _>>()?;
    OutlineLayout::new(
        paragraphs,
        width,
        outline.layout.width_set_by_user == Some(true),
    )
}

impl Arrange for Title {
    type Output = Vec<([f32; 2], OutlineLayout)>;

    fn layout(
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
                .unwrap_or(TITLE_WIDTH);
            let layout = outline_layout(outline, engine, definitions, width)?;
            let origin = [
                outline.layout.x.unwrap_or(0.0),
                bottom + outline.layout.y.unwrap_or(0.0),
            ];
            let height = outline.layout.max_height.unwrap_or(0.0);
            if origin.iter().any(|v| !v.is_finite()) || !height.is_finite() || height < 0.0 {
                return Err(LayoutError::InvalidSpacing);
            }
            bottom = origin[1] + layout.size[1].max(height) + if outline.title { 3.6 } else { 0.0 };
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
    use onestore::document::Layout;
    use onestore::page::{PageParagraph, TextObject};

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
            content: onestore::page::ParagraphContent::Text(TextObject {
                date_field: None,
                id: ExGuid {
                    n: n + 100,
                    ..ExGuid::default()
                },
                text: Paragraph::new(text.into(), Format::default()),
                tags: Vec::new(),
            }),
            lists: Vec::new(),
            tags: Vec::new(),
            media: Default::default(),
            collapsed: false,
            style: None,
        }
    }

    fn table(rows: &[&[&str]], widths: &[f32], mut n: u32) -> PageParagraph {
        use onestore::page::{TableCell, TableColumn, TableRow};
        let mut id = || {
            n += 1;
            ExGuid {
                n,
                ..ExGuid::default()
            }
        };
        let mut node = paragraph(id().n, "", 1, None);
        node.content = ParagraphContent::Table(Table {
            id: id(),
            columns: widths
                .iter()
                .map(|width| TableColumn {
                    width: *width,
                    locked: false,
                })
                .collect(),
            rows: rows
                .iter()
                .map(|row| TableRow {
                    id: id(),
                    cells: row
                        .iter()
                        .map(|text| TableCell {
                            id: id(),
                            layout: Layout::default(),
                            indents: vec![18.0, 0.0, 27.0],
                            shading: None,
                            paragraphs: vec![paragraph(id().n, text, 1, None)],
                            unsupported: Vec::new(),
                        })
                        .collect(),
                })
                .collect(),
            borders: Some(true),
            layout: Layout::default(),
            tags: Vec::new(),
        });
        node
    }

    #[test]
    fn table_rows_align_cells_and_expand_for_wrapped_text() {
        let mut engine = TextEngine::default();
        let mut outline = Outline {
            id: ExGuid::default(),
            title: false,
            min_width: None,
            layout: Layout {
                max_width: Some(300.0),
                ..Layout::default()
            },
            indents: vec![18.0, 0.0, 27.0],
            paragraphs: vec![
                paragraph(1, "Before", 1, None),
                table(
                    &[
                        &[
                            "A long paragraph that wraps inside a single table cell",
                            "B",
                        ],
                        &["C", "D"],
                    ],
                    &[72.0, 48.0],
                    1000,
                ),
                paragraph(2, "After", 1, None),
            ],
            unsupported: Vec::new(),
        };
        let definitions = BTreeMap::new();
        let layout = outline.layout(&mut engine, &definitions).unwrap();
        assert_eq!(layout.paragraphs.len(), 6);
        assert_eq!(layout.tables.len(), 1);
        let cells = &layout.tables[0].cells;
        assert_eq!(cells.len(), 4);
        assert_eq!(
            cells
                .iter()
                .map(|cell| cell.paragraphs.clone())
                .collect::<Vec<_>>(),
            [1..2, 2..3, 3..4, 4..5]
        );
        assert!(layout.paragraph_cell(0).is_none());
        assert_eq!(layout.paragraph_cell(4).unwrap().id, cells[3].id);
        assert!(layout.paragraph_cell(5).is_none());
        let paragraphs = &layout.paragraphs;
        assert!(paragraphs[1].text.lines().count() > 1);
        assert_eq!(paragraphs[2].text.lines().count(), 1);
        assert_eq!(paragraphs[1].origin[1], paragraphs[2].origin[1]);
        assert_eq!(paragraphs[3].origin[1], paragraphs[4].origin[1]);
        assert_eq!(cells[0].rect[3], cells[1].rect[3]);
        assert_eq!(cells[0].rect[3], cells[2].rect[1]);
        assert!((cells[0].rect[2] - cells[1].rect[0]).abs() < 0.00001);
        assert_eq!(paragraphs[1].origin[0], 0.0);
        assert_eq!(paragraphs[2].origin[0], 76.98);
        assert!(paragraphs[3].origin[1] > paragraphs[1].origin[1] + paragraphs[1].text.height());
        assert!(paragraphs[5].origin[1] > cells[3].rect[3]);
        assert_eq!(
            layout.size[1],
            paragraphs[5].origin[1] + paragraphs[5].text.height()
        );
        let ParagraphContent::Table(source) = &outline.paragraphs[1].content else {
            panic!()
        };
        assert_eq!(layout.tables[0].id, source.id);
        assert_eq!(
            cells.iter().map(|c| c.id).collect::<Vec<_>>(),
            source
                .rows
                .iter()
                .flat_map(|row| row.cells.iter().map(|c| c.id))
                .collect::<Vec<_>>()
        );

        outline.layout.width_set_by_user = Some(true);
        let fixed = outline.layout(&mut engine, &definitions).unwrap();
        assert_eq!(fixed.size, [300.0, layout.size[1]]);
        assert_eq!(fixed.paragraphs[2].origin, paragraphs[2].origin);
        let ParagraphContent::Table(source) = &mut outline.paragraphs[1].content else {
            panic!()
        };
        source.columns[0].width = 160.0;
        let widened = outline.layout(&mut engine, &definitions).unwrap();
        assert!(widened.size[1] < fixed.size[1]);
        assert_eq!(widened.paragraphs[2].origin[0], 164.98);
    }

    #[test]
    fn nested_table_layout_translates_cells_and_respects_collapsed_children() {
        let mut outer = table(&[&["Left", "Right"]], &[160.0, 72.0], 1000);
        let mut inner = table(&[&["Nested", "Cell"]], &[48.0, 48.0], 2000);
        inner.level = 2;
        let ParagraphContent::Table(source) = &mut outer.content else {
            panic!()
        };
        let hidden = paragraph(
            42,
            "Hidden descendant",
            2,
            Some(source.rows[0].cells[0].paragraphs[0].id.n),
        );
        source.rows[0].cells[0].paragraphs[0].collapsed = true;
        source.rows[0].cells[0].paragraphs.push(hidden);
        source.rows[0].cells[0].paragraphs.push(inner);
        let mut engine = TextEngine::default();
        let layout = OutlineLayout::flow(
            [&outer].into_iter(),
            &[18.0, 0.0, 27.0],
            300.0,
            false,
            0,
            None,
            &mut |node, width, indents| {
                ParagraphLayout::shape(&mut engine, node, width, indents, &BTreeMap::new())
            },
        )
        .unwrap();
        assert_eq!(layout.tables.len(), 2);
        assert_eq!(layout.tables[0].cells[0].paragraphs, 0..3);
        assert_eq!(layout.tables[0].cells[1].paragraphs, 3..4);
        assert_eq!(layout.tables[1].cells[0].paragraphs, 1..2);
        assert_eq!(layout.tables[1].cells[1].paragraphs, 2..3);
        assert_eq!(
            layout.paragraph_cell(1).unwrap().id,
            layout.tables[1].cells[0].id
        );
        assert_eq!(
            layout
                .paragraphs
                .iter()
                .map(|p| p.projection.text().text())
                .collect::<Vec<_>>(),
            ["Left", "Nested", "Cell", "Right"]
        );
        assert_eq!(layout.paragraphs[1].origin[0], 27.0);
        assert_eq!(layout.tables[1].cells[0].rect[0], 27.0 - 3.6);
        assert!(layout.tables[1].cells[0].rect[1] > layout.tables[0].cells[0].rect[1]);
        assert!(layout.tables[1].cells[0].rect[3] < layout.tables[0].cells[0].rect[3]);
        assert_eq!(
            layout.paragraphs[0].origin[1],
            layout.paragraphs[3].origin[1]
        );
    }

    #[test]
    #[ignore = "requires CANVAS_TEST_SECTION native Tab capture and CANVAS_TEST_SUBSTITUTE Carlito font"]
    fn native_table_layout() {
        use onestore::page::{Page, PageObject};
        use onestore::{RevisionIndex, Store, document::Document};
        let bytes = std::fs::read(std::env::var_os("CANVAS_TEST_SECTION").unwrap()).unwrap();
        let store = Store::parse(&bytes).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let document = Document::parse(&index).unwrap();
        let mut engine = TextEngine::default();
        engine
            .register_substitute(parley::fontique::Blob::new(std::sync::Arc::new(
                std::fs::read(std::env::var_os("CANVAS_TEST_SUBSTITUTE").unwrap()).unwrap(),
            )))
            .unwrap();
        let page = Page::from_document(&document, "rows").unwrap();
        let outline = page
            .objects
            .iter()
            .find_map(|object| match object {
                PageObject::Outline(outline) => Some(outline),
                _ => None,
            })
            .unwrap();
        let layout = outline.layout(&mut engine, &page.definitions).unwrap();
        assert!((layout.size[0] - 84.6).abs() < 0.001);
        assert!((layout.size[1] - 58.76315).abs() < 0.001);
        for (paragraph, expected) in layout.paragraphs.iter().zip([
            [0.0, 3.54],
            [44.34, 3.54],
            [0.0, 21.947714],
            [44.34, 21.947714],
            [0.0, 40.355427],
            [44.34, 40.355427],
        ]) {
            assert_eq!(paragraph.text.lines().count(), 1);
            for (actual, expected) in paragraph.origin.into_iter().zip(expected) {
                assert!((actual - expected).abs() < 0.001);
            }
        }
        assert_eq!(layout.tables.len(), 1);
        assert_eq!(layout.tables[0].cells.len(), 6);
        for (title, size) in [
            ("soft-break", [82.350006, 35.375435]),
            ("fixed", [180.0, 21.947721]),
        ] {
            let page = Page::from_document(&document, title).unwrap();
            let outline = page
                .objects
                .iter()
                .find_map(|object| match object {
                    PageObject::Outline(outline) => Some(outline),
                    _ => None,
                })
                .unwrap();
            let layout = outline.layout(&mut engine, &page.definitions).unwrap();
            for (actual, expected) in layout.size.into_iter().zip(size) {
                assert!(
                    (actual - expected).abs() < 0.001,
                    "{title}: {actual} != {expected}"
                );
            }
            assert_eq!(layout.tables.len(), 1);
            assert_eq!(layout.tables[0].cells.len(), 2);
            assert_eq!(
                layout
                    .paragraphs
                    .iter()
                    .map(|p| p.text.lines().count())
                    .max(),
                Some(if title == "soft-break" { 2 } else { 1 })
            );
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
        node.text_mut().unwrap().tags.push(Tag {
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
            (17, TagIcon::Exclamation),
            (100, TagIcon::RedSquare),
            (101, TagIcon::YellowSquare),
            (102, TagIcon::BlueSquare),
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
    fn files_and_ink_take_their_own_paragraph_in_the_flow() {
        use onestore::page::{Attachment, Ink, InkStroke};
        let before = paragraph(1, "Before", 1, None);
        let mut file = paragraph(2, "", 1, None);
        file.content = ParagraphContent::Attachment(Attachment {
            id: ExGuid {
                n: 20,
                ..ExGuid::default()
            },
            filename: "notes 🦀.txt".into(),
            source_path: None,
            size: Some([24.0, 24.0]),
            bytes: None,
            preview: None,
            recording: None,
        });
        let mut ink = paragraph(3, "", 1, None);
        ink.content = ParagraphContent::Ink(Ink {
            id: ExGuid {
                n: 30,
                ..ExGuid::default()
            },
            layout: Default::default(),
            strokes: vec![InkStroke {
                id: ExGuid::default(),
                points: vec![[300.0, 120.0], [360.0, 180.0]],
                width: 1.0,
                height: 1.0,
                color: None,
                transparency: None,
                pen_tip: None,
            }],
            groups: Vec::new(),
        });
        let after = paragraph(4, "After", 1, None);
        let mut engine = TextEngine::default();
        let layout = OutlineLayout::flow(
            [&before, &file, &ink, &after].into_iter(),
            &[0.0],
            468.0,
            false,
            0,
            None,
            &mut |node, width, indents| {
                ParagraphLayout::shape(&mut engine, node, width, indents, &BTreeMap::new())
            },
        )
        .unwrap();
        let top = layout.paragraphs[0].text.height();
        let [icon, handwriting] = [&layout.objects[0], &layout.objects[1]];
        assert_eq!(icon.rect, [15.0, top + 6.0, 39.0, top + 30.0]);
        let label = icon.label().unwrap();
        assert_eq!(label.projection.text().text(), "notes 🦀");
        assert_eq!(label.origin, [0.0, top + 40.5]);
        assert_eq!(icon.bottom, label.origin[1] + label.text.height() + 9.0);
        assert_eq!(
            handwriting.rect,
            [0.0, icon.bottom, 360.0, icon.bottom + 180.0]
        );
        assert_eq!(layout.paragraphs[1].origin[1], handwriting.bottom);
        assert_eq!(layout.size[0], 360.0);
    }

    #[test]
    fn tags_paint_oldest_first_and_the_newest_colour_wins() {
        use onestore::document::Tag;
        let mut definitions = BTreeMap::new();
        let mut tag = |n, shape, color| {
            let id = ExGuid {
                n,
                ..ExGuid::default()
            };
            definitions.insert(
                id,
                Definition {
                    kind: Kind::TagDefinition {
                        shape: Some(shape),
                        label: None,
                        action_type: None,
                        color,
                        highlight: None,
                    },
                    format: Format::default(),
                },
            );
            Tag {
                definition: Some(id),
                status: 1,
                action_type: None,
                created: None,
                completed: None,
                start: None,
                due: None,
                task_id: None,
                extra_set: 0,
            }
        };
        let [action, mood, project, question] = [
            tag(1, 0, Some(0x0080_0080)),
            tag(2, 0, Some(0x0080_8000)),
            tag(3, 100, None),
            tag(4, 15, None),
        ];
        let mut node = paragraph(1, "Lyric", 1, None);
        // As stored: newest first.
        node.text_mut().unwrap().tags = vec![question, project, mood, action];
        let mut engine = TextEngine::default();
        let shaped =
            ParagraphLayout::shape(&mut engine, &node, 200.0, &[0.0, 0.0], &definitions).unwrap();
        assert_eq!(
            shaped
                .tags
                .iter()
                .map(|tag| (tag.icon, tag.origin[0]))
                .collect::<Vec<_>>(),
            [(TagIcon::RedSquare, -20.25), (TagIcon::Question, -8.25)]
        );
        let colors: Vec<_> = shaped
            .text
            .lines()
            .flat_map(|(line, _)| line.items().collect::<Vec<_>>())
            .filter_map(|item| match item {
                parley::PositionedLayoutItem::GlyphRun(run) => Some(run.style().brush.color),
                _ => None,
            })
            .collect();
        assert_eq!(colors, [0x0080_8000]);
        assert_eq!(shaped.projection.text().spans()[0].format.color, None);
        let outline = OutlineLayout::new(vec![shaped], 200.0, false).unwrap();
        assert_eq!(outline.tag_column_offset(), -12.0);
    }

    #[test]
    fn title_uses_a_default_wrap_limit_and_keeps_date_after_wrapped_title() {
        let mut title = Title {
            date: None,
            id: ExGuid::default(),
            layout: Layout::default(),
            outlines: ["A title with enough words to wrap", "A date"]
                .into_iter()
                .enumerate()
                .map(|(index, text)| Outline {
                    title: index == 0,
                    min_width: None,
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
        assert_eq!(layouts[1].0, [0.0, 21.6 + 3.6]);
        title.outlines[0].paragraphs[0].text_mut().unwrap().text = Paragraph::new(
            "A title with enough words to wrap ".repeat(12),
            Format::default(),
        );
        let layouts = title.layout(&mut engine, &definitions).unwrap();
        assert!(layouts[0].1.paragraphs[0].text.lines().count() > 1);
        assert!(layouts[0].1.size[0] <= 468.0);
        assert_eq!(layouts[1].0[1], layouts[0].1.size[1] + 3.6);
        title.outlines[0].layout.max_width = Some(70.0);
        let layouts = title.layout(&mut engine, &definitions).unwrap();
        assert!(layouts[0].1.paragraphs[0].text.lines().count() > 1);
        assert_eq!(layouts[1].0[1], layouts[0].1.size[1] + 3.6);
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
        first.text_mut().unwrap().text = Paragraph::new(
            "First".into(),
            Format {
                space_after: Some(4.0),
                ..Format::default()
            },
        );
        let mut second = paragraph(2, "Second", 2, Some(1));
        second.text_mut().unwrap().text = Paragraph::new(
            "Second".into(),
            Format {
                space_before: Some(7.0),
                space_after: Some(3.0),
                ..Format::default()
            },
        );
        second.collapsed = true;
        let mut outline = Outline {
            title: false,
            min_width: None,
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
        // OneNote stores only the level-zero indent for an outline of top-level paragraphs.
        outline.indents = vec![0.0];
        outline.paragraphs.truncate(1);
        let single = outline.layout(&mut engine, &BTreeMap::new()).unwrap();
        assert_eq!(single.paragraphs[0].origin[0], 0.0);
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
            title: false,
            min_width: None,
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
