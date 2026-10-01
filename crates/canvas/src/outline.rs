use crate::{
    document::{DocumentEdit, descendants, edited_nodes, leaves},
    layout::{InlineSpace, LayoutError, TextEngine, TextLayout},
};
use onestore::page::text::{Paragraph, TextProjection};
use onestore::page::{Definition, Outline, PageParagraph, ParagraphContent, Table, Title};
use onestore::{
    ExGuid,
    document::{Format, Kind},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Range,
    sync::Arc,
};

pub(crate) const TITLE_WIDTH: f32 = 468.0;

#[derive(Clone, Default)]
pub struct OutlineLayout {
    pub paragraphs: Vec<ParagraphLayout>,
    pub tables: Vec<TableLayout>,
    /// Pictures, files and handwriting that occupy a paragraph of their own.
    pub(crate) objects: Vec<ObjectLayout>,
    pub size: [f32; 2],
    /// One per root node, so an edit places anew only what it changes and moves what follows.
    blocks: Vec<Block>,
    /// The widest root node, at least 36 points, and the widest root text's line boxes; kept so
    /// an edit sizes an automatic width without measuring every node.
    pub(crate) widths: [f32; 2],
}

/// A root node's place in its outline's flow.
#[derive(Clone)]
struct Block {
    /// Where the node's paragraphs, tables and objects start.
    first: [usize; 3],
    /// Spacing before and after the node and its own height, or `None` while hidden.
    metrics: Option<[f32; 3]>,
    /// The node's right edge and, for text, its line boxes' right edge.
    extent: [f32; 2],
    /// The flow's bottom and pending spacing after the node.
    state: (f64, Option<f32>),
    /// A table's vertical coordinates before its offset.
    rel: Vec<f32>,
    /// The number the next numbered sibling at the node's level continues from.
    count: Option<Count>,
}

/// A layout of root nodes `range` once an edit applies, and how the nodes after them move.
pub(crate) struct Relayout {
    pub(crate) range: Range<usize>,
    pub(crate) segment: OutlineLayout,
    /// Each following node's new top and flow state, up to the first that stays.
    moved: Vec<(f32, (f64, Option<f32>))>,
    pub(crate) size: [f32; 2],
    widths: [f32; 2],
}

#[derive(Clone)]
pub(crate) struct ObjectLayout {
    /// The object's identity, which keys a picture's or file icon's decoded image.
    pub(crate) id: ExGuid,
    /// Where the picture, file icon, drawing or placeholder draws, outline-local.
    pub(crate) rect: [f32; 4],
    pub(crate) kind: ObjectKind,
    /// Outline-local bottom of the whole object, label included.
    pub(crate) bottom: f32,
    pub(crate) tags: Option<BlockTags>,
}

/// The note tags of a table, picture or file, which OneNote centres on it in the tag column.
#[derive(Clone)]
pub(crate) struct BlockTags {
    /// The paragraph holding the block.
    pub(crate) paragraph: ExGuid,
    /// Its siblings' group, whose list markers the tags clear.
    parent: Option<ExGuid>,
    /// The block's left edge, outline-local.
    x: f32,
    /// Origins are outline-local.
    pub(crate) tags: Vec<ParagraphTag>,
}

impl BlockTags {
    /// `node`'s tags and those of its content, `content`, shaped as a tagged paragraph of
    /// `node`'s format would show them; `None` without any.
    fn new(
        node: &PageParagraph,
        content: &[onestore::document::Tag],
        x: f32,
        shape: &mut impl FnMut(
            &PageParagraph,
            Option<&Count>,
            f32,
            &[f32],
        ) -> Result<ParagraphLayout, LayoutError>,
    ) -> Result<Option<Self>, LayoutError> {
        if node.tags.is_empty() && content.is_empty() {
            return Ok(None);
        }
        let mut tagged = caption(node.id, node.id, "", node.format.clone());
        tagged.tags.clone_from(&node.tags);
        if let ParagraphContent::Text(text) = &mut tagged.content {
            text.tags = content.to_vec();
        }
        let tags = shape(&tagged, None, ATTACHMENT_WIDTH, &[0.0, 0.0])?.tags;
        Ok(Some(Self {
            paragraph: node.id,
            parent: node.parent,
            x,
            tags,
        }))
    }

    /// Centres the tags on a block spanning `top..bottom`.
    fn centre(&mut self, top: f32, bottom: f32) {
        for tag in &mut self.tags {
            tag.origin[1] = (top + bottom - tag.size) / 2.0;
        }
    }

    fn offset(&mut self, [x, y]: [f32; 2]) {
        self.x += x;
        for tag in &mut self.tags {
            tag.origin[0] += x;
            tag.origin[1] += y;
        }
    }
}

#[derive(Clone)]
pub(crate) enum ObjectKind {
    Picture,
    /// A file's icon with its name centered below.
    File(ParagraphLayout),
    /// Handwriting, whose strokes are relative to the rect's top-left.
    Ink(onestore::page::Ink),
    /// Content the canvas cannot draw, marked by a labelled box.
    Unsupported(ParagraphLayout),
}

impl ObjectLayout {
    pub(crate) fn label(&self) -> Option<&ParagraphLayout> {
        match &self.kind {
            ObjectKind::File(label) | ObjectKind::Unsupported(label) => Some(label),
            ObjectKind::Picture | ObjectKind::Ink(_) => None,
        }
    }

    /// The whole object, a file's column with its name included.
    pub fn bounds(&self) -> [f32; 4] {
        match &self.kind {
            ObjectKind::File(label) => [
                label.origin[0],
                self.rect[1] - 6.0,
                label.origin[0] + ATTACHMENT_WIDTH,
                self.bottom,
            ],
            _ => [self.rect[0], self.rect[1], self.rect[2], self.bottom],
        }
    }

    /// The rect's top and bottom, the object's bottom and its label's top once the object
    /// starts at `y`, given its own height, and the height it takes in the flow.
    fn at(&self, y: f32, height: f32) -> ([f32; 4], f32) {
        match &self.kind {
            ObjectKind::File(label) => {
                let top = y + 6.0;
                let label_top = top + height + 10.5;
                let bottom = label_top + label.text.height() + 9.0;
                ([top, top + height, bottom, label_top], bottom - y)
            }
            ObjectKind::Unsupported(_) => ([y, y + height, y + height, y + 8.0], height),
            ObjectKind::Picture | ObjectKind::Ink(_) => ([y, y + height, y + height, 0.0], height),
        }
    }

    fn place(&mut self, y: f32, height: f32) -> f32 {
        let ([top, end, bottom, label_top], flow) = self.at(y, height);
        [self.rect[1], self.rect[3], self.bottom] = [top, end, bottom];
        if let ObjectKind::File(label) | ObjectKind::Unsupported(label) = &mut self.kind {
            label.origin[1] = label_top;
        }
        let [_, top, _, bottom] = self.bounds();
        if let Some(tags) = &mut self.tags {
            tags.centre(top, bottom);
        }
        flow
    }
}

/// The vertical coordinates of these pieces, in a fixed order.
fn verticals<'a>(
    paragraphs: &'a mut [ParagraphLayout],
    tables: &'a mut [TableLayout],
    objects: &'a mut [ObjectLayout],
) -> impl Iterator<Item = &'a mut f32> {
    let tags = |tags: &'a mut Option<BlockTags>| {
        tags.iter_mut()
            .flat_map(|tags| &mut tags.tags)
            .map(|tag| &mut tag.origin[1])
    };
    paragraphs
        .iter_mut()
        .map(|paragraph| &mut paragraph.origin[1])
        .chain(tables.iter_mut().flat_map(move |table| {
            let cells = table.cells.iter_mut().flat_map(|cell| {
                let [_, top, _, bottom] = &mut cell.rect;
                [top, bottom]
            });
            cells.chain(tags(&mut table.tags))
        }))
        .chain(objects.iter_mut().flat_map(move |object| {
            let ObjectLayout {
                rect: [_, top, _, end],
                bottom,
                kind,
                tags: block,
                ..
            } = object;
            let label = match kind {
                ObjectKind::File(label) | ObjectKind::Unsupported(label) => {
                    Some(&mut label.origin[1])
                }
                ObjectKind::Picture | ObjectKind::Ink(_) => None,
            };
            [top, end, bottom]
                .into_iter()
                .chain(label)
                .chain(tags(block))
        }))
}

/// OneNote centers a file's icon and name in a column this wide.
const ATTACHMENT_WIDTH: f32 = 54.0;
/// The grey OneNote gives secondary text such as the page date (its `PageDateTime` style).
const PLACEHOLDER: u32 = 0x0080_8080;

#[derive(Clone)]
pub struct TableLayout {
    pub id: ExGuid,
    /// Cells are in paragraph order with disjoint visible ranges.
    pub cells: Vec<CellLayout>,
    pub borders: bool,
    pub(crate) tags: Option<BlockTags>,
}

#[derive(Clone)]
pub struct CellLayout {
    pub id: ExGuid,
    pub rect: [f32; 4],
    /// Visible paragraph indices, including paragraphs in nested tables.
    pub(crate) paragraphs: std::ops::Range<usize>,
}

impl CellLayout {
    /// Native ink gutters extend beyond the cell borders.
    pub(crate) fn text_bounds(&self) -> [f32; 4] {
        [
            self.rect[0] - 2.7,
            self.rect[1],
            self.rect[2] + 4.62,
            self.rect[3],
        ]
    }

    pub(crate) fn clip(&self, rect: parley::BoundingBox) -> Option<parley::BoundingBox> {
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
    /// A numbered paragraph's number, and whether its list restarts the count instead of
    /// continuing from the previous sibling's.
    pub(crate) number: Option<(Count, bool)>,
    pub tags: Vec<ParagraphTag>,
    /// Its siblings' group, whose list markers its tags clear: the parent paragraph, or the
    /// table cell holding a cell's top-level paragraph.
    pub(crate) parent: Option<ExGuid>,
    /// The equations drawn in two dimensions in spaces of the text, in order.
    pub math: Vec<crate::math::MathLayout>,
    /// A highlight of the whole paragraph (its own `highlight`), COLORREF: a band across its
    /// outline behind its lines, as OneNote marks a conflict page's conflicting changes.
    pub(crate) band: Option<u32>,
}

/// How the page draws a note tag.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum TagIcon {
    /// Symbol `shape` of MS-ONE's NoteTagShape, `checked` only where it is a check box.
    Symbol { shape: u16, checked: bool },
    /// An Outlook task, drawn with its stored follow-up flag and never checked from the page.
    Task { shape: u16 },
}

impl TagIcon {
    /// How the page draws tag symbol `shape`, `checked` where it is a check box; none for
    /// shape 0, which marks the text instead.
    pub fn of(shape: u16, checked: bool) -> Option<Self> {
        (shape != 0).then_some(Self::Symbol {
            shape,
            checked: checked && checkable(shape),
        })
    }

    pub(crate) fn checkable(self) -> bool {
        matches!(self, Self::Symbol { shape, .. } if checkable(shape))
    }
}

/// Whether tag symbol `shape` is a check box, as MS-ONE's NoteTagShape marks them.
pub(crate) fn checkable(shape: u16) -> bool {
    matches!(
        shape,
        1..=12 | 28 | 30 | 32 | 48 | 50 | 52 | 69 | 71 | 73 | 89..=99
    )
}

/// OneNote 2010's name for tag symbol `shape`, as its symbol gallery's tooltips give it; the
/// follow-up flags it keeps for Outlook tasks take MS-ONE's descriptions.
pub fn symbol_name(shape: u16) -> Option<&'static str> {
    SYMBOL_NAMES
        .get(usize::from(shape).checked_sub(1)?)
        .copied()
}

const SYMBOL_NAMES: [&str; 143] = [
    "Green Check Box",
    "Yellow Check Box",
    "Blue Check Box",
    "Green Star Check Box",
    "Yellow Star Check Box",
    "Blue Star Check Box",
    "Green Exclamation Check Box",
    "Yellow Exclamation Check Box",
    "Blue Exclamation Check Box",
    "Green Arrow Check Box",
    "Yellow Arrow Check Box",
    "Blue Arrow Check Box",
    "Yellow Star",
    "Follow-up Flag",
    "Question",
    "Blue Right Arrow",
    "High Priority",
    "Telephone",
    "Calendar",
    "Clock",
    "Light Bulb",
    "Pushpin",
    "Home",
    "Comment",
    "Smiley",
    "Award Ribbon",
    "Key",
    "Blue Check Box 1",
    "Blue Circle 1",
    "Blue Check Box 2",
    "Blue Circle 2",
    "Blue Check Box 3",
    "Blue Circle 3",
    "Blue 8-Point Star",
    "Blue Check Mark",
    "Blue Circle",
    "Blue Down Arrow",
    "Blue Left Arrow",
    "Blue Solid Target",
    "Blue Star",
    "Blue Sun",
    "Blue Target",
    "Blue Triangle",
    "Blue Umbrella",
    "Blue Up Arrow",
    "Blue X with Dots",
    "Blue X",
    "Green Check Box 1",
    "Green Circle 1",
    "Green Check Box 2",
    "Green Circle 2",
    "Green Check Box 3",
    "Green Circle 3",
    "Green 8-Point Star",
    "Green Check Mark",
    "Green Circle",
    "Green Down Arrow",
    "Green Left Arrow",
    "Green Right Arrow",
    "Green Solid Target",
    "Green Star",
    "Green Sun",
    "Green Target",
    "Green Triangle",
    "Green Umbrella",
    "Green Up Arrow",
    "Green X with Dots",
    "Green X",
    "Yellow Check Box 1",
    "Yellow Circle 1",
    "Yellow Check Box 2",
    "Yellow Circle 2",
    "Yellow Check Box 3",
    "Yellow Circle 3",
    "Yellow 8-Point Star",
    "Yellow Check Mark",
    "Yellow Circle",
    "Yellow Down Arrow",
    "Yellow Left Arrow",
    "Yellow Right Arrow",
    "Yellow Solid Target",
    "Yellow Sun",
    "Yellow Target",
    "Yellow Triangle",
    "Yellow Umbrella",
    "Yellow Up Arrow",
    "Yellow X with Dots",
    "Yellow X",
    "Follow Up Today Flag",
    "Follow Up Tomorrow Flag",
    "Follow Up This Week Flag",
    "Follow Up Next Week Flag",
    "No Follow Up Date Flag",
    "To Do Blue Person",
    "To Do Yellow Person",
    "To Do Green Person",
    "To Do Blue Flag",
    "To Do Yellow Flag",
    "To Do Green Flag",
    "Red Square (Project A)",
    "Yellow Square (Project B)",
    "Blue Square (Project C)",
    "Green Square",
    "Orange Square",
    "Pink Square",
    "E-mail",
    "Envelope (Closed)",
    "Envelope (Opened)",
    "Mobile phone",
    "Telephone with Clock",
    "Question Balloon",
    "Paperclip",
    "Frowning",
    "IM contact",
    "Person",
    "Two People",
    "Reminder Bell",
    "Contact (like Outlook's)",
    "Flowers Bouquet",
    "Date",
    "Music Note",
    "Movie Clip",
    "Quote Mark",
    "Globe",
    "Link Globe",
    "Laptop",
    "Plane",
    "Car",
    "Binoculars",
    "Presentation",
    "Padlock",
    "Book (Open)",
    "Notebook Icon",
    "Paper (blank with lines)",
    "Research Icon",
    "Marker",
    "Dollar sign $",
    "Coins with window behind it",
    "Schedule Task",
    "Lighting Bolt",
    "Cloud",
    "Heart",
    "Flower",
];

#[derive(Clone, Debug, PartialEq)]
pub struct ParagraphTag {
    pub icon: TagIcon,
    /// Coordinates are outline-local in x and paragraph-local in y.
    pub origin: [f32; 2],
    /// The icon's side, which follows its paragraph's first run.
    pub size: f32,
    pub label: String,
    pub disabled: bool,
}

impl ParagraphTag {
    /// The icon's side beside 10 to 17.5 pt text.
    pub(crate) const SIZE: f32 = 12.0;
    /// How far a 12 pt icon starts left of its text when no sibling has a list marker.
    pub(crate) const INSET: f32 = 20.25;
    /// Space between a tag and the leftmost list marker among its paragraph's siblings.
    const MARKER_GAP: f32 = 0.9;

    /// Where an icon of `side` starts below its paragraph's top, for a first line whose
    /// baseline is `baseline` below it and a first run of `size` points: OneNote 2010 centres
    /// it 0.357 of the run's size, less 0.2 pt, above the baseline (within 0.75 pt, 8 to 60 pt).
    fn top(baseline: f32, size: f32, side: f32) -> f32 {
        baseline - 0.357 * size + 0.2 - side / 2.0
    }

    /// OneNote 2010's icon side for text of `size` points.
    fn side(size: f32) -> f32 {
        match size {
            24.0.. => 24.0,
            18.0.. => 18.0,
            10.0.. => Self::SIZE,
            _ => 9.0,
        }
    }

    /// Where a one-tag column of `side` starts for text at `x` among siblings whose leftmost
    /// list marker starts at `marker`; its right edge stays put as the icon grows.
    fn column(x: f32, marker: Option<f32>, side: f32) -> f32 {
        let right = x - Self::INSET + Self::SIZE;
        marker.map_or(right, |marker| right.min(marker - Self::MARKER_GAP)) - side
    }
}

/// How far a 12 pt icon starts left of a picture or file on the page, as OneNote 2010 draws
/// one (`corpus/object-tags`).
const PAGE_INSET: f32 = 24.75;

/// The note tags of a picture or file on the page, whose bounds are `bounds`: oldest first in
/// a column ending left of it, centred on it; a tag whose definition is missing draws nothing.
pub(crate) fn object_tags(
    tags: &[onestore::document::Tag],
    definitions: &BTreeMap<ExGuid, Definition>,
    [x, top, _, bottom]: [f32; 4],
) -> Vec<ParagraphTag> {
    let side = ParagraphTag::side(crate::layout::DEFAULT_FONT_SIZE);
    let mut drawn: Vec<ParagraphTag> = tags
        .iter()
        .rev()
        .filter_map(|tag| {
            let (icon, label) = if tag.status & 4 != 0 {
                let shape = tag.shape.unwrap_or(0);
                (TagIcon::Task { shape }, None)
            } else {
                let Kind::TagDefinition { shape, label, .. } =
                    &definitions.get(tag.definition.as_ref()?)?.kind
                else {
                    return None;
                };
                let checked = tag.status & 1 != 0;
                (TagIcon::of(shape.unwrap_or(0), checked)?, label.as_deref())
            };
            Some(ParagraphTag {
                icon,
                origin: [0.0, (top + bottom - side) / 2.0],
                size: side,
                label: label.unwrap_or_default().to_owned(),
                disabled: tag.status & 2 != 0,
            })
        })
        .collect();
    let right = x - PAGE_INSET + ParagraphTag::SIZE;
    let count = drawn.len() as f32;
    for (index, tag) in drawn.iter_mut().enumerate() {
        tag.origin[0] = right - side * (count - index as f32);
    }
    drawn
}

/// How much further than its list spacing OneNote 2010 sets a marker's advance from its text.
const MARKER_OFFSET: f32 = 3.9;
/// Points an equation's linear text lays out at, holding the caret without showing.
const HELD: f32 = 0.01;

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

/// A file's column at `x`, before it is placed: its icon centred over its name, which OneNote
/// shows without the extension; and the icon's height.
fn file_column(
    file: &onestore::page::Attachment,
    paragraph: ExGuid,
    format: &Format,
    x: f32,
    shape: &mut impl FnMut(
        &PageParagraph,
        Option<&Count>,
        f32,
        &[f32],
    ) -> Result<ParagraphLayout, LayoutError>,
) -> Result<(ObjectLayout, f32), LayoutError> {
    let [w, h] = file.size.unwrap_or([24.0, 24.0]);
    let left = x + (ATTACHMENT_WIDTH - w) / 2.0;
    let name = std::path::Path::new(&file.filename)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(&file.filename);
    let format = Format {
        alignment: Some(1),
        ..format.clone()
    };
    let mut label = shape(
        &caption(paragraph, file.id, name, format),
        None,
        ATTACHMENT_WIDTH,
        &[0.0, 0.0],
    )?;
    label.reset_origin(x);
    let object = ObjectLayout {
        id: file.id,
        rect: [left, 0.0, left + w, 0.0],
        kind: ObjectKind::File(label),
        bottom: 0.0,
        tags: None,
    };
    Ok((object, h))
}

/// A file on the page as OneNote draws one, relative to its position: the column a
/// paragraph's file takes, with its text in the page's default format.
pub(crate) fn page_file(
    engine: &mut TextEngine,
    file: &onestore::page::Attachment,
) -> Result<ObjectLayout, LayoutError> {
    let (mut object, height) = file_column(
        file,
        file.id,
        &Format::default(),
        0.0,
        &mut |paragraph, previous, width, indents| {
            ParagraphLayout::shape(
                engine,
                paragraph,
                previous,
                width,
                indents,
                &BTreeMap::new(),
            )
        },
    )?;
    object.place(0.0, height);
    Ok(object)
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

/// The step OneNote gives each level past the end of an indentation table.
const MISSING_INDENT: f64 = 27.0;

/// Text offset of `level` from its outline: entry `n` of `indents` steps level `n` in from
/// level `n - 1`; entry 0 moves nothing OneNote 2010 draws, text nor markers.
pub(crate) fn indentation(level: u32, indents: &[f32], width: f32) -> Result<f32, LayoutError> {
    if indents.iter().any(|v| !v.is_finite() || *v < 0.0) || level == 0 {
        return Err(LayoutError::InvalidIndentation);
    }
    let known = (level as usize).min(indents.len().saturating_sub(1));
    let indent = (indents
        .get(1..=known)
        .unwrap_or_default()
        .iter()
        .map(|v| f64::from(*v))
        .sum::<f64>()
        + f64::from(level - known as u32) * MISSING_INDENT) as f32;
    if !indent.is_finite() || indent >= width {
        return Err(LayoutError::InvalidIndentation);
    }
    Ok(indent)
}

/// A numbered paragraph's number and its NumberListFormat.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Count {
    pub(crate) number: u32,
    format: Arc<str>,
}

/// The number after a previous sibling's in the same `format`, or the first. A number in
/// another format starts again at 1 even where one in this format came before it; siblings
/// without a number leave the count alone (`evidence/toolbar-17/restart.txt`).
fn next(previous: Option<&Count>, format: &str) -> u32 {
    previous
        .filter(|count| &*count.format == format)
        .map_or(1, |count| count.number.saturating_add(1))
}

/// The number a paragraph laid out as `number` takes once it follows `previous`.
fn follows(number: Option<&(Count, bool)>, previous: Option<&Count>) -> Option<Count> {
    number.map(|(count, restart)| Count {
        number: if *restart {
            count.number
        } else {
            next(previous, &count.format)
        },
        format: count.format.clone(),
    })
}

/// What the numbered sibling after `node`, numbered `number`, continues from: siblings without
/// a number leave the count alone, as do empty numbered ones after another number
/// (`evidence/structural-edits/xml/c8-bs-1.xml`, `c8-enter-empty-1.xml`, `c2s-num-1.xml`).
fn tally(node: &PageParagraph, number: Option<Count>, previous: Option<Count>) -> Option<Count> {
    number
        .filter(|_| {
            previous.is_none() || node.text().is_some_and(|text| !text.text.text().is_empty())
        })
        .or(previous)
}

/// Removes deeper paragraphs' entries from a flow's `(level, count)` stack of the latest
/// paragraph at each level, and the previous sibling's at `level`, returning that one's count.
fn sibling(siblings: &mut Vec<(u32, Option<Count>)>, level: u32) -> Option<Count> {
    while siblings.last().is_some_and(|(deeper, _)| *deeper > level) {
        siblings.pop();
    }
    match siblings.last() {
        Some((same, _)) if *same == level => siblings.pop().unwrap().1,
        _ => None,
    }
}

/// `text` in `font` as a Unicode font shows it. Windows' Symbol font is symbol-encoded, so
/// its bytes map through the Adobe Symbol encoding (a bullet is `U+00B7` there); Wingdings'
/// map into the private use area their symbol cmaps cover, which also keeps a shaper from
/// dropping `U+00AD` as a soft hyphen.
pub fn symbol_text(font: &str, text: &str) -> String {
    /// The Adobe Symbol encoding from 0x20 and from 0xA0, after Unicode's `SYMBOL.TXT`.
    const SYMBOL: [&str; 2] = [
        " !∀#∃%&∋()∗+,−./0123456789:;<=>?≅ΑΒΧΔΕΦΓΗΙϑΚΛΜΝΟΠΘΡΣΤΥςΩΞΨΖ[∴]⊥_‾αβχδεφγηιϕκλμνοπθρστυϖωξψζ{|}∼",
        "€ϒ′≤⁄∞ƒ♣♦♥♠↔←↑→↓°±″≥×∝∂•÷≠≡≈…⏐⎯↵ℵℑℜ℘⊗⊕∅∩∪⊃⊇⊄⊂⊆∈∉∠∇®©™∏√⋅¬∧∨⇔⇐⇑⇒⇓◊〈®©™∑⎛⎜⎝⎡⎢⎣⎧⎨⎩⎪\u{f8ff}〉∫⌠⎮⌡⎞⎟⎠⎤⎥⎦⎫⎬⎭",
    ];
    if font.eq_ignore_ascii_case("Symbol") {
        text.chars()
            .map(|c| {
                let (range, from) = match u32::from(c) {
                    code @ 0x20..0x7f => (SYMBOL[0], code - 0x20),
                    code @ 0xa0..0xff => (SYMBOL[1], code - 0xa0),
                    _ => return c,
                };
                range.chars().nth(from as usize).unwrap_or(c)
            })
            .collect()
    } else if font.to_ascii_lowercase().starts_with("wingdings") {
        text.chars()
            .map(|c| match u32::from(c) {
                code @ 0x20..0x100 => char::from_u32(0xf000 + code).unwrap_or(c),
                _ => c,
            })
            .collect()
    } else {
        text.to_owned()
    }
}

/// The marker NumberListFormat `format` shows for `number`.
pub fn numbered(format: &str, number: u32) -> Result<String, LayoutError> {
    let (prefix, rest) = format
        .split_once('\u{fffd}')
        .ok_or(LayoutError::InvalidList)?;
    let mut rest = rest.chars();
    Ok(format!(
        "{prefix}{}{}",
        numeral(rest.next(), number)?,
        rest.as_str()
    ))
}

/// `number` in a list numbering sequence: 0 arabic, 1 and 2 upper and lower roman, 3 and 4
/// upper and lower letters, 5 ordinal, 6 and 7 cardinal and ordinal words, 22 arabic of two
/// digits at least, as OneNote 2010's `numberSequence` spells them in English
/// (`evidence/toolbar-17/sequences.txt`).
pub(crate) fn numeral(sequence: Option<char>, number: u32) -> Result<String, LayoutError> {
    let roman = |number: u32| {
        const DIGITS: [(u32, &str); 13] = [
            (1000, "M"),
            (900, "CM"),
            (500, "D"),
            (400, "CD"),
            (100, "C"),
            (90, "XC"),
            (50, "L"),
            (40, "XL"),
            (10, "X"),
            (9, "IX"),
            (5, "V"),
            (4, "IV"),
            (1, "I"),
        ];
        // Past 3999 the thousands run on as Ms; a million, far past what was observed,
        // would lay out a thousand of them.
        if !(1..1_000_000).contains(&number) {
            return Err(LayoutError::UnsupportedContent);
        }
        let mut rest = number;
        let mut text = String::new();
        for (value, digits) in DIGITS {
            while rest >= value {
                text.push_str(digits);
                rest -= value;
            }
        }
        Ok(text)
    };
    // Past Z a letter repeats, AA to ZZ, up to thirty times, then starts over at A.
    let letter = |number: u32| {
        let index = number
            .checked_sub(1)
            .ok_or(LayoutError::UnsupportedContent)?;
        let letter = char::from(b'A' + (index % 26) as u8);
        Ok(std::iter::repeat_n(letter, (index / 26 % 30) as usize + 1).collect::<String>())
    };
    match sequence.map(u32::from) {
        Some(0) => Ok(number.to_string()),
        Some(22) => Ok(format!("{number:02}")),
        Some(1) => roman(number),
        Some(2) => roman(number).map(|text| text.to_lowercase()),
        Some(3) => letter(number),
        Some(4) => letter(number).map(|text| text.to_lowercase()),
        Some(5) => {
            let suffix = match (number % 10, number % 100) {
                (_, 11..=13) => "th",
                (1, _) => "st",
                (2, _) => "nd",
                (3, _) => "rd",
                _ => "th",
            };
            Ok(format!("{number}{suffix}"))
        }
        Some(sequence @ (6 | 7)) => {
            let mut words = cardinal(number)?;
            if sequence == 7 {
                // The last word turns ordinal: one to first, twenty to twentieth.
                let at = words.rfind([' ', '-']).map_or(0, |at| at + 1);
                let last = &words[at..];
                let ordinal = match last {
                    "one" => "first".to_owned(),
                    "two" => "second".to_owned(),
                    "three" => "third".to_owned(),
                    "five" => "fifth".to_owned(),
                    "eight" => "eighth".to_owned(),
                    "nine" => "ninth".to_owned(),
                    "twelve" => "twelfth".to_owned(),
                    _ if last.ends_with('y') => format!("{}ieth", &last[..last.len() - 1]),
                    _ => format!("{last}th"),
                };
                words.replace_range(at.., &ordinal);
            }
            let mut chars = words.chars();
            let first = chars.next().unwrap_or_default();
            Ok(first.to_uppercase().chain(chars).collect())
        }
        _ => Err(LayoutError::UnsupportedContent),
    }
}

/// `number` in lowercase English words, "one hundred twenty-one", below a million.
fn cardinal(number: u32) -> Result<String, LayoutError> {
    const SMALL: [&str; 20] = [
        "zero",
        "one",
        "two",
        "three",
        "four",
        "five",
        "six",
        "seven",
        "eight",
        "nine",
        "ten",
        "eleven",
        "twelve",
        "thirteen",
        "fourteen",
        "fifteen",
        "sixteen",
        "seventeen",
        "eighteen",
        "nineteen",
    ];
    const TENS: [&str; 10] = [
        "", "", "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety",
    ];
    fn below_thousand(number: u32) -> String {
        let (hundreds, rest) = (number / 100, number % 100);
        let rest = match rest {
            0 => None,
            1..20 => Some(SMALL[rest as usize].to_owned()),
            _ if rest % 10 == 0 => Some(TENS[(rest / 10) as usize].to_owned()),
            _ => Some(format!(
                "{}-{}",
                TENS[(rest / 10) as usize],
                SMALL[(rest % 10) as usize]
            )),
        };
        match (hundreds, rest) {
            (0, rest) => rest.unwrap_or_default(),
            (hundreds, None) => format!("{} hundred", SMALL[hundreds as usize]),
            (hundreds, Some(rest)) => format!("{} hundred {rest}", SMALL[hundreds as usize]),
        }
    }
    match number {
        1..1000 => Ok(below_thousand(number)),
        1000..1_000_000 => {
            let (thousands, rest) = (number / 1000, number % 1000);
            let head = format!("{} thousand", below_thousand(thousands));
            Ok(if rest == 0 {
                head
            } else {
                format!("{head} {}", below_thousand(rest))
            })
        }
        _ => Err(LayoutError::UnsupportedContent),
    }
}

fn spacing(format: &Format) -> Result<[f32; 2], LayoutError> {
    let spacing = [format.space_before, format.space_after].map(|space| space.unwrap_or(0.0));
    if spacing.iter().any(|v| !v.is_finite() || *v < 0.0) {
        return Err(LayoutError::InvalidSpacing);
    }
    Ok(spacing)
}

/// Where a node with `spacing` starts after the flow `state` of a bottom and pending spacing.
fn top(state: &mut (f64, Option<f32>), [before, after]: [f32; 2]) -> f32 {
    if let Some(previous) = state.1 {
        state.0 += f64::from(previous.max(before));
    }
    state.1 = Some(after);
    state.0 as f32
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
        [
            self.origin[0] + self.text.shaped.width(),
            self.markers
                .iter()
                .map(|(layout, _)| layout.height())
                .fold(self.text.height(), f32::max),
        ]
    }

    /// Lays out a text paragraph after a sibling numbered `previous`.
    pub(crate) fn shape(
        engine: &mut TextEngine,
        paragraph: &PageParagraph,
        previous: Option<&Count>,
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
        // An Outlook task tag carries no definition but its own icon; a tag whose definition
        // is missing shows nothing, as OneNote draws no icon for it.
        let mut tag_definitions: Vec<_> = paragraph
            .tags
            .iter()
            .chain(&source.tags)
            .filter_map(|tag| {
                if tag.status & 4 != 0 {
                    return Some((tag, None, None, None, None));
                }
                match &definitions.get(tag.definition.as_ref()?)?.kind {
                    Kind::TagDefinition {
                        shape,
                        label,
                        color,
                        highlight,
                        ..
                    } => Some((tag, Some(*shape), label.as_deref(), *color, *highlight)),
                    _ => None,
                }
            })
            .collect();
        tag_definitions.reverse();
        // The newest tag that sets a colour wins.
        let color = tag_definitions.iter().rev().find_map(|tag| tag.3);
        let highlight = tag_definitions.iter().rev().find_map(|tag| tag.4);
        // Equations with objects draw in two dimensions in a space kept in their line; their
        // linear text stays in the text, too small to see, to hold the caret.
        let visible = projection.text();
        let mut equations = Vec::new();
        for zone in crate::math::built(visible) {
            let units = |byte| {
                visible
                    .utf16_offset(byte)
                    .map_err(|_| LayoutError::InvalidSourceRange)
            };
            let math = visible
                .slice(units(zone.start)?..units(zone.end)?)
                .map_err(|_| LayoutError::InvalidSourceRange)?;
            // An equation that does not parse shows its linear text.
            if let Ok(math) = crate::math::layout(engine, &math) {
                equations.push((zone, math));
            }
        }
        let spaces: Vec<_> = equations
            .iter()
            .map(|(zone, math)| InlineSpace {
                index: zone.start,
                width: math.size[0],
                ascent: math.baseline,
                descent: math.size[1] - math.baseline,
            })
            .collect();
        // URL text shows as a link, as OneNote links it on opening a page.
        let mut marks: Vec<(Range<usize>, bool)> = crate::editor::shown_urls(visible)
            .into_iter()
            .map(|url| (url, false))
            .chain(equations.iter().map(|(zone, _)| (zone.clone(), true)))
            .collect();
        marks.sort_by_key(|(range, _)| range.start);
        let mut text = if color.is_none() && highlight.is_none() && marks.is_empty() {
            engine.layout(visible, width - indent)?
        } else {
            let mut start = 0;
            let runs = visible.spans().iter().flat_map(|span| {
                let mut format = span.format.clone();
                format.color = color.or(format.color);
                format.highlight = highlight.or(format.highlight);
                let mut runs = Vec::new();
                let mut from = start;
                for (range, equation) in &marks {
                    let (a, b) = (
                        range.start.clamp(from, span.end),
                        range.end.clamp(from, span.end),
                    );
                    if a < b {
                        runs.push((visible.text()[from..a].to_owned(), format.clone()));
                        let marked = if *equation {
                            Format {
                                font_size: Some(HELD),
                                superscript: None,
                                subscript: None,
                                ..format.clone()
                            }
                        } else {
                            Format {
                                hyperlink: Some(true),
                                ..format.clone()
                            }
                        };
                        runs.push((visible.text()[a..b].to_owned(), marked));
                        from = b;
                    }
                }
                runs.push((visible.text()[from..span.end].to_owned(), format));
                start = span.end;
                runs
            });
            engine.layout_with(
                &Paragraph::from_runs(
                    runs.filter(|(text, _)| !text.is_empty())
                        .collect::<Vec<_>>(),
                ),
                width - indent,
                &spaces,
            )?
        };
        let mut markers = Vec::new();
        let mut marker_x = indent - MARKER_OFFSET;
        let mut number = None;
        for id in paragraph.lists.iter().rev() {
            let definition = definitions.get(id).ok_or(LayoutError::InvalidList)?;
            let Kind::List {
                font,
                format: Some(value),
                restart,
                ..
            } = &definition.kind
            else {
                return Err(LayoutError::InvalidList);
            };
            let mut color = definition.format.color;
            let value = if value.contains('\u{fffd}') {
                // An empty numbered paragraph shows its number as a gray placeholder.
                if source.text.text().is_empty() {
                    color = Some(PLACEHOLDER);
                }
                let current = restart.unwrap_or(next(previous, value));
                let format = value.as_str().into();
                number = Some((
                    Count {
                        number: current,
                        format,
                    },
                    restart.is_some(),
                ));
                numbered(value, current)?
            } else {
                value.clone()
            };
            if value.contains('\u{fffd}') {
                return Err(LayoutError::UnsupportedContent);
            }
            // Without a font of its own a marker takes its text's, as a number does.
            let font = font
                .clone()
                .or_else(|| definition.format.font.clone())
                .or_else(|| format.font.clone());
            let value = font
                .as_deref()
                .map_or(value.clone(), |font| symbol_text(font, &value));
            let marker = Paragraph::new(
                value,
                Format {
                    font,
                    font_size: definition.format.font_size.or(format.font_size),
                    color,
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
        let baseline = text.lines().next().unwrap().1.baseline;
        let mut tags = Vec::new();
        for tag in &tag_definitions {
            let (tag, shape, label) = (tag.0, tag.1, tag.2);
            let icon = match shape {
                None => TagIcon::Task {
                    shape: tag.shape.unwrap_or(0),
                },
                Some(shape) => match TagIcon::of(shape.unwrap_or(0), tag.status & 1 != 0) {
                    Some(icon) => icon,
                    None => continue,
                },
            };
            let size = format.font_size.unwrap_or(crate::layout::DEFAULT_FONT_SIZE);
            let side = ParagraphTag::side(size);
            tags.push(ParagraphTag {
                icon,
                origin: [0.0, ParagraphTag::top(baseline, size, side)],
                size: side,
                label: label.unwrap_or_default().to_owned(),
                disabled: tag.status & 2 != 0,
            });
        }
        let math = equations.into_iter().map(|(_, math)| math).collect();
        let mut result = Self {
            id: paragraph.id,
            origin: [indent, 0.0],
            projection,
            text,
            markers,
            number,
            tags,
            math,
            parent: paragraph.parent,
            band: paragraph.format.highlight,
        };
        result.place_tags(result.marker_left());
        Ok(result)
    }

    fn marker_left(&self) -> Option<f32> {
        self.markers.iter().map(|(_, [x, _])| *x).reduce(f32::min)
    }

    fn place_tags(&mut self, marker: Option<f32>) {
        place_tags(&mut self.tags, self.origin[0], marker);
    }
}

/// Places the tags of content at `x` among siblings whose leftmost marker starts at
/// `marker`; later tags follow the first to its right, toward the content.
fn place_tags(tags: &mut [ParagraphTag], x: f32, marker: Option<f32>) {
    for (index, tag) in tags.iter_mut().enumerate() {
        tag.origin[0] = ParagraphTag::column(x, marker, tag.size) + tag.size * index as f32;
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

impl OutlineLayout {
    /// OneNote gives an outline one tag column, as wide as its most-tagged paragraph needs; each
    /// paragraph's tags start at the column's left edge. Tag origins assume a one-tag column.
    pub(crate) fn tag_column_offset(&self) -> f32 {
        -self
            .paragraphs
            .iter()
            .map(|p| p.tags.as_slice())
            .chain(self.block_tags().map(|block| block.tags.as_slice()))
            .map(|tags| tags.iter().skip(1).map(|tag| tag.size).sum::<f32>())
            .fold(0.0, f32::max)
    }

    fn block_tags(&self) -> impl Iterator<Item = &BlockTags> {
        let tables = self.tables.iter().filter_map(|table| table.tags.as_ref());
        tables.chain(
            self.objects
                .iter()
                .filter_map(|object| object.tags.as_ref()),
        )
    }

    /// Every note tag the outline draws, with the paragraph it marks and its outline-local
    /// origin before `tag_column_offset`.
    pub fn tags(&self) -> impl Iterator<Item = (ExGuid, [f32; 2], &ParagraphTag)> {
        let text = self.paragraphs.iter().flat_map(|paragraph| {
            paragraph.tags.iter().map(move |tag| {
                let y = paragraph.origin[1] + tag.origin[1];
                (paragraph.id, [tag.origin[0], y], tag)
            })
        });
        let blocks = self.block_tags().flat_map(|block| {
            block
                .tags
                .iter()
                .map(|tag| (block.paragraph, tag.origin, tag))
        });
        text.chain(blocks)
    }

    /// Innermost table cell containing a visible paragraph index.
    pub(crate) fn paragraph_cell(&self, index: usize) -> Option<&CellLayout> {
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
            if let Some(tags) = &mut table.tags {
                tags.offset(origin);
            }
        }
        for object in &mut child.objects {
            for (value, offset) in object.rect.iter_mut().zip(origin.into_iter().cycle()) {
                *value += offset;
            }
            object.bottom += origin[1];
            if let Some(tags) = &mut object.tags {
                tags.offset(origin);
            }
            if let ObjectKind::File(label) | ObjectKind::Unsupported(label) = &mut object.kind {
                let y = label.origin[1];
                label.reset_origin(label.origin[0] + origin[0]);
                label.origin[1] = y + origin[1];
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
        shape: &mut impl FnMut(
            &PageParagraph,
            Option<&Count>,
            f32,
            &[f32],
        ) -> Result<ParagraphLayout, LayoutError>,
    ) -> Result<Self, LayoutError> {
        let mut result = Self::stack(
            nodes,
            indents,
            width,
            depth,
            edit,
            BTreeSet::new(),
            (0.0, None),
            &mut Vec::new(),
            shape,
        )?;
        result.size[0] = if fixed_width { width } else { result.size[0] }.max(result.table_width());
        result.place_tags();
        Ok(result)
    }

    /// Moves each paragraph's tags clear of the widest list marker among its siblings.
    fn place_tags(&mut self) {
        let mut markers = BTreeMap::<Option<ExGuid>, f32>::new();
        for paragraph in &self.paragraphs {
            if let Some(x) = paragraph.marker_left() {
                let left = markers.entry(paragraph.parent).or_insert(x);
                *left = left.min(x);
            }
        }
        for paragraph in &mut self.paragraphs {
            let marker = markers.get(&paragraph.parent).copied();
            paragraph.place_tags(marker);
        }
        let tables = self
            .tables
            .iter_mut()
            .filter_map(|table| table.tags.as_mut());
        let objects = self
            .objects
            .iter_mut()
            .filter_map(|object| object.tags.as_mut());
        for block in tables.chain(objects) {
            place_tags(
                &mut block.tags,
                block.x,
                markers.get(&block.parent).copied(),
            );
        }
    }

    /// Lays out `nodes` after the flow `state` and `siblings` stack, hiding the children of
    /// `hiding`; at the root, keeps a block for each node.
    #[allow(clippy::too_many_arguments)]
    fn stack<'a>(
        nodes: impl Iterator<Item = &'a PageParagraph>,
        indents: &[f32],
        width: f32,
        depth: usize,
        edit: Option<&DocumentEdit>,
        mut hiding: BTreeSet<ExGuid>,
        mut state: (f64, Option<f32>),
        siblings: &mut Vec<(u32, Option<Count>)>,
        shape: &mut impl FnMut(
            &PageParagraph,
            Option<&Count>,
            f32,
            &[f32],
        ) -> Result<ParagraphLayout, LayoutError>,
    ) -> Result<Self, LayoutError> {
        if !width.is_finite() || width <= 0.0 {
            return Err(LayoutError::InvalidWidth);
        }
        if depth > 64 {
            return Err(LayoutError::UnsupportedContent);
        }
        let mut result = Self {
            size: [36.0, 0.0],
            widths: [36.0, f32::NEG_INFINITY],
            ..Self::default()
        };
        for node in nodes {
            let hidden = node.parent.is_some_and(|parent| hiding.contains(&parent));
            if hidden || node.collapsed {
                hiding.insert(node.id);
            }
            let previous = sibling(siblings, node.level);
            let mut number = None;
            let mut block = Block {
                first: [
                    result.paragraphs.len(),
                    result.tables.len(),
                    result.objects.len(),
                ],
                metrics: None,
                extent: [f32::NEG_INFINITY; 2],
                state,
                rel: Vec::new(),
                count: None,
            };
            if !hidden {
                let (space, height, flow, extent) = match &node.content {
                    ParagraphContent::Text(_) => {
                        let mut paragraph = shape(node, previous.as_ref(), width, indents)?;
                        number = paragraph.number.clone().map(|(count, _)| count);
                        let space = spacing(&paragraph.projection.text().spans()[0].format)?;
                        paragraph.origin[1] = top(&mut state, space);
                        let size = paragraph.size();
                        block.extent[1] = paragraph.origin[0] + paragraph.text.shaped.width();
                        result.paragraphs.push(paragraph);
                        (space, size[1], size[1], size[0])
                    }
                    ParagraphContent::Table(table) => {
                        let x = indentation(node.level, indents, width)?;
                        let space = spacing(&node.format)?;
                        let y = top(&mut state, space);
                        let mut child = Self::table(table, depth + 1, edit, shape)?;
                        child.tables[0].tags =
                            BlockTags::new(node, &table.tags, 0.0, shape)?.map(|mut tags| {
                                tags.centre(0.0, child.size[1]);
                                tags
                            });
                        if depth == 0 {
                            block.rel = verticals(
                                &mut child.paragraphs,
                                &mut child.tables,
                                &mut child.objects,
                            )
                            .map(|value| *value)
                            .collect();
                        }
                        let size = child.size;
                        result.append(child, [x, y]);
                        (space, size[1], size[1], x + size[0])
                    }
                    content => {
                        let x = indentation(node.level, indents, width)?;
                        let space = spacing(&node.format)?;
                        let y = top(&mut state, space);
                        let (mut object, height, extent) = match content {
                            ParagraphContent::Image(image) => {
                                let [w, h] =
                                    image_size(image).ok_or(LayoutError::UnsupportedContent)?;
                                let object = ObjectLayout {
                                    id: image.id,
                                    rect: [x, 0.0, x + w, 0.0],
                                    kind: ObjectKind::Picture,
                                    bottom: 0.0,
                                    tags: None,
                                };
                                (object, h, x + w)
                            }
                            ParagraphContent::Attachment(file) => {
                                let (object, h) =
                                    file_column(file, node.id, &node.format, x, shape)?;
                                (object, h, x + ATTACHMENT_WIDTH)
                            }
                            ParagraphContent::Ink(ink) => {
                                // The paragraph reaches from its origin to the farthest stroke point.
                                let [w, h] = ink
                                    .bounds()
                                    .map(|[x, y, w, h]| [x + w, y + h])
                                    .filter(|size| size.iter().all(|v| v.is_finite() && *v >= 0.0))
                                    .ok_or(LayoutError::UnsupportedContent)?;
                                let object = ObjectLayout {
                                    id: ink.id,
                                    rect: [x, 0.0, x + w, 0.0],
                                    kind: ObjectKind::Ink(ink.clone()),
                                    bottom: 0.0,
                                    tags: None,
                                };
                                (object, h, x + w)
                            }
                            ParagraphContent::Unsupported(unsupported) => {
                                let w = unsupported.layout.max_width.unwrap_or(160.0).max(160.0);
                                let mut label = shape(
                                    &caption(
                                        node.id,
                                        unsupported.id,
                                        "Unsupported content",
                                        Format::default(),
                                    ),
                                    None,
                                    w - 16.0,
                                    &[0.0, 0.0],
                                )?;
                                label.reset_origin(x + 8.0);
                                let h = unsupported
                                    .layout
                                    .max_height
                                    .unwrap_or(0.0)
                                    .max(label.text.height() + 16.0);
                                let object = ObjectLayout {
                                    id: unsupported.id,
                                    rect: [x, 0.0, x + w, 0.0],
                                    kind: ObjectKind::Unsupported(label),
                                    bottom: 0.0,
                                    tags: None,
                                };
                                (object, h, x + w)
                            }
                            ParagraphContent::Text(_) | ParagraphContent::Table(_) => {
                                unreachable!("text and tables are not objects")
                            }
                        };
                        let tags = match content {
                            ParagraphContent::Image(image) => image.tags.as_slice(),
                            ParagraphContent::Attachment(file) => file.tags.as_slice(),
                            _ => &[],
                        };
                        object.tags = BlockTags::new(node, tags, x, shape)?;
                        let flow = object.place(y, height);
                        result.objects.push(object);
                        (space, height, flow, extent)
                    }
                };
                state.0 += f64::from(flow);
                result.size[0] = result.size[0].max(extent);
                result.widths[1] = result.widths[1].max(block.extent[1]);
                block.extent[0] = extent;
                block.metrics = Some([space[0], space[1], height]);
                if !(state.0 as f32).is_finite() || !result.size[0].is_finite() {
                    return Err(LayoutError::InvalidSpacing);
                }
            }
            block.count = tally(node, number, previous);
            siblings.push((node.level, block.count.clone()));
            block.state = state;
            if depth == 0 {
                result.blocks.push(block);
            }
        }
        result.size[1] = state.0 as f32;
        result.widths[0] = result.size[0];
        Ok(result)
    }

    /// Root node `root`'s number, when it is numbered text on show.
    fn number(&self, nodes: &[PageParagraph], root: usize) -> Option<&(Count, bool)> {
        let block = &self.blocks[root];
        (block.metrics.is_some() && nodes[root].text().is_some())
            .then(|| self.paragraphs[block.first[0]].number.as_ref())
            .flatten()
    }

    /// Where each root node's flow starts and ends in the outline, or `None` while a collapsed
    /// parent hides it.
    pub(crate) fn spans(&self) -> impl Iterator<Item = Option<[f32; 2]>> + '_ {
        let mut state = (0.0, None);
        self.blocks.iter().map(move |block| {
            let span = block
                .metrics
                .map(|[before, after, _]| [top(&mut state, [before, after]), block.state.0 as f32]);
            state = block.state;
            span
        })
    }

    /// The paragraphs, tables and objects of root nodes `range`.
    pub(crate) fn pieces(&self, range: Range<usize>) -> [Range<usize>; 3] {
        let first = |root: usize| {
            self.blocks.get(root).map_or(
                [self.paragraphs.len(), self.tables.len(), self.objects.len()],
                |block| block.first,
            )
        };
        let [start, end] = [first(range.start), first(range.end)];
        [0, 1, 2].map(|kind| start[kind]..end[kind])
    }

    /// Lays out root nodes `range` of `nodes` once `edit` applies, the edit's own or the table
    /// holding its cell, and works out where the nodes after them move. Resizing columns of
    /// other nodes or showing or hiding them lays out every node; renumbering lays out the
    /// renumbered.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn relayout(
        &self,
        nodes: &[PageParagraph],
        edit: &DocumentEdit,
        range: Range<usize>,
        indents: &[f32],
        width: f32,
        fixed_width: bool,
        shape: &mut impl FnMut(
            &PageParagraph,
            Option<&Count>,
            f32,
            &[f32],
        ) -> Result<ParagraphLayout, LayoutError>,
    ) -> Result<Relayout, LayoutError> {
        let resized = |node: &PageParagraph| match &node.content {
            ParagraphContent::Table(table) => edit.columns.contains_key(&table.id),
            _ => false,
        };
        let mut range = if edit.columns.len()
            == descendants(&nodes[range.clone()], None)
                .filter(|(_, _, node)| resized(node))
                .count()
        {
            range
        } else {
            0..nodes.len()
        };
        loop {
            let old = self.pieces(range.clone());
            let sources = leaves(&nodes[range.clone()], None)
                .map(|(_, _, node)| (node.id, node))
                .collect::<BTreeMap<_, _>>();
            let cached = self.paragraphs[old[0].clone()]
                .iter()
                .filter_map(|layout| Some((layout.id, (*sources.get(&layout.id)?, layout))))
                .collect::<BTreeMap<_, _>>();
            let count = match edit.container {
                None => range.len() - edit.range.len() + edit.replacement.len(),
                Some(_) => range.len(),
            };
            let replacement = edited_nodes(nodes, None, Some(edit))
                .skip(range.start)
                .take(count);
            let supplied = replacement
                .clone()
                .map(|node| node.id)
                .collect::<BTreeSet<_>>();
            // Children of a hidden or collapsed earlier node stay hidden.
            let hiding = replacement
                .clone()
                .filter_map(|node| {
                    let parent = node.parent.filter(|parent| !supplied.contains(parent))?;
                    let root = nodes[..range.start]
                        .iter()
                        .rposition(|node| node.id == parent)?;
                    (self.blocks[root].metrics.is_none() || nodes[root].collapsed).then_some(parent)
                })
                .collect();
            let start = range
                .start
                .checked_sub(1)
                .map_or((0.0, None), |root| self.blocks[root].state);
            // Each earlier level's latest paragraph, back to the nearest at the first level.
            let mut siblings = Vec::new();
            let mut level = u32::MAX;
            for root in (0..range.start).rev() {
                if nodes[root].level < level {
                    level = nodes[root].level;
                    siblings.push((level, self.blocks[root].count.clone()));
                    if level <= 1 {
                        break;
                    }
                }
            }
            siblings.reverse();
            let segment = Self::stack(
                replacement.clone(),
                indents,
                width,
                0,
                Some(edit),
                hiding,
                start,
                &mut siblings,
                &mut |node, previous, width, indents| {
                    let indent = indentation(node.level, indents, width)?;
                    if let Some((source, cached)) = cached.get(&node.id)
                        && *source == node
                        && follows(cached.number.as_ref(), previous)
                            == cached.number.clone().map(|(count, _)| count)
                        && cached.text.shaped.layout_max_advance() == width - indent
                        && cached
                            .markers
                            .iter()
                            .all(|(marker, _)| marker.shaped.layout_max_advance() == width)
                    {
                        let mut result = (*cached).clone();
                        result.reset_origin(indent);
                        return Ok(result);
                    }
                    shape(node, previous, width, indents)
                },
            )?;
            let hides =
                |block: &Block, node: &PageParagraph| block.metrics.is_none() || node.collapsed;
            if range.end < nodes.len() {
                let now = replacement
                    .zip(&segment.blocks)
                    .map(|(node, block)| (node.id, hides(block, node)))
                    .collect::<BTreeMap<_, _>>();
                if nodes[range.clone()]
                    .iter()
                    .zip(&self.blocks[range.clone()])
                    .any(|(node, block)| {
                        now.get(&node.id)
                            .is_some_and(|now| *now != hides(block, node))
                    })
                {
                    range = 0..nodes.len();
                    continue;
                }
            }
            let mut renumbered = range.end;
            for root in range.end..nodes.len() {
                let previous = sibling(&mut siblings, nodes[root].level);
                let number = self.number(nodes, root);
                let now = follows(number, previous.as_ref());
                let counted = tally(&nodes[root], now.clone(), previous);
                if now.as_ref() != number.map(|(count, _)| count)
                    || counted != self.blocks[root].count
                {
                    renumbered = root + 1;
                }
                siblings.push((nodes[root].level, counted));
                if nodes[root].level <= 1 && renumbered <= root {
                    break;
                }
            }
            if renumbered > range.end {
                range.end = renumbered;
                continue;
            }
            let mut state = segment.blocks.last().map_or(start, |block| block.state);
            let mut incoming = range
                .end
                .checked_sub(1)
                .map_or((0.0, None), |root| self.blocks[root].state);
            let mut moved = Vec::new();
            for root in range.end..self.blocks.len() {
                if state == incoming {
                    break;
                }
                let block = &self.blocks[root];
                incoming = block.state;
                let mut y = 0.0;
                if let Some([before, after, height]) = block.metrics {
                    y = top(&mut state, [before, after]);
                    let [_, tables, objects] = self.pieces(root..root + 1);
                    let flow = match self.objects[objects].first() {
                        Some(object) if tables.is_empty() => object.at(y, height).1,
                        _ => height,
                    };
                    state.0 += f64::from(flow);
                    if !(state.0 as f32).is_finite() {
                        return Err(LayoutError::InvalidSpacing);
                    }
                }
                moved.push((y, state));
            }
            let bottom = if moved.len() == self.blocks.len() - range.end {
                state.0 as f32
            } else {
                self.size[1]
            };
            let widest = |blocks: &[Block], side: usize| {
                blocks
                    .iter()
                    .map(|block| block.extent[side])
                    .fold(f32::NEG_INFINITY, f32::max)
            };
            let widths = [0, 1].map(|side| {
                let [before, after] = [
                    widest(&self.blocks[range.clone()], side),
                    widest(&segment.blocks, side),
                ];
                if after >= before || before < self.widths[side] {
                    self.widths[side].max(after)
                } else {
                    [
                        &self.blocks[..range.start],
                        &segment.blocks,
                        &self.blocks[range.end..],
                    ]
                    .into_iter()
                    .map(|blocks| widest(blocks, side))
                    .fold([36.0, f32::NEG_INFINITY][side], f32::max)
                }
            });
            let tables = &old[1];
            let tables = self.tables[..tables.start]
                .iter()
                .chain(&segment.tables)
                .chain(&self.tables[tables.end..]);
            let size = [
                if fixed_width { width } else { widths[0] }.max(table_width(tables)),
                bottom,
            ];
            return Ok(Relayout {
                range,
                segment,
                moved,
                size,
                widths,
            });
        }
    }

    /// Puts a [`Relayout`] of this layout in place.
    pub(crate) fn commit(&mut self, relayout: Relayout) {
        let Relayout {
            range,
            mut segment,
            moved,
            size,
            widths,
        } = relayout;
        let [paragraphs, tables, objects] = self.pieces(range.clone());
        let first = [paragraphs.start, tables.start, objects.start];
        let delta = [
            segment.paragraphs.len() as isize - paragraphs.len() as isize,
            segment.tables.len() as isize - tables.len() as isize,
            segment.objects.len() as isize - objects.len() as isize,
        ];
        for cell in segment.tables.iter_mut().flat_map(|table| &mut table.cells) {
            cell.paragraphs = cell.paragraphs.start + first[0]..cell.paragraphs.end + first[0];
        }
        for block in &mut segment.blocks {
            for (value, first) in block.first.iter_mut().zip(first) {
                *value += first;
            }
        }
        let after = range.start + segment.blocks.len();
        let later_tables = tables.start + segment.tables.len();
        self.paragraphs.splice(paragraphs, segment.paragraphs);
        self.tables.splice(tables, segment.tables);
        self.objects.splice(objects, segment.objects);
        self.blocks.splice(range, segment.blocks);
        if delta != [0; 3] {
            for block in &mut self.blocks[after..] {
                for (value, delta) in block.first.iter_mut().zip(delta) {
                    *value = value.wrapping_add_signed(delta);
                }
            }
            for cell in self.tables[later_tables..]
                .iter_mut()
                .flat_map(|table| &mut table.cells)
            {
                cell.paragraphs = cell.paragraphs.start.wrapping_add_signed(delta[0])
                    ..cell.paragraphs.end.wrapping_add_signed(delta[0]);
            }
        }
        for (root, (y, state)) in (after..).zip(moved) {
            let [paragraphs, tables, objects] = self.pieces(root..root + 1);
            let block = &mut self.blocks[root];
            block.state = state;
            let Some([.., height]) = block.metrics else {
                continue;
            };
            if !tables.is_empty() {
                for (value, rel) in verticals(
                    &mut self.paragraphs[paragraphs],
                    &mut self.tables[tables],
                    &mut self.objects[objects],
                )
                .zip(&block.rel)
                {
                    *value = rel + y;
                }
            } else if let Some(object) = self.objects[objects].first_mut() {
                object.place(y, height);
            } else {
                self.paragraphs[paragraphs.start].origin[1] = y;
            }
        }
        self.size = size;
        self.widths = widths;
        self.place_tags();
    }

    pub(crate) fn table_width(&self) -> f32 {
        table_width(&self.tables)
    }

    /// How far the content reaches, without the 36-point floor of `size`.
    pub(crate) fn content_width(&self) -> f32 {
        let text = self.paragraphs.iter().map(|paragraph| paragraph.size()[0]);
        let objects = self.objects.iter().map(|object| object.rect[2]);
        text.chain(objects).fold(self.table_width(), f32::max)
    }

    fn table(
        table: &Table,
        depth: usize,
        edit: Option<&DocumentEdit>,
        shape: &mut impl FnMut(
            &PageParagraph,
            Option<&Count>,
            f32,
            &[f32],
        ) -> Result<ParagraphLayout, LayoutError>,
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
        let mut result = Self {
            tables: vec![TableLayout {
                id: table.id,
                cells: Vec::new(),
                borders: table.borders.unwrap_or(true),
                tags: None,
            }],
            size: [
                (0..table.columns.len())
                    .map(|index| width(index) + 4.98)
                    .sum::<f32>()
                    - 1.83,
                3.54,
            ],
            ..Self::default()
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
                let mut child = Self::flow(
                    edited_nodes(&cell.paragraphs, Some(cell.id), edit),
                    &cell.indents,
                    width,
                    false,
                    depth,
                    edit,
                    shape,
                )?;
                height = height.max(child.size[1]);
                for paragraph in &mut child.paragraphs {
                    paragraph.parent.get_or_insert(cell.id);
                }
                let tables = child
                    .tables
                    .iter_mut()
                    .filter_map(|table| table.tags.as_mut());
                let objects = child
                    .objects
                    .iter_mut()
                    .filter_map(|object| object.tags.as_mut());
                for block in tables.chain(objects) {
                    block.parent.get_or_insert(cell.id);
                }
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
}

/// Stored column widths extend 1.77pt past the final cell border.
fn table_width<'a>(tables: impl IntoIterator<Item = &'a TableLayout>) -> f32 {
    tables
        .into_iter()
        .filter_map(|table| table.cells.last())
        .map(|cell| cell.rect[2] + 1.77)
        .fold(0.0, f32::max)
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

pub(crate) fn outline_layout(
    outline: &Outline,
    engine: &mut TextEngine,
    definitions: &BTreeMap<ExGuid, Definition>,
    width: f32,
) -> Result<OutlineLayout, LayoutError> {
    if !outline.unsupported.is_empty() {
        return Err(LayoutError::UnsupportedContent);
    }
    OutlineLayout::flow(
        outline.paragraphs.iter(),
        &outline.indents,
        width,
        outline.layout.width_set_by_user == Some(true),
        0,
        None,
        &mut |node, previous, width, indents| {
            ParagraphLayout::shape(engine, node, previous, width, indents, definitions)
        },
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
            &mut |node, previous, width, indents| {
                ParagraphLayout::shape(
                    &mut engine,
                    node,
                    previous,
                    width,
                    indents,
                    &BTreeMap::new(),
                )
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
    #[ignore = "requires CANVAS_TEST_SECTION native Tab capture"]
    fn native_table_layout() {
        use onestore::page::{Page, PageObject};
        use onestore::{RevisionIndex, Store, document::Document};
        let bytes = std::fs::read(std::env::var_os("CANVAS_TEST_SECTION").unwrap()).unwrap();
        let store = Store::parse(&bytes).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let document = Document::parse(&index).unwrap();
        let mut engine = TextEngine::default();
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
        let plain = ParagraphLayout::shape(
            &mut engine,
            &node,
            None,
            120.0,
            &[18.0, 0.0, 27.0],
            &definitions,
        )
        .unwrap();
        node.text_mut().unwrap().tags.push(Tag {
            definition: Some(id),
            status: 3,
            action_type: None,
            shape: None,
            property_status: None,
            created: None,
            completed: None,
            start: None,
            due: None,
            task_id: None,
            extra_set: 0,
        });
        for shape in [3, 13, 15, 17, 18, 23, 118, 136, 100, 101, 102, 121] {
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
            let tagged = ParagraphLayout::shape(
                &mut engine,
                &node,
                None,
                120.0,
                &[18.0, 0.0, 27.0],
                &definitions,
            )
            .unwrap();
            assert_eq!(
                tagged.tags[0].icon,
                TagIcon::Symbol {
                    shape,
                    checked: shape == 3
                }
            );
            let baseline = tagged.text.lines().next().unwrap().1.baseline;
            assert_eq!(
                tagged.tags[0].origin,
                [6.75, baseline - 0.357 * 11.0 + 0.2 - 6.0]
            );
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
        let tagged = ParagraphLayout::shape(
            &mut engine,
            &node,
            None,
            120.0,
            &[18.0, 0.0, 27.0],
            &definitions,
        )
        .unwrap();
        assert_eq!(tagged.tags[0].origin[0], tagged.markers[0].1[0] - 12.9);
        let baseline = tagged.text.lines().next().unwrap().1.baseline;
        assert_eq!(
            tagged.tags[0].origin[1],
            baseline - 0.357 * 11.0 + 0.2 - 6.0
        );
        let Kind::TagDefinition { shape, .. } = &mut definitions.get_mut(&id).unwrap().kind else {
            unreachable!()
        };
        *shape = Some(999);
        let mut shaped = |node: &PageParagraph, definitions| {
            ParagraphLayout::shape(
                &mut engine,
                node,
                None,
                120.0,
                &[18.0, 0.0, 27.0],
                definitions,
            )
            .unwrap()
        };
        // A symbol MS-ONE does not list keeps its number, an Outlook task shows its flag, and
        // a tag whose definition is missing shows nothing.
        assert_eq!(
            shaped(&node, &definitions).tags[0].icon,
            TagIcon::Symbol {
                shape: 999,
                checked: false
            }
        );
        let mut task = node.clone();
        let stored = &mut task.text_mut().unwrap().tags[0];
        stored.status |= 4;
        stored.shape = Some(89);
        assert_eq!(
            shaped(&task, &definitions).tags[0].icon,
            TagIcon::Task { shape: 89 }
        );
        let mut untagged = definitions.clone();
        untagged.remove(&id);
        assert!(shaped(&node, &untagged).tags.is_empty());
    }

    /// OneNote 2010 at 400% (Calibri bullets and numbers from 8 to 24 pt, default list
    /// spacing): a marker's advance ends 11.1 pt before its text, and a tag's 12 pt slot ends
    /// 0.9 pt before the leftmost marker among its paragraph's siblings, or 8.25 pt before its
    /// text when none has one.
    #[test]
    fn markers_and_tags_sit_where_onenote_draws_them() {
        use onestore::document::Tag;
        let id = |n| ExGuid {
            n,
            ..ExGuid::default()
        };
        let list = |format: &str| Definition {
            kind: Kind::List {
                font: None,
                format: Some(format.into()),
                bullet: None,
                restart: None,
            },
            format: Format::default(),
        };
        let definitions = BTreeMap::from([
            (id(900), list("\u{2022}")),
            (id(901), list("\u{fffd}\u{0}.")),
            (
                id(902),
                Definition {
                    kind: Kind::TagDefinition {
                        shape: Some(3),
                        label: None,
                        action_type: None,
                        color: None,
                        highlight: None,
                    },
                    format: Format::default(),
                },
            ),
        ]);
        let tag = Tag {
            definition: Some(id(902)),
            action_type: None,
            shape: None,
            property_status: None,
            status: 0,
            created: None,
            completed: None,
            start: None,
            due: None,
            task_id: None,
            extra_set: 0,
        };
        let tagged = |n, level, parent| {
            let mut node = paragraph(n, "Tag", level, parent);
            node.text_mut().unwrap().tags.push(tag.clone());
            node
        };
        let mut number = paragraph(1, "Number", 1, None);
        number.lists.push(id(901));
        let mut bullet = paragraph(5, "Bullet", 2, Some(3));
        bullet.lists.push(id(900));
        let nodes = [
            number,
            tagged(2, 1, None),
            paragraph(3, "Plain", 1, None),
            tagged(4, 2, Some(3)),
            bullet,
            paragraph(6, "Plain", 2, Some(3)),
        ];
        let mut engine = TextEngine::default();
        let layout = OutlineLayout::flow(
            nodes.iter(),
            &[18.0, 0.0, 27.0, 27.0],
            400.0,
            false,
            0,
            None,
            &mut |node, previous, width, indents| {
                ParagraphLayout::shape(&mut engine, node, previous, width, indents, &definitions)
            },
        )
        .unwrap();
        let [number, first, _, second, bullet, _] = &layout.paragraphs[..] else {
            panic!()
        };
        for paragraph in [number, bullet] {
            let (marker, [x, _]) = &paragraph.markers[0];
            let advance = marker.lines().next().unwrap().0.metrics().advance;
            assert!((x + advance - (paragraph.origin[0] - 11.1)).abs() < 1e-4);
        }
        // A numbered sibling pushes the level-1 tag left; the level-2 tag clears the bullet
        // that follows it rather than its own text.
        assert_eq!(first.tags[0].origin[0], number.markers[0].1[0] - 0.9 - 12.0);
        assert_eq!(
            second.tags[0].origin[0],
            bullet.markers[0].1[0] - 0.9 - 12.0
        );
        assert!(first.tags[0].origin[0] < first.origin[0] - 20.25);
        let alone = OutlineLayout::flow(
            [tagged(7, 1, None)].iter(),
            &[18.0, 0.0, 27.0, 27.0],
            400.0,
            false,
            0,
            None,
            &mut |node, previous, width, indents| {
                ParagraphLayout::shape(&mut engine, node, previous, width, indents, &definitions)
            },
        )
        .unwrap();
        assert_eq!(alone.paragraphs[0].tags[0].origin[0], -20.25);
    }

    /// OneNote 2010 at 400%: the first run's size picks the tag icon (9 pt below 10 pt text,
    /// 12 pt to 17.5, 18 pt to 23.5, then 24 pt up to at least 60), whose right edge stays
    /// 8.25 pt before the text and whose centre sits 0.357 of the run's size less 0.2 pt above
    /// the first baseline.
    #[test]
    fn tag_icons_follow_the_first_run() {
        use onestore::document::Tag;
        let id = |n| ExGuid {
            n,
            ..ExGuid::default()
        };
        let definitions = BTreeMap::from([(
            id(902),
            Definition {
                kind: Kind::TagDefinition {
                    shape: Some(3),
                    label: None,
                    action_type: None,
                    color: None,
                    highlight: None,
                },
                format: Format::default(),
            },
        )]);
        let tag = Tag {
            definition: Some(id(902)),
            action_type: None,
            shape: None,
            property_status: None,
            status: 0,
            created: None,
            completed: None,
            start: None,
            due: None,
            task_id: None,
            extra_set: 0,
        };
        let tagged = |sizes: &[f32], tags: usize| {
            let mut node = paragraph(1, "", 1, None);
            let text = node.text_mut().unwrap();
            text.text = Paragraph::from_runs(sizes.iter().map(|size| {
                (
                    "Tag ".to_string(),
                    Format {
                        font_size: Some(*size),
                        ..Format::default()
                    },
                )
            }));
            text.tags = vec![tag.clone(); tags];
            node
        };
        let mut engine = TextEngine::default();
        let mut lay = |nodes: &[PageParagraph]| {
            OutlineLayout::flow(
                nodes.iter(),
                &[18.0, 0.0, 27.0, 27.0],
                400.0,
                false,
                0,
                None,
                &mut |node, previous, width, indents| {
                    ParagraphLayout::shape(
                        &mut engine,
                        node,
                        previous,
                        width,
                        indents,
                        &definitions,
                    )
                },
            )
            .unwrap()
        };
        for (sizes, side) in [
            (&[9.5][..], 9.0),
            (&[10.0], 12.0),
            (&[17.5], 12.0),
            (&[18.0], 18.0),
            (&[23.5], 18.0),
            (&[24.0], 24.0),
            (&[60.0], 24.0),
            (&[11.0, 24.0], 12.0),
            (&[24.0, 11.0], 24.0),
        ] {
            let layout = lay(&[tagged(sizes, 1)]);
            let icon = &layout.paragraphs[0].tags[0];
            assert_eq!((icon.size, icon.origin[0] + icon.size), (side, -8.25));
            let baseline = layout.paragraphs[0].text.lines().next().unwrap().1.baseline;
            let centre = icon.origin[1] + side / 2.0;
            assert!((centre - (baseline - 0.357 * sizes[0] + 0.2)).abs() < 1e-4);
        }
        // A second icon extends the column left by its own side.
        let layout = lay(&[tagged(&[20.0], 2)]);
        assert_eq!(layout.tag_column_offset(), -18.0);
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
            layout: Default::default(),
            bytes: None,
            preview: None,
            recording: None,
            tags: Vec::new(),
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
                raster_operation: None,
                pressure: Vec::new(),
            }],
            groups: Vec::new(),
            shape: None,
        });
        let after = paragraph(4, "After", 1, None);
        let mut engine = TextEngine::default();
        let layout = OutlineLayout::flow(
            [&before, &file, &ink, &after].into_iter(),
            &[0.0, 0.0],
            468.0,
            false,
            0,
            None,
            &mut |node, previous, width, indents| {
                ParagraphLayout::shape(
                    &mut engine,
                    node,
                    previous,
                    width,
                    indents,
                    &BTreeMap::new(),
                )
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
                shape: None,
                property_status: None,
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
            ParagraphLayout::shape(&mut engine, &node, None, 200.0, &[0.0, 0.0], &definitions)
                .unwrap();
        assert_eq!(
            shaped
                .tags
                .iter()
                .map(|tag| (tag.icon, tag.origin[0]))
                .collect::<Vec<_>>(),
            [
                (TagIcon::of(100, false).unwrap(), -20.25),
                (TagIcon::of(15, false).unwrap(), -8.25)
            ]
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
        assert_eq!(colors, [Some(0x0080_8000)]);
        assert_eq!(shaped.projection.text().spans()[0].format.color, None);
        let outline = OutlineLayout {
            paragraphs: vec![shaped],
            ..OutlineLayout::default()
        };
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
        outline.paragraphs.truncate(1);
        for (indents, x) in [(vec![18.0, 0.0, 27.0, 27.0], 0.0), (vec![0.0], 27.0)] {
            outline.indents = indents;
            let single = outline.layout(&mut engine, &BTreeMap::new()).unwrap();
            assert_eq!(single.paragraphs[0].origin[0], x);
        }
    }

    /// Text offsets of levels 1 through 5 as OneNote 2010 draws each table (cold reads of
    /// Rust-written outlines, measured to the pixel at 96 dpi).
    #[test]
    fn indentation_follows_onenote_for_short_and_unusual_tables() {
        for (indents, expected) in [
            (&[18.0, 0.0, 27.0, 27.0][..], [0.0, 27.0, 54.0, 81.0, 108.0]),
            (&[18.0, 0.0], [0.0, 27.0, 54.0, 81.0, 108.0]),
            (&[0.0, 0.0], [0.0, 27.0, 54.0, 81.0, 108.0]),
            (&[0.0], [27.0, 54.0, 81.0, 108.0, 135.0]),
            (&[18.0], [27.0, 54.0, 81.0, 108.0, 135.0]),
            (&[], [27.0, 54.0, 81.0, 108.0, 135.0]),
            (&[0.0, 5.0], [5.0, 32.0, 59.0, 86.0, 113.0]),
            (&[5.0, 10.0, 20.0, 40.0], [10.0, 30.0, 70.0, 97.0, 124.0]),
            (
                &[40.0, 0.0, 10.0, 20.0, 30.0, 50.0],
                [0.0, 10.0, 30.0, 60.0, 110.0],
            ),
        ] {
            for (level, x) in (1..).zip(expected) {
                assert_eq!(
                    indentation(level, indents, 500.0).unwrap(),
                    x,
                    "{indents:?}"
                );
            }
        }
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

    #[test]
    fn symbol_fonts_show_what_windows_draws() {
        // OneNote's bullet library stores these (`BULLET_LIBRARY`).
        assert_eq!(symbol_text("Symbol", "\u{b7}"), "\u{2022}");
        assert_eq!(symbol_text("Symbol", "\u{de}"), "\u{21d2}");
        assert_eq!(symbol_text("Symbol", "ap\u{f0}"), "\u{3b1}\u{3c0}\u{f8ff}");
        assert_eq!(symbol_text("Wingdings", "l\u{ad}"), "\u{f06c}\u{f0ad}");
        assert_eq!(symbol_text("Wingdings 2", "\u{9d}"), "\u{f09d}");
        assert_eq!(symbol_text("Calibri", "\u{b7}"), "\u{b7}");
    }

    #[test]
    fn numerals_follow_onenote_number_sequences() {
        assert_eq!(numeral(Some('\u{0}'), 12).unwrap(), "12");
        assert_eq!(numeral(Some('\u{1}'), 1994).unwrap(), "MCMXCIV");
        assert_eq!(numeral(Some('\u{2}'), 4).unwrap(), "iv");
        assert_eq!(numeral(Some('\u{3}'), 3).unwrap(), "C");
        assert_eq!(numeral(Some('\u{4}'), 26).unwrap(), "z");
        assert!(numeral(Some('\u{1}'), 0).is_err());
        // As OneNote 2010 renders them through COM: letters to 800, Roman numerals to 4010.
        assert_eq!(numeral(Some('\u{4}'), 27).unwrap(), "aa");
        assert_eq!(numeral(Some('\u{3}'), 105).unwrap(), "AAAAA");
        assert_eq!(numeral(Some('\u{3}'), 780).unwrap(), "Z".repeat(30));
        assert_eq!(numeral(Some('\u{3}'), 781).unwrap(), "A");
        assert_eq!(numeral(Some('\u{3}'), 800).unwrap(), "T");
        assert_eq!(numeral(Some('\u{1}'), 3999).unwrap(), "MMMCMXCIX");
        assert_eq!(numeral(Some('\u{1}'), 4009).unwrap(), "MMMMIX");
        assert_eq!(numeral(Some('\u{2}'), 4000).unwrap(), "mmmm");
        assert_eq!(numeral(Some('\u{16}'), 7).unwrap(), "07");
        assert_eq!(numeral(Some('\u{16}'), 106).unwrap(), "106");
        assert!(numeral(Some('\u{9}'), 1).is_err());
        // As OneNote 2010 renders them through COM, 1 to 130.
        let words = |sequence, numbers: &[u32]| {
            numbers
                .iter()
                .map(|number| numeral(Some(sequence), *number).unwrap())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            words(
                '\u{5}',
                &[1, 2, 3, 4, 11, 12, 13, 21, 22, 23, 101, 111, 112, 121]
            ),
            [
                "1st", "2nd", "3rd", "4th", "11th", "12th", "13th", "21st", "22nd", "23rd",
                "101st", "111th", "112th", "121st"
            ]
        );
        assert_eq!(
            words('\u{6}', &[1, 12, 20, 21, 99, 100, 101, 130]),
            [
                "One",
                "Twelve",
                "Twenty",
                "Twenty-one",
                "Ninety-nine",
                "One hundred",
                "One hundred one",
                "One hundred thirty"
            ]
        );
        assert_eq!(
            words(
                '\u{7}',
                &[1, 2, 3, 5, 8, 9, 12, 20, 21, 40, 100, 101, 112, 130]
            ),
            [
                "First",
                "Second",
                "Third",
                "Fifth",
                "Eighth",
                "Ninth",
                "Twelfth",
                "Twentieth",
                "Twenty-first",
                "Fortieth",
                "One hundredth",
                "One hundred first",
                "One hundred twelfth",
                "One hundred thirtieth"
            ]
        );
    }
}
