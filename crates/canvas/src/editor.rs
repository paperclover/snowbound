use crate::{
    date::PageDate,
    document::{DocumentEdit, TextDocument, TextPosition, descendants, leaves},
    layout::{LayoutError, TextEngine},
    outline::{Arrange, OutlineLayout, ParagraphLayout, outline_layout, visible_paragraphs},
};
use draw::edit::{self, Movement, SelectionUnit};
use onestore::ExGuid;
use onestore::document::{Format, Kind};
use onestore::op::PageOp;
use onestore::page::text::{EditError, Paragraph};
use onestore::page::{
    Definition, Outline, Page, PageObject, PageParagraph, ParagraphContent, Title,
};
use parley::{
    Affinity, BoundingBox,
    editing::{Cursor, Selection as ParagraphSelection},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    ops::Range,
};

pub const DEFAULT_OUTLINE_WIDTH: f32 = 468.0;

#[cfg(test)]
mod evidence;
mod format;
mod ops;
pub(crate) mod page;
mod table;
pub use format::{Alignment, FormatState, Formatting, NoteTag, Toggle};
pub use page::ReadOnlyObject;

#[derive(Debug)]
pub enum EditorError {
    Edit(EditError),
    Layout(LayoutError),
    InvalidGeometry,
}

impl fmt::Display for EditorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Edit(error) => error.fmt(f),
            Self::Layout(error) => error.fmt(f),
            Self::InvalidGeometry => {
                f.write_str("The page contains invalid object dimensions or positions.")
            }
        }
    }
}

impl std::error::Error for EditorError {}

impl From<EditError> for EditorError {
    fn from(error: EditError) -> Self {
        Self::Edit(error)
    }
}

impl From<LayoutError> for EditorError {
    fn from(error: LayoutError) -> Self {
        Self::Layout(error)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Selection {
    pub positions: [TextPosition; 2],
    pub affinities: [Affinity; 2],
}

impl From<[TextPosition; 2]> for Selection {
    fn from(positions: [TextPosition; 2]) -> Self {
        Self {
            positions,
            affinities: [Affinity::Downstream; 2],
        }
    }
}

pub struct CanvasEditor {
    pub(crate) objects: Vec<page::Content>,
    date: Option<PageDate>,
    outlines: Vec<TextOutline>,
    definitions: BTreeMap<ExGuid, Definition>,
    header: PageHeader,
    active: Focus,
    undo: Vec<History>,
    redo: Vec<History>,
    composition: Option<Composition>,
    preferred_x: Option<f32>,
    /// Formatting chosen at a caret for the text typed there next, until an edit.
    pending: Option<(ExGuid, TextPosition, onestore::document::Format)>,
    /// What `take_ops` hands over next.
    ops: Result<Vec<PageOp>, onestore::Error>,
    /// The page as stored when the editor last read it and the ops `take_ops` handed out
    /// since, which `refresh` compares a changed stored page with; none once the editor
    /// holds an edit that could not be stored.
    stored: Option<(Page, Vec<PageOp>)>,
}

/// Imported page state the editable content does not carry.
#[derive(Default)]
struct PageHeader {
    title: String,
    identity: Option<[u8; 16]>,
    created: Option<u64>,
    margin_origin: [f32; 2],
    areas: Vec<page::TitleArea>,
}

enum Focus {
    Outline(usize),
    Draft {
        index: usize,
        outline: Box<TextOutline>,
    },
    Caret {
        outline: Box<TextOutline>,
        index: usize,
    },
}

#[derive(Clone)]
pub struct TextOutline {
    pub id: onestore::ExGuid,
    pub title: bool,
    min_width: Option<f32>,
    layout: onestore::document::Layout,
    document: TextDocument,
    indents: Vec<f32>,
    shaped: OutlineLayout,
    selection: Selection,
}

impl TextOutline {
    /// A single plain paragraph containing only ASCII spaces or no text.
    pub fn is_empty(&self) -> bool {
        let [node] = self.document.nodes() else {
            return false;
        };
        node.lists.is_empty()
            && node.tags.is_empty()
            && !node.collapsed
            && node.text().is_some_and(|text| {
                text.tags.is_empty() && text.text.text().bytes().all(|byte| byte == b' ')
            })
    }

    /// Tests an outline-local point against the provisional paragraph band.
    pub fn contains_extension(&self, point: [f32; 2]) -> bool {
        !self.title
            && (0.0..=self.shaped.size[0]).contains(&point[0])
            && point[1] > self.shaped.size[1]
            && point[1] <= self.shaped.size[1] + 27.0
    }

    pub fn new(
        engine: &mut TextEngine,
        document: TextDocument,
        width: f32,
        position: [f32; 2],
    ) -> Result<Self, EditorError> {
        if !position.iter().all(|value| value.is_finite()) {
            return Err(EditError::InvalidRange.into());
        }
        let indents = vec![18.0, 0.0, 27.0, 27.0];
        let fixed_width = document.nodes().len() != 1
            || document.nodes()[0]
                .text()
                .is_none_or(|text| !text.text.text().is_empty());
        let shaped = OutlineLayout::flow(
            document.nodes().iter(),
            &indents,
            width,
            fixed_width,
            0,
            None,
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
        Ok(Self {
            id: onestore::page::text::new_id()?,
            title: false,
            min_width: None,
            layout: onestore::document::Layout {
                x: Some(position[0]),
                y: Some(position[1]),
                max_width: Some(width),
                width_set_by_user: Some(fixed_width),
                ..Default::default()
            },
            document,
            indents,
            shaped,
            selection: [TextPosition {
                paragraph: 0,
                offset: 0,
            }; 2]
                .into(),
        })
    }

    pub fn from_outline(
        engine: &mut TextEngine,
        outline: &Outline,
        definitions: &BTreeMap<ExGuid, Definition>,
    ) -> Result<Self, EditorError> {
        if [outline.layout.x, outline.layout.y]
            .into_iter()
            .flatten()
            .any(|value| !value.is_finite())
        {
            return Err(EditError::InvalidRange.into());
        }
        if outline
            .min_width
            .is_some_and(|width| !width.is_finite() || width <= 0.0)
        {
            return Err(LayoutError::InvalidWidth.into());
        }
        let shaped = if outline.title {
            if outline
                .layout
                .max_height
                .is_some_and(|height| !height.is_finite() || height < 0.0)
            {
                return Err(LayoutError::InvalidSpacing.into());
            }
            outline_layout(
                outline,
                engine,
                definitions,
                outline
                    .layout
                    .reserved_width
                    .or(outline.layout.max_width)
                    .unwrap_or(crate::outline::TITLE_WIDTH),
            )?
        } else {
            outline.layout(engine, definitions)?
        };
        let document = TextDocument::from_nodes(outline.paragraphs.clone())?;
        // The caret needs a paragraph of text to stand in.
        if document.text_nodes().next().is_none() {
            return Err(EditError::UnsupportedContent.into());
        }
        Ok(Self {
            id: outline.id,
            title: outline.title,
            min_width: outline.min_width,
            layout: outline.layout.clone(),
            indents: outline.indents.clone(),
            document,
            shaped,
            selection: [TextPosition {
                paragraph: 0,
                offset: 0,
            }; 2]
                .into(),
        })
    }

    pub fn snapshot(&self) -> Outline {
        Outline {
            id: self.id,
            title: self.title,
            min_width: self.min_width,
            layout: self.layout.clone(),
            indents: self.indents.clone(),
            paragraphs: self.document.nodes().to_vec(),
            unsupported: Vec::new(),
        }
    }

    pub fn document(&self) -> &TextDocument {
        &self.document
    }

    pub fn layout(&self) -> &onestore::document::Layout {
        &self.layout
    }

    pub fn shaped(&self) -> &OutlineLayout {
        &self.shaped
    }

    pub fn origin(&self) -> [f32; 2] {
        [self.layout.x.unwrap_or(0.0), self.layout.y.unwrap_or(0.0)]
    }

    pub fn wrap_width(&self) -> f32 {
        self.layout
            .reserved_width
            .or(self.layout.max_width)
            .unwrap_or(crate::outline::TITLE_WIDTH)
    }

    /// Native typing room expands the interactive bounds without changing the wrap constraint.
    pub fn bounds(&self) -> BoundingBox {
        let [x, y] = self.origin().map(f64::from);
        let width = if self.layout.width_set_by_user == Some(true) {
            self.shaped.size[0]
        } else {
            let text_width = if self.shaped.tables.is_empty() {
                Some(self.shaped.size[0])
            } else {
                Some(self.shaped.widths[1]).filter(|width| *width > f32::NEG_INFINITY)
            };
            text_width.map_or(self.shaped.size[0], |width| {
                (width + 36.0)
                    .max(72.0)
                    .min(self.wrap_width())
                    .max(self.shaped.size[0])
            })
        }
        .max(self.min_width.unwrap_or(0.0));
        BoundingBox {
            x0: x,
            y0: y,
            x1: x + f64::from(width),
            y1: y + f64::from(if self.title {
                self.shaped.size[1].max(self.layout.max_height.unwrap_or(0.0))
            } else {
                self.shaped.size[1]
            }),
        }
    }

    pub fn layouts(&self) -> impl Iterator<Item = (usize, &ParagraphLayout)> {
        let mut shaped = self.shaped.paragraphs.iter().peekable();
        self.document
            .text_nodes()
            .enumerate()
            .filter_map(move |(index, node)| {
                if shaped
                    .peek()
                    .is_some_and(|paragraph| paragraph.id == node.id)
                {
                    Some((index, shaped.next().unwrap()))
                } else {
                    None
                }
            })
    }

    /// Layouts follow the text leaves minus hidden ones, so an index shared by a leaf and its
    /// layout needs no search.
    fn visible_index(&self, source: usize) -> Result<usize, EditError> {
        let id = self
            .document
            .leaf(source)
            .ok_or(EditError::InvalidRange)?
            .2
            .id;
        if self
            .shaped
            .paragraphs
            .get(source)
            .is_some_and(|paragraph| paragraph.id == id)
        {
            return Ok(source);
        }
        self.shaped
            .paragraphs
            .iter()
            .position(|paragraph| paragraph.id == id)
            .ok_or(EditError::InvalidRange)
    }

    fn source_index(&self, visible: usize) -> usize {
        let id = self.shaped.paragraphs[visible].id;
        if self
            .document
            .leaf(visible)
            .is_some_and(|(_, _, node)| node.id == id)
        {
            return visible;
        }
        self.document
            .text_nodes()
            .position(|paragraph| paragraph.id == id)
            .unwrap()
    }

    pub fn paragraph_layout(&self, source: usize) -> Result<&ParagraphLayout, EditError> {
        Ok(&self.shaped.paragraphs[self.visible_index(source)?])
    }

    fn column(&self, x: f32, current: ExGuid) -> Result<Vec<usize>, EditError> {
        let paragraphs = self
            .shaped
            .paragraphs
            .iter()
            .enumerate()
            .map(|(index, paragraph)| (paragraph.id, index))
            .collect::<BTreeMap<_, _>>();
        let tables = self
            .shaped
            .tables
            .iter()
            .map(|table| (table.id, table))
            .collect::<BTreeMap<_, _>>();
        let mut result = Vec::new();
        let mut pending = vec![visible_paragraphs(self.document.nodes().iter())];
        while let Some(nodes) = pending.last_mut() {
            let Some(node) = nodes.next() else {
                pending.pop();
                continue;
            };
            match &node.content {
                onestore::page::ParagraphContent::Text(_) => {
                    result.push(
                        *paragraphs
                            .get(&node.id)
                            .ok_or(EditError::InvalidStructure)?,
                    );
                }
                onestore::page::ParagraphContent::Table(table) => {
                    let layout = tables.get(&table.id).ok_or(EditError::InvalidStructure)?;
                    let cells = layout
                        .cells
                        .get(..table.columns.len())
                        .ok_or(EditError::InvalidStructure)?;
                    let (first, last) = cells
                        .first()
                        .zip(cells.last())
                        .ok_or(EditError::InvalidStructure)?;
                    let x = x.clamp(first.rect[0], last.rect[2]);
                    let column = cells
                        .iter()
                        .enumerate()
                        .min_by(|(_, a), (_, b)| {
                            let distance = |cell: &crate::outline::CellLayout| {
                                (f64::from(x) - f64::from(x.clamp(cell.rect[0], cell.rect[2])))
                                    .abs()
                            };
                            distance(a).total_cmp(&distance(b))
                        })
                        .ok_or(EditError::InvalidStructure)?
                        .0;
                    for row in table.rows.iter().rev() {
                        let cell = row
                            .cells
                            .iter()
                            .find(|cell| {
                                leaves(&cell.paragraphs, None)
                                    .any(|(_, _, node)| node.id == current)
                            })
                            .unwrap_or(&row.cells[column]);
                        pending.push(visible_paragraphs(cell.paragraphs.iter()));
                    }
                }
                onestore::page::ParagraphContent::Image(_)
                | onestore::page::ParagraphContent::Attachment(_)
                | onestore::page::ParagraphContent::Ink(_)
                | onestore::page::ParagraphContent::Unsupported(_) => {}
            }
        }
        Ok(result)
    }

    fn paragraph_at(&self, x: f32, y: f32) -> Result<usize, EditError> {
        if self.shaped.tables.is_empty() {
            return Ok(self
                .shaped
                .paragraphs
                .partition_point(|paragraph| paragraph.origin[1] <= y)
                .saturating_sub(1));
        }
        let paragraphs = self
            .shaped
            .paragraphs
            .iter()
            .enumerate()
            .map(|(index, paragraph)| (paragraph.id, (index, paragraph)))
            .collect::<BTreeMap<_, _>>();
        let tables = self
            .shaped
            .tables
            .iter()
            .map(|table| (table.id, table))
            .collect::<BTreeMap<_, _>>();
        let mut nodes = self.document.nodes();
        loop {
            let mut target = None;
            for node in visible_paragraphs(nodes.iter()) {
                let top = match &node.content {
                    onestore::page::ParagraphContent::Text(_) => {
                        paragraphs
                            .get(&node.id)
                            .ok_or(EditError::InvalidStructure)?
                            .1
                            .origin[1]
                    }
                    onestore::page::ParagraphContent::Table(table) => {
                        tables
                            .get(&table.id)
                            .and_then(|layout| layout.cells.first())
                            .ok_or(EditError::InvalidStructure)?
                            .rect[1]
                            - 1.86
                    }
                    // Objects hold no caret; text around them takes the hit.
                    onestore::page::ParagraphContent::Image(_)
                    | onestore::page::ParagraphContent::Attachment(_)
                    | onestore::page::ParagraphContent::Ink(_)
                    | onestore::page::ParagraphContent::Unsupported(_) => continue,
                };
                if target.is_none() || top <= y {
                    target = Some(node);
                }
                if top > y {
                    break;
                }
            }
            let target = target.ok_or(EditError::InvalidStructure)?;
            let onestore::page::ParagraphContent::Table(table) = &target.content else {
                return paragraphs
                    .get(&target.id)
                    .map(|(index, _)| *index)
                    .ok_or(EditError::InvalidStructure);
            };
            let layout = tables.get(&table.id).ok_or(EditError::InvalidStructure)?;
            let (first, last) = layout
                .cells
                .first()
                .zip(layout.cells.last())
                .ok_or(EditError::InvalidStructure)?;
            let x = x.clamp(first.rect[0], last.rect[2]);
            let y = y.clamp(first.rect[1], last.rect[3]);
            let cell = layout
                .cells
                .iter()
                .map(|cell| {
                    let dx = f64::from(x) - f64::from(x.clamp(cell.rect[0], cell.rect[2]));
                    let dy = f64::from(y) - f64::from(y.clamp(cell.rect[1], cell.rect[3]));
                    (cell, dy.abs(), dx.abs())
                })
                .min_by(|a, b| a.1.total_cmp(&b.1).then(a.2.total_cmp(&b.2)))
                .ok_or(EditError::InvalidStructure)?
                .0;
            nodes = self.document.container(Some(cell.id))?;
        }
    }
}

struct Composition {
    original: TextChange,
    range: Range<TextPosition>,
}

impl ParagraphLayout {
    fn cursor(&self, source: u32, affinity: Affinity) -> Result<Cursor, EditError> {
        let visible = self.projection.visible_offset(source)?;
        let byte = self.projection.text().byte_offset(visible)?;
        Ok(self.text.cursor(byte, affinity))
    }
}

#[derive(Clone)]
struct TextChange {
    edit: DocumentEdit,
    selection: Selection,
    positions: Vec<Placement>,
}

#[derive(Clone)]
struct Placement {
    id: ExGuid,
    position: [Option<f32>; 2],
}

enum RestoreFocus {
    Outline(ExGuid),
    Caret {
        source: Box<Outline>,
        selection: Selection,
        index: usize,
    },
}

enum History {
    Date(Box<PageDate>),
    Draft {
        outlines: Box<[TextOutline; 2]>,
        index: usize,
        caret: Box<Outline>,
        selection: Selection,
        restore_caret: bool,
    },
    Text {
        outline: onestore::ExGuid,
        change: Box<TextChange>,
    },
    Position {
        outline: onestore::ExGuid,
        position: [Option<f32>; 2],
    },
    Layout {
        outline: ExGuid,
        layout: onestore::document::Layout,
    },
    Image {
        image: ExGuid,
        layout: onestore::document::Layout,
    },
    /// Restores the picture at `index` in paint order, or removes the one there when `None`.
    Picture {
        index: usize,
        image: Option<Box<onestore::page::Image>>,
    },
    Remove {
        outline: onestore::ExGuid,
        focus: RestoreFocus,
    },
    Insert {
        index: usize,
        outline: Box<TextOutline>,
        focus: RestoreFocus,
    },
    Restore {
        index: usize,
        source: Box<Outline>,
        selection: Selection,
        focus: RestoreFocus,
    },
}

impl CanvasEditor {
    pub fn new(
        engine: &mut TextEngine,
        document: TextDocument,
        width: f32,
    ) -> Result<Self, EditorError> {
        let outline = TextOutline::new(engine, document, width, [0.0; 2])?;
        let (outlines, active) = if outline.is_empty() {
            (
                Vec::new(),
                Focus::Caret {
                    outline: Box::new(outline),
                    index: 0,
                },
            )
        } else {
            (vec![outline], Focus::Outline(0))
        };
        Ok(Self {
            outlines,
            definitions: BTreeMap::new(),
            objects: Vec::new(),
            date: None,
            header: PageHeader::default(),
            active,
            undo: Vec::new(),
            redo: Vec::new(),
            composition: None,
            preferred_x: None,
            pending: None,
            ops: Ok(Vec::new()),
            stored: None,
        })
    }

    pub fn from_page(mut page: Page, engine: &mut TextEngine) -> Result<Self, EditorError> {
        let stored = page.clone();
        let page::Import {
            objects,
            mut outlines,
            date,
            areas,
        } = page::build(&mut page, engine, true)?;
        let needs_caret = outlines.is_empty();
        if needs_caret {
            let x = objects
                .iter()
                .filter_map(|object| match object {
                    page::Content::ReadOnly(object) => Some(object.rect()[2]),
                    page::Content::Ink(ink) => page::ink_bounds(ink).map(|b| b[2]),
                    _ => None,
                })
                .fold(0.0_f32, f32::max);
            outlines.push(TextOutline::new(
                engine,
                TextDocument::new(vec![Paragraph::new(String::new(), Default::default())])?,
                240.0,
                [if x > 0.0 { x + 24.0 } else { 0.0 }, 0.0],
            )?);
        }
        let mut editor = Self::from_text_outlines(outlines, page.definitions, date)?;
        if needs_caret {
            editor.active = Focus::Caret {
                outline: Box::new(editor.outlines.pop().unwrap()),
                index: 0,
            };
        }
        editor.objects = objects;
        editor.header = PageHeader {
            title: page.title,
            identity: page.identity,
            created: page.created,
            margin_origin: page.margin_origin,
            areas,
        };
        let mut ids = BTreeSet::new();
        if !editor.object_layouts().all(|(id, _)| ids.insert(id)) {
            return Err(EditError::InvalidStructure.into());
        }
        editor.stored = Some((stored, Vec::new()));
        Ok(editor)
    }

    /// Shows `page`, the stored page after a change made elsewhere. What the change did not
    /// reach stays as it is, history included: the page as stored before it (as last read,
    /// with the ops handed out since) is compared with `page`; outlines the change reached
    /// show anew with the caret and selection kept by paragraph identity, and history entries
    /// editing them are dropped. False when the change reached nothing shown. Marked text
    /// must be committed or cancelled first.
    pub fn refresh(&mut self, page: Page, engine: &mut TextEngine) -> Result<bool, EditorError> {
        let known = self.stored.take().and_then(|(mut stored, sent)| {
            sent.iter()
                .try_for_each(|op| onestore::op::predict(&mut stored, op))
                .ok()
                .map(|()| stored)
        });
        // Without that, the editor's page stands in, which storage may normalize apart.
        let known = match known {
            Some(known) => known,
            None => self.page()?,
        };
        self.stored = Some((page.clone(), Vec::new()));
        if known == page {
            return Ok(false);
        }
        fn outlines(page: &Page) -> BTreeMap<ExGuid, &Outline> {
            page.objects
                .iter()
                .flat_map(|object| match object {
                    PageObject::Outline(outline) => std::slice::from_ref(outline),
                    PageObject::Title(title) => title.outlines.as_slice(),
                    _ => &[],
                })
                .map(|outline| (outline.id, outline))
                .collect()
        }
        let (before, after) = (outlines(&known), outlines(&page));
        let changed: BTreeSet<ExGuid> = before
            .keys()
            .chain(after.keys())
            .filter(|id| before.get(id) != after.get(id))
            .copied()
            .collect();
        // Everything but outline content: the page's objects, geometry and date.
        let frame = |page: &Page| {
            let hollow = |outline: &Outline| Outline {
                id: outline.id,
                title: false,
                min_width: None,
                layout: Default::default(),
                indents: Vec::new(),
                paragraphs: Vec::new(),
                unsupported: Vec::new(),
            };
            let objects: Vec<PageObject> = page
                .objects
                .iter()
                .map(|object| match object {
                    PageObject::Outline(outline) => PageObject::Outline(hollow(outline)),
                    PageObject::Title(title) => PageObject::Title(Title {
                        id: title.id,
                        date: title.date,
                        layout: title.layout.clone(),
                        outlines: title.outlines.iter().map(hollow).collect(),
                    }),
                    object => object.clone(),
                })
                .collect();
            (page.created, page.margin_origin, objects)
        };
        let objects_changed = frame(&known) != frame(&page);
        let date_changed = known.created != page.created
            || self
                .date
                .as_ref()
                .is_some_and(|date| changed.contains(&date.source().id));
        let mut fresh = Self::from_page(page, engine)?;
        let shown = self.active_outline();
        let (id, selection) = (shown.id, shown.selection);
        let reached = changed.contains(&id);
        let document = reached.then(|| shown.document.clone());
        let mut own: BTreeMap<ExGuid, TextOutline> = std::mem::take(&mut self.outlines)
            .into_iter()
            .map(|outline| (outline.id, outline))
            .collect();
        for outline in &mut fresh.outlines {
            if !changed.contains(&outline.id)
                && let Some(kept) = own.remove(&outline.id)
            {
                *outline = kept;
            }
        }
        let position = fresh.outlines.iter().position(|outline| outline.id == id);
        match (
            std::mem::replace(&mut self.active, Focus::Outline(0)),
            position,
        ) {
            (Focus::Caret { outline, index }, _) => {
                fresh.active = Focus::Caret {
                    outline,
                    index: index.min(fresh.outlines.len()),
                };
            }
            (Focus::Draft { outline, .. }, Some(index)) if !reached => {
                fresh.active = Focus::Draft { index, outline };
            }
            (_, Some(index)) if !reached => fresh.active = Focus::Outline(index),
            (_, Some(index)) => {
                fresh.active = Focus::Outline(index);
                let document = document.expect("a reached outline's document was kept");
                let mapped = Selection {
                    positions: selection.positions.map(|position| {
                        follow(&document, &fresh.outlines[index].document, position)
                    }),
                    affinities: selection.affinities,
                };
                let _ = fresh.select(mapped);
                self.pending = self.pending.take().filter(|(_, at, _)| {
                    follow(&document, &fresh.active_outline().document, *at) == *at
                });
            }
            (_, None) => self.pending = None,
        }
        if !date_changed {
            fresh.date = self.date.take();
        }
        if !objects_changed {
            fresh.objects = std::mem::take(&mut self.objects);
        }
        for (id, definition) in std::mem::take(&mut self.definitions) {
            fresh.definitions.entry(id).or_insert(definition);
        }
        // Undoing an outline's creation focuses the outline focused before, if it is there.
        let gone = |focus: &RestoreFocus| matches!(focus, RestoreFocus::Outline(id) if !fresh.outlines.iter().any(|o| o.id == *id));
        let reaches = |history: &History| match history {
            History::Date(_) => date_changed,
            History::Image { .. } | History::Picture { .. } => objects_changed,
            History::Draft { outlines, .. } => changed.contains(&outlines[0].id),
            History::Text { outline, change } => {
                changed.contains(outline)
                    || change
                        .positions
                        .iter()
                        .any(|placement| objects_changed || changed.contains(&placement.id))
            }
            History::Position { outline, .. } | History::Layout { outline, .. } => {
                changed.contains(outline)
            }
            History::Remove { outline, focus } => changed.contains(outline) || gone(focus),
            History::Insert { outline, focus, .. } => changed.contains(&outline.id) || gone(focus),
            History::Restore { source, focus, .. } => changed.contains(&source.id) || gone(focus),
        };
        fresh.undo = std::mem::take(&mut self.undo)
            .into_iter()
            .filter(|history| !reaches(history))
            .collect();
        fresh.redo = std::mem::take(&mut self.redo)
            .into_iter()
            .filter(|history| !reaches(history))
            .collect();
        fresh.pending = self.pending.take();
        fresh.preferred_x = self.preferred_x;
        fresh.ops = std::mem::replace(&mut self.ops, Ok(Vec::new()));
        fresh.stored = self.stored.take();
        *self = fresh;
        Ok(true)
    }

    /// Rebuilds the stored page, restoring the title areas and read-only objects import split up.
    pub fn page(&self) -> Result<Page, EditorError> {
        let mut objects: Vec<PageObject> = Vec::new();
        for content in &self.objects {
            let mut outline = match content {
                page::Content::Editable(id) => {
                    match self.outlines.iter().find(|outline| outline.id == *id) {
                        Some(outline) => outline.snapshot(),
                        None => continue,
                    }
                }
                page::Content::Outline { source, .. } => source.clone(),
                page::Content::Date { .. } => self
                    .date
                    .as_ref()
                    .ok_or(EditError::InvalidStructure)?
                    .source()
                    .clone(),
                page::Content::Image(image) => {
                    objects.push(PageObject::Image(image.clone()));
                    continue;
                }
                page::Content::Ink(ink) => {
                    objects.push(PageObject::Ink(ink.clone()));
                    continue;
                }
                page::Content::ReadOnly(object) => {
                    objects.push(object.source.clone());
                    continue;
                }
            };
            let area = self
                .header
                .areas
                .iter()
                .find_map(|area| Some((area, *area.origins.get(&outline.id)?)));
            let Some((area, origin)) = area else {
                objects.push(PageObject::Outline(outline));
                continue;
            };
            [outline.layout.x, outline.layout.y] = origin;
            match objects.last_mut() {
                Some(PageObject::Title(title)) if title.id == area.id => {
                    title.outlines.push(outline)
                }
                _ => objects.push(PageObject::Title(Title {
                    id: area.id,
                    date: area.date,
                    layout: area.layout.clone(),
                    outlines: vec![outline],
                })),
            }
        }
        objects.extend(
            self.outlines
                .iter()
                .filter(|outline| !self.has_page_outline(outline.id))
                .map(|outline| PageObject::Outline(outline.snapshot())),
        );
        // Definitions stay for undo after their last paragraph lets go; the page, as
        // OneNote stores it, holds only those its paragraphs reference.
        let mut referenced = BTreeSet::new();
        for object in &objects {
            let outlines = match object {
                PageObject::Outline(outline) => std::slice::from_ref(outline),
                PageObject::Title(title) => title.outlines.as_slice(),
                _ => &[],
            };
            for (_, _, node) in outlines
                .iter()
                .flat_map(|outline| descendants(&outline.paragraphs, None))
            {
                referenced.extend(ops::references(node));
            }
        }
        let mut definitions = self.definitions.clone();
        definitions.retain(|id, _| referenced.contains(id));
        Ok(Page {
            title: self.header.title.clone(),
            identity: self.header.identity,
            created: self
                .date
                .as_ref()
                .map(PageDate::timestamp)
                .or(self.header.created),
            margin_origin: self.header.margin_origin,
            objects,
            definitions,
        })
    }

    /// Whether this outline occupies a slot in the imported page's paint order.
    pub fn has_page_outline(&self, id: ExGuid) -> bool {
        self.objects
            .iter()
            .any(|object| matches!(object, page::Content::Editable(candidate) if *candidate == id))
    }

    pub fn object_layouts(&self) -> impl Iterator<Item = (ExGuid, &onestore::document::Layout)> {
        self.outlines
            .iter()
            .map(|outline| (outline.id, &outline.layout))
            .chain(
                self.date
                    .iter()
                    .map(|date| (date.source().id, &date.source().layout)),
            )
            .chain(self.objects.iter().filter_map(page::Content::layout))
    }

    fn object_layout_mut(&mut self, id: ExGuid) -> Option<&mut onestore::document::Layout> {
        self.outlines
            .iter_mut()
            .map(|outline| (outline.id, &mut outline.layout))
            .chain(
                self.objects
                    .iter_mut()
                    .filter_map(page::Content::layout_mut),
            )
            .find_map(|(candidate, layout)| (candidate == id).then_some(layout))
    }

    fn header_bottom(&self, title: ExGuid, text_bottom: f32) -> f32 {
        self.objects
            .iter()
            .filter_map(|object| match object {
                page::Content::Outline {
                    source,
                    layout,
                    below_title,
                } if *below_title == Some(title) => {
                    Some(text_bottom + source.layout.y.unwrap_or(0.0) + layout.size[1])
                }
                page::Content::Date { below_title } if *below_title == Some(title) => {
                    self.date.as_ref().map(|date| {
                        text_bottom + date.source().layout.y.unwrap_or(0.0) + date.layout().size[1]
                    })
                }
                _ => None,
            })
            .fold(text_bottom, f32::max)
    }

    pub fn leave_title(&mut self, engine: &mut TextEngine) -> Result<(), EditorError> {
        let title = self.active_outline();
        if !title.title {
            return Ok(());
        }
        let bottom = self.header_bottom(title.id, title.bounds().y1 as f32);
        let position = [
            title.origin()[0],
            ((((f64::from(bottom) - 14.4) / 18.0).ceil() + 1.0) * 18.0 + 14.4) as f32,
        ];
        if let Some((id, origin)) = self
            .outlines()
            .iter()
            .rev()
            .find(|outline| {
                let bounds = outline.bounds();
                let [x, y] = position.map(f64::from);
                !outline.title
                    && x >= bounds.x0
                    && x <= bounds.x1
                    && y >= bounds.y0
                    && y <= bounds.y1
            })
            .map(|outline| (outline.id, outline.origin()))
        {
            self.focus_outline(id)?;
            self.select_at(position[0] - origin[0], position[1] - origin[1], false)?;
        } else {
            self.place_caret(engine, position, DEFAULT_OUTLINE_WIDTH)?;
        }
        Ok(())
    }

    fn title_flow(&self, height: f32) -> Result<Vec<Placement>, EditorError> {
        let title = self.active_outline();
        if !title.title {
            return Ok(Vec::new());
        }
        // Native flow uses text height even when the stored title bounds are taller.
        let old = self.header_bottom(title.id, title.origin()[1] + title.shaped.size[1]);
        let new = self.header_bottom(title.id, title.origin()[1] + height);
        if !old.is_finite() || !new.is_finite() {
            return Err(EditorError::InvalidGeometry);
        }
        if new <= old {
            return Ok(Vec::new());
        }
        let left = title.origin()[0] - 36.0;
        let right = title.origin()[0] + title.wrap_width() + 36.0;
        let destination = new + 11.52;
        let bodies = || {
            self.outlines
                .iter()
                .filter(|outline| !outline.title)
                .map(|outline| {
                    let rect = outline.bounds();
                    (
                        outline.id,
                        &outline.layout,
                        [
                            rect.x0 as f32,
                            rect.y0 as f32,
                            rect.x1 as f32,
                            rect.y1 as f32,
                        ],
                    )
                })
                .chain(self.objects.iter().filter_map(|object| match object {
                    page::Content::Image(image) if !image.background => {
                        let x = image.layout.x.unwrap_or(0.0);
                        let y = image.layout.y.unwrap_or(0.0);
                        Some((
                            image.id,
                            &image.layout,
                            [
                                x,
                                y,
                                x + image.layout.max_width?,
                                y + image.layout.max_height?,
                            ],
                        ))
                    }
                    page::Content::ReadOnly(object)
                        if !matches!(object.source, onestore::page::PageObject::Title(_)) =>
                    {
                        Some((object.source.id(), object.source.layout(), object.rect()))
                    }
                    _ => None,
                }))
        };
        let Some(first) = bodies()
            .filter(|(_, _, rect)| {
                rect[1] >= old + 10.8
                    && rect[1] < destination
                    && rect[0] <= right
                    && rect[2] >= left
            })
            .map(|(_, _, rect)| rect[1])
            .min_by(f32::total_cmp)
        else {
            return Ok(Vec::new());
        };
        let delta = destination - first;
        bodies()
            .filter(|(_, _, rect)| rect[1] >= first)
            .map(|(id, layout, rect)| {
                let y = rect[1] + delta;
                if !y.is_finite() || !(rect[3] + delta).is_finite() {
                    return Err(EditorError::InvalidGeometry);
                }
                Ok(Placement {
                    id,
                    position: [layout.x, Some(y)],
                })
            })
            .collect()
    }

    pub fn from_outlines(
        engine: &mut TextEngine,
        outlines: Vec<Outline>,
        definitions: BTreeMap<ExGuid, Definition>,
    ) -> Result<Self, EditorError> {
        let outlines = outlines
            .iter()
            .map(|outline| TextOutline::from_outline(engine, outline, &definitions))
            .collect::<Result<_, _>>()?;
        Self::from_text_outlines(outlines, definitions, None)
    }

    pub fn from_text_outlines(
        outlines: Vec<TextOutline>,
        definitions: BTreeMap<ExGuid, Definition>,
        date: Option<PageDate>,
    ) -> Result<Self, EditorError> {
        if outlines.is_empty() {
            return Err(EditError::InvalidRange.into());
        }
        let mut ids = BTreeSet::new();
        for (outline_id, paragraphs) in outlines
            .iter()
            .map(|outline| (outline.id, outline.document.nodes()))
            .chain(
                date.iter()
                    .map(|date| (date.source().id, date.source().paragraphs.as_slice())),
            )
        {
            if !ids.insert(outline_id) {
                return Err(EditError::InvalidStructure.into());
            }
            crate::document::validate_nodes(paragraphs, &mut ids)?;
        }
        Ok(Self {
            outlines,
            definitions,
            objects: Vec::new(),
            date,
            header: PageHeader::default(),
            active: Focus::Outline(0),
            undo: Vec::new(),
            redo: Vec::new(),
            composition: None,
            preferred_x: None,
            pending: None,
            ops: Ok(Vec::new()),
            stored: None,
        })
    }

    pub fn date(&self) -> Option<&PageDate> {
        self.date.as_ref()
    }

    pub fn change_date(
        &mut self,
        engine: &mut TextEngine,
        timestamp: u64,
        mut text: [String; 2],
    ) -> Result<bool, EditorError> {
        let date = self.date.as_ref().ok_or(EditError::InvalidRange)?;
        if timestamp == date.timestamp() {
            return Ok(false);
        }
        let mut source = date.source().clone();
        for ((field, _), paragraph) in date.fields().zip(&mut source.paragraphs) {
            let format = paragraph.text().unwrap().text.format_at(0)?.clone();
            paragraph.text_mut().unwrap().text =
                Paragraph::new(std::mem::take(&mut text[field as usize]), format);
        }
        let updated = PageDate::new(timestamp, source, engine, &self.definitions)?;
        self.finish_composition();
        self.undo
            .push(History::Date(Box::new(self.date.replace(updated).unwrap())));
        self.redo.clear();
        self.record(Ok(self.date_ops()));
        Ok(true)
    }

    /// Stored content; arrow-created paragraphs appear only in `visible_outlines`.
    pub fn outlines(&self) -> &[TextOutline] {
        &self.outlines
    }
    pub fn visible_outlines(&self) -> impl DoubleEndedIterator<Item = &TextOutline> {
        self.outlines
            .iter()
            .enumerate()
            .map(|(index, outline)| match &self.active {
                Focus::Draft {
                    index: active,
                    outline,
                } if index == *active => outline.as_ref(),
                _ => outline,
            })
    }

    pub fn active_outline(&self) -> &TextOutline {
        match &self.active {
            Focus::Outline(index) => &self.outlines[*index],
            Focus::Caret { outline, .. } | Focus::Draft { outline, .. } => outline,
        }
    }

    fn active_outline_mut(&mut self) -> &mut TextOutline {
        match &mut self.active {
            Focus::Outline(index) => &mut self.outlines[*index],
            Focus::Caret { outline, .. } | Focus::Draft { outline, .. } => outline,
        }
    }

    /// Provisional input geometry outside the document's outline collection.
    pub fn caret_outline(&self) -> Option<&TextOutline> {
        match &self.active {
            Focus::Caret { outline, .. } => Some(outline),
            Focus::Outline(_) | Focus::Draft { .. } => None,
        }
    }

    pub fn place_caret(
        &mut self,
        engine: &mut TextEngine,
        position: [f32; 2],
        width: f32,
    ) -> Result<(), EditorError> {
        let document = TextDocument::new(vec![Paragraph::new(String::new(), Default::default())])?;
        let outline = TextOutline::new(engine, document, width, position)?;
        self.finish_composition();
        self.active = Focus::Caret {
            outline: Box::new(outline),
            index: self.outlines.len(),
        };
        self.preferred_x = None;
        Ok(())
    }

    /// The format a paragraph style gives text, which clearing formatting returns to.
    fn style_format(&self, style: Option<ExGuid>) -> Result<onestore::document::Format, EditError> {
        Ok(match style {
            Some(id) => self
                .definitions
                .get(&id)
                .filter(|definition| {
                    matches!(definition.kind, onestore::document::Kind::Style { .. })
                })
                .ok_or(EditError::InvalidStructure)?
                .format
                .clone(),
            None => Default::default(),
        })
    }

    /// An empty paragraph in `base`'s style, whose runs keep `base`'s language as OneNote's do.
    fn blank_paragraph(&self, base: &PageParagraph) -> Result<PageParagraph, EditError> {
        let mut format = self.style_format(base.style)?;
        format.language = format.language.or(base
            .text()
            .and_then(|text| text.text.spans()[0].format.language));
        let mut node =
            crate::document::node(Paragraph::new(String::new(), format), base.format.clone())?;
        node.level = base.level;
        node.style = base.style;
        Ok(node)
    }

    pub fn select_below(
        &mut self,
        engine: &mut TextEngine,
        id: ExGuid,
        point: [f32; 2],
    ) -> Result<bool, EditorError> {
        if point.iter().any(|coordinate| !coordinate.is_finite()) {
            return Err(EditError::InvalidRange.into());
        }
        self.finish_composition();
        let Some(index) = self.outlines.iter().position(|outline| outline.id == id) else {
            return Ok(false);
        };
        let outline = &self.outlines[index];
        if !outline.contains_extension(point) {
            return Ok(false);
        }
        let draft = Box::new(match &self.active {
            Focus::Draft {
                outline,
                index: active,
            } if *active == index => (**outline).clone(),
            _ => outline.clone(),
        });
        let previous = std::mem::replace(
            &mut self.active,
            Focus::Draft {
                outline: draft,
                index,
            },
        );
        let preferred_x = self.preferred_x;
        let result = (|| {
            // Tiny source font sizes must not make one click generate unbounded paragraphs.
            for _ in 0..256 {
                if self.active_outline().shaped.size[1] >= point[1] {
                    break;
                }
                let outline = self.active_outline();
                let count = outline.document.nodes().len();
                let height = outline.shaped.size[1];
                let last = &outline.document.nodes()[count - 1];
                let node = PageParagraph {
                    parent: last.parent,
                    ..self.blank_paragraph(last)?
                };
                self.apply(
                    engine,
                    DocumentEdit {
                        columns: BTreeMap::new(),
                        container: None,
                        range: count..count,
                        replacement: vec![node],
                    },
                    outline.selection,
                    false,
                    None,
                )?;
                if self.active_outline().shaped.size[1] <= height {
                    return Err(EditError::InvalidStructure.into());
                }
            }
            if self.active_outline().shaped.size[1] < point[1] {
                return Err(EditError::TextTooLong.into());
            }
            self.select_at(point[0], point[1], false)?;
            Ok(true)
        })();
        if result.is_err() {
            self.active = previous;
            self.preferred_x = preferred_x;
        }
        result
    }

    pub fn focus_outline(&mut self, id: onestore::ExGuid) -> Result<(), EditError> {
        if self.active_outline().id != id && !self.outlines.iter().any(|outline| outline.id == id) {
            return Err(EditError::InvalidRange);
        }
        self.finish_composition();
        if let Some(index) = self.outlines.iter().position(|outline| outline.id == id)
            && !matches!(self.active, Focus::Draft { index: active, .. } if active == index)
        {
            self.active = Focus::Outline(index);
        }
        self.preferred_x = None;
        Ok(())
    }

    pub fn create_outline(
        &mut self,
        engine: &mut TextEngine,
        position: [f32; 2],
        width: f32,
    ) -> Result<onestore::ExGuid, EditorError> {
        let document = TextDocument::new(vec![Paragraph::new(
            String::new(),
            onestore::document::Format::default(),
        )])?;
        let outline = TextOutline::new(engine, document, width, position)?;
        let id = outline.id;
        self.finish_composition();
        self.undo.push(History::Remove {
            outline: id,
            focus: match &self.active {
                Focus::Outline(index) | Focus::Draft { index, .. } => {
                    RestoreFocus::Outline(self.outlines[*index].id)
                }
                Focus::Caret { outline, index } => RestoreFocus::Caret {
                    source: Box::new(outline.snapshot()),
                    selection: outline.selection,
                    index: *index,
                },
            },
        });
        self.redo.clear();
        self.active = Focus::Outline(self.outlines.len());
        self.outlines.push(outline);
        self.record(self.outline_ops(id, None));
        self.preferred_x = None;
        Ok(id)
    }

    pub fn move_outline(
        &mut self,
        id: onestore::ExGuid,
        position: [f32; 2],
    ) -> Result<(), EditError> {
        if !position.iter().all(|value| value.is_finite()) {
            return Err(EditError::InvalidRange);
        }
        let outline = if self.active_outline().id == id {
            self.active_outline()
        } else {
            self.outlines
                .iter()
                .find(|outline| outline.id == id)
                .ok_or(EditError::InvalidRange)?
        };
        if outline.title {
            return Err(EditError::UnsupportedContent);
        }
        if position == outline.origin() {
            return Ok(());
        }
        self.finish_composition();
        if self.caret_outline().is_some_and(|outline| outline.id == id) {
            let outline = self.active_outline_mut();
            outline.layout.x = Some(position[0]);
            outline.layout.y = Some(position[1]);
            self.preferred_x = None;
            return Ok(());
        }
        let index = self
            .outlines
            .iter()
            .position(|outline| outline.id == id)
            .unwrap();
        let layout = &mut self.outlines[index].layout;
        let previous = [layout.x, layout.y];
        self.undo.push(History::Position {
            outline: id,
            position: previous,
        });
        self.redo.clear();
        layout.x = Some(position[0]);
        layout.y = Some(position[1]);
        self.record(self.placement_ops(&Placement {
            id,
            position: previous,
        }));
        self.active = Focus::Outline(index);
        self.preferred_x = None;
        Ok(())
    }

    /// Origin and size of a picture the user can select; OneNote passes clicks through
    /// backgrounds.
    pub fn image_placement(&self, id: ExGuid) -> Option<([f32; 2], [f32; 2])> {
        if let Some(image) = self.image(id) {
            let layout = &image.layout;
            return Some((
                [layout.x.unwrap_or(0.0), layout.y.unwrap_or(0.0)],
                [layout.max_width?, layout.max_height?],
            ));
        }
        let (outline, ..) = self.outline_picture(id)?;
        let outline = self.outlines.iter().find(|item| item.id == outline)?;
        let [x0, y0, x1, y1] = outline.shaped.objects.iter().find(|o| o.id == id)?.rect;
        let origin = outline.origin();
        Some(([origin[0] + x0, origin[1] + y0], [x1 - x0, y1 - y0]))
    }

    /// Whether the picture sits in an outline's flow, where only its size can change.
    pub fn image_in_outline(&self, id: ExGuid) -> bool {
        self.outline_picture(id).is_some()
    }

    /// A picture held as a paragraph: its outline, the paragraph's container and index there,
    /// and the paragraph.
    fn outline_picture(
        &self,
        id: ExGuid,
    ) -> Option<(ExGuid, Option<ExGuid>, usize, &PageParagraph)> {
        self.outlines.iter().find_map(|outline| {
            descendants(outline.document.nodes(), None).find_map(|(container, index, node)| {
                matches!(&node.content, onestore::page::ParagraphContent::Image(image) if image.id == id)
                    .then_some((outline.id, container, index, node))
            })
        })
    }

    /// Replaces a picture paragraph through the outline's text history.
    fn edit_outline_picture(
        &mut self,
        engine: &mut TextEngine,
        id: ExGuid,
        replacement: Option<PageParagraph>,
    ) -> Result<(), EditorError> {
        let (outline, container, index, _) =
            self.outline_picture(id).ok_or(EditError::InvalidRange)?;
        self.focus_outline(outline)?;
        let selection = self.selection();
        self.commit(
            engine,
            DocumentEdit {
                columns: BTreeMap::new(),
                container,
                range: index..index + 1,
                replacement: replacement.into_iter().collect(),
            },
            selection,
        )
    }

    fn image(&self, id: ExGuid) -> Option<&onestore::page::Image> {
        self.objects.iter().find_map(|object| match object {
            page::Content::Image(image) if image.id == id && !image.background => Some(image),
            _ => None,
        })
    }

    fn image_mut(&mut self, id: ExGuid) -> Option<&mut onestore::page::Image> {
        self.objects.iter_mut().find_map(|object| match object {
            page::Content::Image(image) if image.id == id && !image.background => Some(image),
            _ => None,
        })
    }

    /// Moves a picture, and resizes it when `size` differs from the stored size. A picture in
    /// an outline's flow keeps its place.
    pub fn place_image(
        &mut self,
        engine: &mut TextEngine,
        id: ExGuid,
        origin: [f32; 2],
        size: [f32; 2],
    ) -> Result<(), EditorError> {
        if !origin.iter().chain(&size).all(|value| value.is_finite())
            || size.iter().any(|value| *value <= 0.0)
        {
            return Err(EditError::InvalidRange.into());
        }
        if let Some((.., node)) = self.outline_picture(id) {
            let mut node = node.clone();
            let onestore::page::ParagraphContent::Image(image) = &mut node.content else {
                unreachable!()
            };
            if crate::outline::image_size(image) == Some(size) {
                return Ok(());
            }
            [image.layout.max_width, image.layout.max_height] = size.map(Some);
            image.layout.width_set_by_user = Some(true);
            return self.edit_outline_picture(engine, id, Some(node));
        }
        let image = self.image(id).ok_or(EditError::InvalidRange)?;
        let mut layout = image.layout.clone();
        [layout.x, layout.y] = origin.map(Some);
        if [layout.max_width, layout.max_height] != size.map(Some) {
            [layout.max_width, layout.max_height] = size.map(Some);
            layout.width_set_by_user = Some(true);
        }
        if layout == image.layout {
            return Ok(());
        }
        self.finish_composition();
        let previous = std::mem::replace(&mut self.image_mut(id).unwrap().layout, layout);
        self.undo.push(History::Image {
            image: id,
            layout: previous,
        });
        self.redo.clear();
        let image = self.image(id).unwrap();
        self.record(Ok(ops::picture_layout(image)));
        Ok(())
    }

    /// Leaves a picture as OneNote's Left and Right arrows do: the caret goes to the end of the
    /// text outline before it in page order, or to the start of the one after. False when there
    /// is none.
    pub fn step_from_image(
        &mut self,
        engine: &mut TextEngine,
        id: ExGuid,
        forward: bool,
    ) -> Result<bool, EditorError> {
        if let Some((outline, ..)) = self.outline_picture(id) {
            // Within an outline, the caret goes to the text beside the picture.
            let source = self
                .outlines
                .iter()
                .find(|item| item.id == outline)
                .unwrap();
            let mut leaf = 0;
            let mut before = None;
            let mut after = None;
            let mut passed = false;
            for (_, _, node) in descendants(source.document.nodes(), None) {
                match &node.content {
                    onestore::page::ParagraphContent::Image(image) if image.id == id => {
                        passed = true
                    }
                    onestore::page::ParagraphContent::Text(text) => {
                        let end = text.text.utf16_offset(text.text.text().len())?;
                        if passed {
                            after = after.or(Some((leaf, 0)));
                        } else {
                            before = Some((leaf, end));
                        }
                        leaf += 1;
                    }
                    _ => {}
                }
            }
            let Some((paragraph, offset)) = (if forward { after } else { before }) else {
                return Ok(false);
            };
            self.focus_outline(outline)?;
            self.select(Selection {
                positions: [TextPosition { paragraph, offset }; 2],
                affinities: [Affinity::Downstream; 2],
            })?;
            return Ok(true);
        }
        let index = self
            .objects
            .iter()
            .position(|object| matches!(object, page::Content::Image(image) if image.id == id))
            .ok_or(EditError::InvalidRange)?;
        // An emptied outline keeps its slot for undo after leaving `outlines`.
        let editable = |object: &page::Content| match object {
            page::Content::Editable(outline)
                if self.outlines.iter().any(|item| item.id == *outline) =>
            {
                Some(*outline)
            }
            _ => None,
        };
        let outline = if forward {
            self.objects[index + 1..].iter().find_map(editable)
        } else {
            self.objects[..index].iter().rev().find_map(editable)
        };
        let Some(outline) = outline else {
            return Ok(false);
        };
        self.focus_outline(outline)?;
        let edge = if forward {
            Movement::DocumentStart
        } else {
            Movement::DocumentEnd
        };
        self.move_selection(engine, edge, false)?;
        Ok(true)
    }

    pub fn remove_image(&mut self, engine: &mut TextEngine, id: ExGuid) -> Result<(), EditorError> {
        if self.outline_picture(id).is_some() {
            return self.edit_outline_picture(engine, id, None);
        }
        let index = self
            .objects
            .iter()
            .position(|object| {
                matches!(object, page::Content::Image(image) if image.id == id && !image.background)
            })
            .ok_or(EditError::InvalidRange)?;
        self.finish_composition();
        let page::Content::Image(image) = self.objects.remove(index) else {
            unreachable!()
        };
        self.undo.push(History::Picture {
            index,
            image: Some(Box::new(image)),
        });
        self.redo.clear();
        self.record(Ok(vec![PageOp::Delete { object: id }]));
        Ok(())
    }

    pub fn selection(&self) -> Selection {
        self.active_outline().selection
    }

    /// OneNote's 18 pt placement grid passes through this point.
    pub fn margin_origin(&self) -> [f32; 2] {
        self.header.margin_origin
    }

    pub fn resize(&mut self, engine: &mut TextEngine, width: f32) -> Result<(), EditorError> {
        let outline = self.active_outline();
        if outline.layout.reserved_width.or(outline.layout.max_width) == Some(width)
            && outline.layout.width_set_by_user == Some(true)
        {
            return Ok(());
        }
        let previous = outline.layout.clone();
        let resized = self.preview_resize(engine, width)?;
        self.finish_composition();
        if let Focus::Draft { index, .. } = self.active {
            self.active = Focus::Outline(index);
        }
        let id = resized.id;
        *self.active_outline_mut() = resized;
        if self.caret_outline().is_none() {
            self.record(ops::layout_ops(
                id,
                &previous,
                &self.active_outline().layout,
            ));
            self.undo.push(History::Layout {
                outline: id,
                layout: previous,
            });
            self.redo.clear();
        }
        self.preferred_x = None;
        Ok(())
    }

    pub fn preview_resize(
        &self,
        engine: &mut TextEngine,
        width: f32,
    ) -> Result<TextOutline, EditorError> {
        let outline = match &self.active {
            Focus::Draft { index, .. } => &self.outlines[*index],
            _ => self.active_outline(),
        };
        if outline.title {
            return Err(EditError::UnsupportedContent.into());
        }
        if !width.is_finite() || width <= 0.0 {
            return Err(LayoutError::InvalidWidth.into());
        }
        let width = width.max(outline.shaped.table_width());
        let shaped = OutlineLayout::flow(
            outline.document.nodes().iter(),
            &outline.indents,
            width,
            true,
            0,
            None,
            &mut |paragraph, previous, width, indents| {
                ParagraphLayout::shape(
                    engine,
                    paragraph,
                    previous,
                    width,
                    indents,
                    &self.definitions,
                )
            },
        )?;
        let mut layout = outline.layout.clone();
        layout.max_width = Some(width);
        layout.reserved_width = None;
        layout.width_set_by_user = Some(true);
        Ok(TextOutline {
            id: outline.id,
            title: outline.title,
            min_width: outline.min_width,
            layout,
            document: outline.document.clone(),
            indents: outline.indents.clone(),
            shaped,
            selection: outline.selection,
        })
    }

    pub fn select(&mut self, selection: Selection) -> Result<(), EditError> {
        for position in selection.positions {
            self.active_outline().visible_index(position.paragraph)?;
            self.active_outline()
                .document
                .paragraph(position.paragraph)
                .ok_or(EditError::InvalidRange)?
                .byte_offset(position.offset)?;
        }
        self.finish_composition();
        self.preferred_x = None;
        self.active_outline_mut().selection = selection;
        Ok(())
    }

    pub fn select_all(&mut self) -> Result<(), EditError> {
        let paragraph = self
            .active_outline()
            .source_index(self.active_outline().shaped.paragraphs.len() - 1);
        let last = self.active_outline().document.paragraph(paragraph).unwrap();
        self.select(
            [
                TextPosition {
                    paragraph: 0,
                    offset: 0,
                },
                TextPosition {
                    paragraph,
                    offset: last.utf16_offset(last.text().len())?,
                },
            ]
            .into(),
        )
    }

    pub fn select_at(&mut self, x: f32, y: f32, extend: bool) -> Result<(), EditError> {
        let mut selection = self.selection_at(x, y, SelectionUnit::Grapheme)?;
        if extend {
            selection.positions[0] = self.selection().positions[0];
            selection.affinities[0] = self.selection().affinities[0];
        }
        self.select(selection)
    }

    pub fn selection_at(
        &self,
        x: f32,
        y: f32,
        unit: SelectionUnit,
    ) -> Result<Selection, EditError> {
        if !x.is_finite() || !y.is_finite() {
            return Err(EditError::InvalidRange);
        }
        let index = self.active_outline().paragraph_at(x, y)?;
        let paragraph = &self.active_outline().shaped.paragraphs[index];
        let layout = &paragraph.text.shaped;
        let point = [x - paragraph.origin[0], y - paragraph.origin[1]];
        let local = edit::selection_at(
            layout,
            paragraph.text.hit_test(point[0], point[1]),
            point[0],
            unit,
        );
        let mut selection = self.selection();
        for (slot, cursor) in [local.anchor(), local.focus()].into_iter().enumerate() {
            let visible = paragraph.projection.text().utf16_offset(cursor.index())?;
            selection.positions[slot] = TextPosition {
                paragraph: self.active_outline().source_index(index),
                offset: paragraph
                    .projection
                    .source_offset(visible, crate::affinity(cursor.affinity()))?,
            };
            selection.affinities[slot] = cursor.affinity();
        }
        if unit == SelectionUnit::Paragraph
            && index + 1 < self.active_outline().shaped.paragraphs.len()
        {
            let document = &self.active_outline().document;
            let source = selection.positions[0].paragraph;
            let next = self.active_outline().source_index(index + 1);
            let (container, local, _) = document.leaf(source).unwrap();
            let (next_container, next_local, _) = document.leaf(next).unwrap();
            if container == next_container && next_local == local + 1 {
                selection.positions[1] = TextPosition {
                    paragraph: next,
                    offset: 0,
                };
                selection.affinities[1] = Affinity::Downstream;
            }
        }
        Ok(selection)
    }

    pub fn caret(&self, width: f32) -> Result<BoundingBox, EditError> {
        let focus = self.active_outline().selection.positions[1];
        let paragraph = self.active_outline().paragraph_layout(focus.paragraph)?;
        let cursor =
            paragraph.cursor(focus.offset, self.active_outline().selection.affinities[1])?;
        let mut rect = paragraph.text.caret(cursor, width);
        rect.x0 += f64::from(paragraph.origin[0]);
        rect.x1 += f64::from(paragraph.origin[0]);
        rect.y0 += f64::from(paragraph.origin[1]);
        rect.y1 += f64::from(paragraph.origin[1]);
        Ok(rect)
    }

    pub fn move_selection(
        &mut self,
        engine: &mut TextEngine,
        movement: Movement,
        extend: bool,
    ) -> Result<(), EditorError> {
        self.finish_composition();
        let [anchor, focus] = self.active_outline().selection.positions;
        if !extend
            && self.caret_outline().is_some()
            && self
                .active_outline()
                .document
                .paragraphs()
                .next()
                .unwrap()
                .text()
                .is_empty()
            && let Some(step) = match movement {
                Movement::Left => Some([-18.0, 0.0]),
                Movement::Right => Some([18.0, 0.0]),
                Movement::Up => Some([0.0, -18.0]),
                Movement::Down => Some([0.0, 18.0]),
                _ => None,
            }
        {
            let origin = self.active_outline().origin();
            let position = [origin[0] + step[0], origin[1] + step[1]];
            let center_y = position[1] + self.caret(1.0)?.height() as f32 * 0.5;
            let target = self
                .outlines
                .iter()
                .rev()
                .find(|outline| {
                    let rect = outline.bounds();
                    let bottom = rect.y1
                        + if matches!(movement, Movement::Up) {
                            36.0
                        } else {
                            0.0
                        };
                    (rect.x0..=rect.x1).contains(&f64::from(position[0]))
                        && (rect.y0..=bottom).contains(&f64::from(center_y))
                })
                .map(|outline| (outline.id, outline.origin()));
            if let Some((id, origin)) = target {
                let x = position[0]
                    + if step[0] == 0.0 {
                        self.preferred_x.unwrap_or(0.0)
                    } else {
                        0.0
                    }
                    - origin[0];
                self.focus_outline(id)?;
                self.select_at(x, center_y - origin[1], false)?;
                self.preferred_x = (step[0] == 0.0).then_some(x);
            } else {
                let outline = self.active_outline_mut();
                outline.layout.x = Some(position[0]);
                outline.layout.y = Some(position[1]);
                if step[0] != 0.0 {
                    self.preferred_x = None;
                }
            }
            return Ok(());
        }
        if matches!(
            movement,
            Movement::ParagraphStart
                | Movement::ParagraphEnd
                | Movement::DocumentStart
                | Movement::DocumentEnd
        ) {
            let outline = self.active_outline();
            let current = outline.visible_index(focus.paragraph)?;
            let end = matches!(movement, Movement::ParagraphEnd | Movement::DocumentEnd);
            let current_end = outline.document.paragraph(focus.paragraph).unwrap();
            let current_end = current_end.utf16_offset(current_end.text().len())?;
            let index = match movement {
                Movement::DocumentStart => 0,
                Movement::DocumentEnd => outline.shaped.paragraphs.len() - 1,
                Movement::ParagraphStart if focus.offset == 0 => current.saturating_sub(1),
                Movement::ParagraphEnd if focus.offset == current_end => {
                    (current + 1).min(outline.shaped.paragraphs.len() - 1)
                }
                _ => current,
            };
            let source = outline.source_index(index);
            let paragraph = outline.document.paragraph(source).unwrap();
            let position = TextPosition {
                paragraph: source,
                offset: if end {
                    paragraph.utf16_offset(paragraph.text().len())?
                } else {
                    0
                },
            };
            let affinity = if end {
                Affinity::Upstream
            } else {
                Affinity::Downstream
            };
            let selection = &mut self.active_outline_mut().selection;
            if !extend {
                selection.positions[0] = position;
                selection.affinities[0] = affinity;
            }
            selection.positions[1] = position;
            selection.affinities[1] = affinity;
            self.preferred_x = None;
            return Ok(());
        }
        let horizontal = matches!(
            movement,
            Movement::Left | Movement::Right | Movement::WordLeft | Movement::WordRight
        );
        let left = matches!(movement, Movement::Left | Movement::WordLeft);
        if horizontal && !extend && anchor.paragraph != focus.paragraph {
            let index = usize::from(if left { anchor > focus } else { anchor < focus });
            self.active_outline_mut().selection.positions =
                [self.active_outline().selection.positions[index]; 2];
            self.active_outline_mut().selection.affinities =
                [self.active_outline().selection.affinities[index]; 2];
            self.preferred_x = None;
            return Ok(());
        }
        let mut index = self.active_outline().visible_index(focus.paragraph)?;
        let paragraph = &self.active_outline().shaped.paragraphs[index];
        let cursor =
            paragraph.cursor(focus.offset, self.active_outline().selection.affinities[1])?;
        let mut next;
        if matches!(movement, Movement::Up | Movement::Down) {
            let caret = paragraph.text.caret(cursor, 1.0);
            let x = self
                .preferred_x
                .unwrap_or(caret.x0 as f32 + paragraph.origin[0]);
            let line = paragraph
                .text
                .lines()
                .position(|(_, line)| f64::from(line.top) == caret.y0)
                .unwrap();
            let up = matches!(movement, Movement::Up);
            let target = if up {
                line.checked_sub(1)
                    .and_then(|line| paragraph.text.lines().nth(line))
            } else {
                paragraph.text.lines().nth(line + 1)
            }
            .map(|(_, line)| line);
            let target = if target.is_some() {
                target
            } else {
                let neighbor = if self.active_outline().shaped.tables.is_empty() {
                    if up {
                        index.checked_sub(1)
                    } else {
                        (index + 1 < self.active_outline().shaped.paragraphs.len())
                            .then_some(index + 1)
                    }
                } else {
                    let column = self.active_outline().column(x, paragraph.id)?;
                    let current = column
                        .iter()
                        .position(|candidate| *candidate == index)
                        .ok_or(EditError::InvalidStructure)?;
                    if up {
                        current.checked_sub(1).map(|i| column[i])
                    } else {
                        column.get(current + 1).copied()
                    }
                };
                neighbor.and_then(|neighbor| {
                    index = neighbor;
                    let paragraph = &self.active_outline().shaped.paragraphs[index];
                    if up {
                        paragraph.text.lines().last()
                    } else {
                        paragraph.text.lines().next()
                    }
                    .map(|(_, line)| line)
                })
            };
            if target.is_none() && !extend && anchor == focus && !self.active_outline().title {
                let outline = self.active_outline();
                let last = outline.document.nodes().len() - 1;
                let up = matches!(movement, Movement::Up);
                let base_node = &outline.document.nodes()[if up { 0 } else { last }];
                let node = self.blank_paragraph(base_node)?;
                let trailing = outline
                    .document
                    .nodes()
                    .iter()
                    .rev()
                    .take_while(|node| {
                        node.text()
                            .is_some_and(|text| text.text.text().bytes().all(|byte| byte == b' '))
                    })
                    .count();
                let position = if up {
                    Some([
                        outline.origin()[0] + x,
                        (((f64::from(outline.origin()[1]) / 18.0).ceil() - 3.0) * 18.0 + 14.4)
                            as f32,
                    ])
                } else if outline.origin()[0] >= 180.0 && trailing >= 2 {
                    let node = &outline.document.nodes()[last.saturating_sub(trailing)];
                    let bottom = match &node.content {
                        onestore::page::ParagraphContent::Text(_) => {
                            let paragraph = outline
                                .shaped
                                .paragraphs
                                .iter()
                                .find(|paragraph| paragraph.id == node.id)
                                .ok_or(EditError::InvalidStructure)?;
                            f64::from(paragraph.origin[1]) + f64::from(paragraph.text.height())
                        }
                        onestore::page::ParagraphContent::Table(table) => {
                            let cell = outline
                                .shaped
                                .tables
                                .iter()
                                .find(|layout| layout.id == table.id)
                                .and_then(|layout| layout.cells.last())
                                .ok_or(EditError::InvalidStructure)?;
                            f64::from(cell.rect[3]) + 1.68
                        }
                        onestore::page::ParagraphContent::Image(onestore::page::Image {
                            id,
                            ..
                        })
                        | onestore::page::ParagraphContent::Attachment(
                            onestore::page::Attachment { id, .. },
                        )
                        | onestore::page::ParagraphContent::Ink(onestore::page::Ink {
                            id, ..
                        })
                        | onestore::page::ParagraphContent::Unsupported(
                            onestore::page::Unsupported { id, .. },
                        ) => outline
                            .shaped
                            .objects
                            .iter()
                            .find(|object| object.id == *id)
                            .map(|object| f64::from(object.bottom))
                            .ok_or(EditError::InvalidStructure)?,
                    } + f64::from(outline.origin()[1]);
                    let cell = (bottom - 14.4) / 18.0;
                    // Stored coordinates can straddle a grid boundary by one f32 ULP.
                    let grid = if (cell - cell.round()).abs()
                        <= f64::from(f32::EPSILON) * cell.abs().max(1.0)
                    {
                        cell.round()
                    } else {
                        cell.ceil()
                    };
                    Some([outline.origin()[0], ((grid + 3.0) * 18.0 + 14.4) as f32])
                } else {
                    None
                };
                if let Some(position) = position {
                    let outline = TextOutline::new(
                        engine,
                        TextDocument::from_nodes(vec![node])?,
                        DEFAULT_OUTLINE_WIDTH,
                        position,
                    )?;
                    self.active = Focus::Caret {
                        outline: Box::new(outline),
                        index: self.outlines.len(),
                    };
                    self.preferred_x = Some(if up { 0.0 } else { x });
                    return Ok(());
                }
                let edit = DocumentEdit {
                    columns: BTreeMap::new(),
                    container: None,
                    range: last + 1..last + 1,
                    replacement: vec![PageParagraph {
                        parent: base_node.parent,
                        ..node
                    }],
                };
                let paragraph = outline.document.paragraphs().count();
                let previous = if let Focus::Outline(index) = self.active {
                    let draft = self.outlines[index].clone();
                    Some(std::mem::replace(
                        &mut self.active,
                        Focus::Draft {
                            index,
                            outline: Box::new(draft),
                        },
                    ))
                } else {
                    None
                };
                if let Err(error) = self.apply(
                    engine,
                    edit,
                    [TextPosition {
                        paragraph,
                        offset: 0,
                    }; 2]
                        .into(),
                    false,
                    None,
                ) {
                    if let Some(previous) = previous {
                        self.active = previous;
                    }
                    return Err(error);
                }
                self.preferred_x = Some(x);
                return Ok(());
            }
            next = target
                .map(|line| {
                    self.active_outline().shaped.paragraphs[index]
                        .text
                        .hit_test(
                            x - self.active_outline().shaped.paragraphs[index].origin[0],
                            line.top + line.height * 0.5,
                        )
                })
                .unwrap_or(cursor);
            self.preferred_x = Some(x);
        } else {
            let local = if anchor.paragraph == focus.paragraph {
                ParagraphSelection::new(
                    paragraph
                        .cursor(anchor.offset, self.active_outline().selection.affinities[0])?,
                    cursor,
                )
            } else {
                cursor.into()
            };
            next = edit::step(&paragraph.text.shaped, local, movement, extend).focus();
            if horizontal && next == cursor && (extend || anchor == focus) {
                let neighbor = if left {
                    index.checked_sub(1)
                } else {
                    (index + 1 < self.active_outline().shaped.paragraphs.len()).then_some(index + 1)
                };
                if let Some(neighbor) = neighbor {
                    index = neighbor;
                    let paragraph = &self.active_outline().shaped.paragraphs[index];
                    let line = if left {
                        paragraph.text.lines().last()
                    } else {
                        paragraph.text.lines().next()
                    }
                    .unwrap()
                    .1;
                    next = paragraph.text.hit_test(
                        if left { f32::MAX } else { -1.0 },
                        line.top + line.height * 0.5,
                    );
                    if matches!(movement, Movement::WordLeft | Movement::WordRight) {
                        next = edit::word_cursor(&paragraph.text.shaped, next, left);
                    }
                }
            }
        }
        if !extend
            && anchor == focus
            && next == cursor
            && self.active_outline().source_index(index) == focus.paragraph
            && matches!(movement, Movement::Left | Movement::Right)
        {
            let caret = self.caret(1.0)?;
            let origin = self.active_outline().origin();
            let x = caret.x0 + f64::from(origin[0]);
            let y = (caret.y0 + caret.y1) * 0.5 + f64::from(origin[1]);
            let target = self
                .outlines
                .iter()
                .filter(|outline| outline.id != self.active_outline().id)
                .filter_map(|outline| {
                    let rect = outline.bounds();
                    let distance = if left { x - rect.x1 } else { rect.x0 - x };
                    ((rect.y0..rect.y1).contains(&y) && distance > 0.0).then_some((
                        distance,
                        outline.id,
                        outline.origin(),
                    ))
                })
                .min_by(|a, b| a.0.total_cmp(&b.0));
            if let Some((_, id, origin)) = target {
                self.focus_outline(id)?;
                self.select_at(
                    if left { f32::MAX } else { -1.0 },
                    y as f32 - origin[1],
                    false,
                )?;
                return Ok(());
            }
        }
        if !matches!(movement, Movement::Up | Movement::Down) {
            self.preferred_x = None;
        }
        let paragraph = &self.active_outline().shaped.paragraphs[index];
        let visible = paragraph.projection.text().utf16_offset(next.index())?;
        let position = TextPosition {
            paragraph: self.active_outline().source_index(index),
            offset: paragraph
                .projection
                .source_offset(visible, crate::affinity(next.affinity()))?,
        };
        if !extend {
            self.active_outline_mut().selection.positions[0] = position;
            self.active_outline_mut().selection.affinities[0] = next.affinity();
        }
        self.active_outline_mut().selection.positions[1] = position;
        self.active_outline_mut().selection.affinities[1] = next.affinity();
        Ok(())
    }

    pub fn selection_rects(&self) -> Result<Vec<BoundingBox>, EditError> {
        self.rectangles(self.active_outline().selection)
    }

    pub fn marked_rects(&self) -> Result<Vec<BoundingBox>, EditError> {
        match &self.composition {
            Some(composition) => self.rectangles(Selection {
                positions: [composition.range.start, composition.range.end],
                affinities: [Affinity::Downstream, Affinity::Upstream],
            }),
            None => Ok(Vec::new()),
        }
    }

    fn rectangles(&self, selection: Selection) -> Result<Vec<BoundingBox>, EditError> {
        let [anchor, focus] = selection.positions;
        if anchor == focus {
            return Ok(Vec::new());
        }
        let (start, end, affinities) = if anchor < focus {
            (anchor, focus, selection.affinities)
        } else {
            (
                focus,
                anchor,
                [selection.affinities[1], selection.affinities[0]],
            )
        };
        let mut rectangles = Vec::new();
        for (visible, (index, paragraph)) in self
            .active_outline()
            .layouts()
            .enumerate()
            .filter(|(_, (index, _))| *index >= start.paragraph && *index <= end.paragraph)
        {
            let source = self.active_outline().document.paragraph(index).unwrap();
            let first = if index == start.paragraph {
                paragraph.cursor(start.offset, affinities[0])?
            } else {
                paragraph.cursor(0, Affinity::Downstream)?
            };
            let last = if index == end.paragraph {
                paragraph.cursor(end.offset, affinities[1])?
            } else {
                paragraph.cursor(
                    source.utf16_offset(source.text().len())?,
                    Affinity::Upstream,
                )?
            };
            let mut local = paragraph
                .text
                .selection(ParagraphSelection::new(first, last));
            if index < end.paragraph {
                local.push(paragraph.text.caret(last, 4.0));
            }
            for mut rect in local {
                rect.x0 += f64::from(paragraph.origin[0]);
                rect.x1 += f64::from(paragraph.origin[0]);
                rect.y0 += f64::from(paragraph.origin[1]);
                rect.y1 += f64::from(paragraph.origin[1]);
                if let Some(cell) = self.active_outline().shaped.paragraph_cell(visible) {
                    let Some(clipped) = cell.clip(rect) else {
                        continue;
                    };
                    rect = clipped;
                }
                rectangles.push(rect);
            }
        }
        Ok(rectangles)
    }

    pub fn replace(
        &mut self,
        engine: &mut TextEngine,
        replacement: Vec<Paragraph>,
    ) -> Result<(), EditorError> {
        let start = self.active_outline().selection.positions[0]
            .min(self.active_outline().selection.positions[1]);
        let end = self.active_outline().selection.positions[0]
            .max(self.active_outline().selection.positions[1]);
        let last = replacement.last().ok_or(EditError::InvalidRange)?;
        let offset = last.utf16_offset(last.text().len())?;
        let offset = if replacement.len() == 1 {
            start
                .offset
                .checked_add(offset)
                .ok_or(EditError::TextTooLong)?
        } else {
            offset
        };
        let caret = TextPosition {
            paragraph: start
                .paragraph
                .checked_add(replacement.len() - 1)
                .ok_or(EditError::TextTooLong)?,
            offset,
        };
        let edit = self
            .active_outline()
            .document
            .replace(start..end, replacement)?;
        self.commit(
            engine,
            edit,
            Selection {
                positions: [caret; 2],
                affinities: [Affinity::Upstream; 2],
            },
        )
    }

    pub fn insert(&mut self, engine: &mut TextEngine, text: &str) -> Result<(), EditorError> {
        let start = self.active_outline().selection.positions[0]
            .min(self.active_outline().selection.positions[1]);
        let format = self.typing_format(start)?;
        let replacement = text
            .split('\n')
            .map(|line| Paragraph::new(line.to_owned(), format.clone()))
            .collect();
        self.replace(engine, replacement)
    }

    /// Pastes plain text as OneNote does: lines become plain Calibri 11 paragraphs without style
    /// or list between the halves of the caret's paragraph (`evidence/structural-edits/xml/c7-*`).
    /// Pasted runs take the clipboard's `language`, an LCID, not the caret's run's as typing
    /// does; Windows derives it from the keyboard language at copy time.
    pub fn paste(
        &mut self,
        engine: &mut TextEngine,
        text: &str,
        language: u32,
    ) -> Result<(), EditorError> {
        let lines = text
            .split('\n')
            .map(|line| line.strip_suffix('\r').unwrap_or(line))
            .collect::<Vec<_>>();
        let last = lines[lines.len() - 1];
        let [anchor, focus] = self.active_outline().selection.positions;
        let (start, end) = (anchor.min(focus), anchor.max(focus));
        if lines.len() == 1 {
            let mut format = self.typing_format(start)?;
            format.language = Some(language);
            return self.replace(engine, vec![Paragraph::new(last.to_owned(), format)]);
        }
        let edge = Paragraph::new(String::new(), self.typing_format(start)?);
        let pasted = Format {
            font: Some("Calibri".into()),
            font_size: Some(11.0),
            language: Some(language),
            ..Format::default()
        };
        let mut edit = self.active_outline().document.replace(
            start..end,
            std::iter::once(edge.clone())
                .chain(
                    lines
                        .iter()
                        .map(|line| Paragraph::new((*line).to_owned(), pasted.clone())),
                )
                .chain([edge])
                .collect(),
        )?;
        for node in &mut edit.replacement[1..=lines.len()] {
            node.style = None;
            node.lists.clear();
        }
        let caret = TextPosition {
            paragraph: start.paragraph + lines.len(),
            offset: u32::try_from(last.encode_utf16().count())
                .map_err(|_| EditError::TextTooLong)?,
        };
        self.commit(
            engine,
            edit,
            Selection {
                positions: [caret; 2],
                affinities: [Affinity::Upstream; 2],
            },
        )
    }

    /// Enter as OneNote 2010 does (`evidence/structural-edits`): the new paragraph takes the
    /// level, lists and character formatting at the caret but no note tags. At a paragraph's
    /// start its tags stay with its text below; an empty paragraph opens a plain one above and
    /// leaves the list it ends; a heading continues as body text.
    fn split(&mut self, engine: &mut TextEngine) -> Result<(), EditorError> {
        let [anchor, focus] = self.active_outline().selection.positions;
        let (start, end) = (anchor.min(focus), anchor.max(focus));
        let format = self.typing_format(start)?;
        let document = &self.active_outline().document;
        let mut edit =
            document.replace(start..end, vec![Paragraph::new(String::new(), format.clone()); 2])?;
        let nodes = document.container(edit.container)?;
        let next = nodes
            .get(crate::document::subtree_end(nodes, edit.range.start))
            .filter(|next| {
                let node = &nodes[edit.range.start];
                next.parent == node.parent && next.level == node.level
            });
        let [head, tail, ..] = &mut edit.replacement[..] else {
            unreachable!("a split holds both halves")
        };
        let empty = |node: &PageParagraph| node.text().unwrap().text.text().is_empty();
        if start.offset == 0 {
            tail.tags = std::mem::take(&mut head.tags);
            tail.text_mut().unwrap().tags = std::mem::take(&mut head.text_mut().unwrap().tags);
        }
        let style = |id: Option<ExGuid>| match &self.definitions.get(&id?)?.kind {
            Kind::Style { name } => name.as_deref(),
            _ => None,
        };
        if empty(head) && empty(tail) {
            head.lists.clear();
            if next.is_none_or(|next| next.lists.is_empty()) {
                tail.lists.clear();
            }
        } else if empty(tail)
            && matches!(
                style(head.style),
                Some("h1" | "h2" | "h3" | "h4" | "h5" | "h6")
            )
        {
            tail.style = self
                .definitions
                .keys()
                .copied()
                .find(|id| style(Some(*id)) == Some("p"));
            // The heading's run formatting carries over under the body style (`c9-h1-*`).
            let run = format.over(&self.style_format(head.style)?);
            tail.text_mut().unwrap().text = Paragraph::new(
                String::new(),
                run.inherit(&self.style_format(tail.style)?),
            );
        }
        let caret = TextPosition {
            paragraph: start.paragraph + 1,
            offset: 0,
        };
        self.commit(
            engine,
            edit,
            Selection {
                positions: [caret; 2],
                affinities: [Affinity::Upstream; 2],
            },
        )
    }

    /// Tab or Shift+Tab at the start of the selected paragraphs; see [`crate::document::indent`].
    /// A default bullet or number steps to the style OneNote gives its new depth.
    pub fn indent(&mut self, engine: &mut TextEngine, outdent: bool) -> Result<bool, EditorError> {
        let outline = self.active_outline();
        if outline.title {
            return Ok(false);
        }
        let selection = outline.selection;
        let [anchor, focus] = selection.positions;
        let start = anchor.min(focus);
        let end = anchor.max(focus);
        let (container, local_start, _) = outline
            .document
            .leaf(start.paragraph)
            .ok_or(EditError::InvalidRange)?;
        let (end_container, local_end, _) = outline
            .document
            .leaf(end.paragraph)
            .ok_or(EditError::InvalidRange)?;
        if container != end_container {
            return Err(EditError::UnsupportedContent.into());
        }
        let range = local_start
            ..local_end + usize::from(end.offset != 0 || start.paragraph == end.paragraph);
        let nodes = outline.document.container(container)?;
        let Some(mut edit) = crate::document::indent(nodes, container, range, outdent) else {
            return Ok(false);
        };
        let levels = nodes[edit.range.clone()]
            .iter()
            .map(|node| node.level)
            .collect::<Vec<_>>();
        for (node, level) in edit.replacement.iter_mut().zip(levels) {
            if node.level == level {
                continue;
            }
            for list in &mut node.lists {
                let Some(definition) = self
                    .definitions
                    .get(list)
                    .and_then(|definition| format::nested_list(definition, node.level > level))
                else {
                    continue;
                };
                *list = onestore::page::text::new_id()?;
                self.definitions.insert(*list, definition);
            }
        }
        self.commit(engine, edit, selection)?;
        Ok(true)
    }

    /// Deletes the selection or the character beside the caret. Backspace at a paragraph's start
    /// first removes its list, then outdents it, then joins it to the paragraph above, whose
    /// properties win; Delete at its end joins the paragraph below (`evidence/structural-edits/
    /// xml/c4-*`, `c5b-*`). Joins pass over a collapsed paragraph's hidden children.
    pub fn delete(&mut self, engine: &mut TextEngine, backward: bool) -> Result<bool, EditorError> {
        let selection = self.active_outline().selection;
        let [anchor, focus] = selection.positions;
        let mut range = anchor.min(focus)..anchor.max(focus);
        if range.is_empty() {
            let outline = self.active_outline();
            let paragraph = outline.paragraph_layout(focus.paragraph)?;
            let cursor = paragraph.cursor(focus.offset, selection.affinities[1])?;
            let Some(cluster) =
                cursor.logical_clusters(&paragraph.text.shaped)[usize::from(!backward)]
            else {
                let (container, local, node) = outline
                    .document
                    .leaf(focus.paragraph)
                    .ok_or(EditError::InvalidRange)?;
                if backward && !node.lists.is_empty() {
                    let item = PageParagraph {
                        lists: Vec::new(),
                        ..node.clone()
                    };
                    let edit = DocumentEdit {
                        columns: BTreeMap::new(),
                        container,
                        range: local..local + 1,
                        replacement: vec![item],
                    };
                    self.commit(engine, edit, selection)?;
                    return Ok(true);
                }
                if backward && node.level > 1 {
                    return self.indent(engine, true);
                }
                let visible = outline.visible_index(focus.paragraph)?;
                let Some(neighbor) = (if backward {
                    visible.checked_sub(1)
                } else {
                    Some(visible + 1).filter(|next| *next < outline.shaped.paragraphs.len())
                }) else {
                    return Ok(false);
                };
                let neighbor = outline.source_index(neighbor);
                let [upper, lower] = if backward {
                    [neighbor, focus.paragraph]
                } else {
                    [focus.paragraph, neighbor]
                };
                let (_, _, top) = outline
                    .document
                    .leaf(upper)
                    .ok_or(EditError::InvalidRange)?;
                let base = self.style_format(top.style)?;
                let Some(edit) = outline.document.join(upper, lower, &base)? else {
                    return Ok(false);
                };
                let text = outline.document.paragraph(upper).unwrap();
                let caret = TextPosition {
                    paragraph: upper,
                    offset: text.utf16_offset(text.text().len())?,
                };
                self.commit(
                    engine,
                    edit,
                    Selection {
                        positions: [caret; 2],
                        affinities: [Affinity::Upstream; 2],
                    },
                )?;
                return Ok(true);
            };
            let visible = paragraph.projection.text();
            let bytes = cluster.text_range();
            range.start.offset = paragraph.projection.source_offset(
                visible.utf16_offset(bytes.start)?,
                onestore::page::text::Affinity::Downstream,
            )?;
            range.end.offset = paragraph.projection.source_offset(
                visible.utf16_offset(bytes.end)?,
                onestore::page::text::Affinity::Upstream,
            )?;
        }
        self.delete_range(engine, range)
    }

    pub fn delete_to(
        &mut self,
        engine: &mut TextEngine,
        movement: Movement,
    ) -> Result<bool, EditorError> {
        self.finish_composition();
        let original = self.selection();
        let preferred_x = self.preferred_x;
        if original.positions[0] == original.positions[1] {
            let result = self.move_selection(engine, movement, true);
            if let Err(error) = result {
                self.active_outline_mut().selection = original;
                self.preferred_x = preferred_x;
                return Err(error);
            }
        }
        let [anchor, focus] = self.selection().positions;
        self.active_outline_mut().selection = original;
        self.preferred_x = preferred_x;
        self.delete_range(engine, anchor.min(focus)..anchor.max(focus))
    }

    fn delete_range(
        &mut self,
        engine: &mut TextEngine,
        range: Range<TextPosition>,
    ) -> Result<bool, EditorError> {
        if range.is_empty() {
            return Ok(false);
        }
        let caret = range.start;
        let format = self
            .active_outline()
            .document
            .paragraph(caret.paragraph)
            .unwrap()
            .format_at(caret.offset)?
            .clone();
        let edit = self
            .active_outline()
            .document
            .replace(range, vec![Paragraph::new(String::new(), format)])?;
        self.commit(
            engine,
            edit,
            Selection {
                positions: [caret; 2],
                affinities: [Affinity::Upstream; 2],
            },
        )?;
        Ok(true)
    }

    pub fn undo(&mut self, engine: &mut TextEngine) -> Result<bool, EditorError> {
        if self.composition.is_some() {
            return self.cancel_composition(engine);
        }
        let Some(history) = self.undo.pop() else {
            return Ok(false);
        };
        match self.apply_history(engine, history) {
            Ok(inverse) => {
                self.redo.push(inverse);
                Ok(true)
            }
            Err((history, error)) => {
                self.undo.push(history);
                Err(error)
            }
        }
    }

    pub fn redo(&mut self, engine: &mut TextEngine) -> Result<bool, EditorError> {
        self.finish_composition();
        let Some(history) = self.redo.pop() else {
            return Ok(false);
        };
        match self.apply_history(engine, history) {
            Ok(inverse) => {
                self.undo.push(inverse);
                Ok(true)
            }
            Err((history, error)) => {
                self.redo.push(history);
                Err(error)
            }
        }
    }

    fn apply_history(
        &mut self,
        engine: &mut TextEngine,
        history: History,
    ) -> Result<History, (History, EditorError)> {
        self.pending = None;
        let valid = match &history {
            History::Date(date) => self
                .date
                .as_ref()
                .is_some_and(|current| current.source().id == date.source().id),
            History::Draft {
                outlines, index, ..
            } => {
                *index <= self.outlines.len()
                    && self
                        .outlines
                        .iter()
                        .position(|item| item.id == outlines[0].id)
                        .is_none_or(|found| found == *index)
            }
            History::Text { outline, .. }
            | History::Position { outline, .. }
            | History::Layout { outline, .. } => {
                self.outlines.iter().any(|item| item.id == *outline)
            }
            History::Image { image, .. } => self.image(*image).is_some(),
            History::Picture {
                index,
                image: Some(_),
            } => *index <= self.objects.len(),
            History::Picture { index, image: None } => {
                matches!(self.objects.get(*index), Some(page::Content::Image(_)))
            }
            History::Remove { outline, focus } => {
                self.outlines.iter().any(|item| item.id == *outline)
                    && match focus {
                        RestoreFocus::Outline(focus) => {
                            outline != focus && self.outlines.iter().any(|item| item.id == *focus)
                        }
                        RestoreFocus::Caret { .. } => true,
                    }
            }
            History::Insert { index, .. } | History::Restore { index, .. } => {
                *index <= self.outlines.len()
            }
        };
        if !valid {
            return Err((history, EditError::InvalidRange.into()));
        }
        let inverse = match history {
            History::Date(date) => {
                let shown = self.date.replace(*date).unwrap();
                self.record(Ok(self.date_ops()));
                History::Date(Box::new(shown))
            }
            History::Draft {
                outlines,
                index,
                caret,
                selection,
                restore_caret,
            } => {
                let draft = if restore_caret {
                    match TextOutline::from_outline(engine, &caret, &self.definitions) {
                        Ok(mut draft) => {
                            draft.selection = selection;
                            Some(draft)
                        }
                        Err(error) => {
                            return Err((
                                History::Draft {
                                    outlines,
                                    index,
                                    caret,
                                    selection,
                                    restore_caret,
                                },
                                error,
                            ));
                        }
                    }
                } else {
                    None
                };
                let outline = outlines[usize::from(!restore_caret)].clone();
                let id = outline.id;
                let stored = self
                    .outlines
                    .get(index)
                    .is_some_and(|item| item.id == id)
                    .then(|| self.outlines.remove(index));
                self.active = if outline.is_empty() {
                    Focus::Caret {
                        outline: Box::new(outline),
                        index,
                    }
                } else {
                    self.outlines.insert(index, outline);
                    match draft {
                        Some(outline) => Focus::Draft {
                            index,
                            outline: Box::new(outline),
                        },
                        None => Focus::Outline(index),
                    }
                };
                self.record(self.outline_ops(id, stored.as_ref()));
                History::Draft {
                    outlines,
                    index,
                    caret,
                    selection,
                    restore_caret: !restore_caret,
                }
            }
            History::Text { outline, change } => {
                let index = self
                    .outlines
                    .iter()
                    .position(|item| item.id == outline)
                    .unwrap();
                let previous = std::mem::replace(&mut self.active, Focus::Outline(index));
                let inverse = match self.apply(
                    engine,
                    change.edit.clone(),
                    change.selection,
                    false,
                    Some(&change.positions),
                ) {
                    Ok(change) => change,
                    Err(error) => {
                        self.active = previous;
                        return Err((History::Text { outline, change }, error));
                    }
                };
                self.record(self.change_ops(&self.outlines[index], &inverse));
                History::Text {
                    outline,
                    change: Box::new(inverse),
                }
            }
            History::Position { outline, position } => {
                let index = self
                    .outlines
                    .iter()
                    .position(|item| item.id == outline)
                    .unwrap();
                let layout = &mut self.outlines[index].layout;
                let previous = [layout.x, layout.y];
                [layout.x, layout.y] = position;
                self.active = Focus::Outline(index);
                self.record(self.placement_ops(&Placement {
                    id: outline,
                    position: previous,
                }));
                History::Position {
                    outline,
                    position: previous,
                }
            }
            History::Layout { outline, layout } => {
                let index = self
                    .outlines
                    .iter()
                    .position(|item| item.id == outline)
                    .unwrap();
                let mut source = self.outlines[index].snapshot();
                let previous = std::mem::replace(&mut source.layout, layout);
                let mut resized =
                    match TextOutline::from_outline(engine, &source, &self.definitions) {
                        Ok(resized) => resized,
                        Err(error) => {
                            return Err((
                                History::Layout {
                                    outline,
                                    layout: source.layout,
                                },
                                error,
                            ));
                        }
                    };
                resized.selection = self.outlines[index].selection;
                self.outlines[index] = resized;
                self.record(ops::layout_ops(
                    outline,
                    &previous,
                    &self.outlines[index].layout,
                ));
                self.active = Focus::Outline(index);
                History::Layout {
                    outline,
                    layout: previous,
                }
            }
            History::Image { image, layout } => {
                let previous =
                    std::mem::replace(&mut self.image_mut(image).unwrap().layout, layout);
                self.record(Ok(ops::picture_layout(self.image(image).unwrap())));
                History::Image {
                    image,
                    layout: previous,
                }
            }
            History::Picture { index, image } => History::Picture {
                index,
                image: match image {
                    Some(image) => {
                        self.objects.insert(index, page::Content::Image(*image));
                        let page::Content::Image(image) = &self.objects[index] else {
                            unreachable!()
                        };
                        let ops = vec![PageOp::Add {
                            object: PageObject::Image(image.clone()),
                            before: self.successor(image.id),
                        }];
                        self.record(Ok(ops));
                        None
                    }
                    None => match self.objects.remove(index) {
                        page::Content::Image(image) => {
                            self.record(Ok(vec![PageOp::Delete { object: image.id }]));
                            Some(Box::new(image))
                        }
                        _ => unreachable!(),
                    },
                },
            },
            History::Remove { outline, focus } => {
                let index = self
                    .outlines
                    .iter()
                    .position(|item| item.id == outline)
                    .unwrap();
                let next_focus = match &focus {
                    RestoreFocus::Caret {
                        source,
                        selection,
                        index,
                    } => match TextOutline::from_outline(engine, source, &self.definitions) {
                        Ok(mut outline) => {
                            outline.selection = *selection;
                            Focus::Caret {
                                outline: Box::new(outline),
                                index: *index,
                            }
                        }
                        Err(error) => return Err((History::Remove { outline, focus }, error)),
                    },
                    RestoreFocus::Outline(id) => {
                        let previous = self
                            .outlines
                            .iter()
                            .position(|outline| outline.id == *id)
                            .unwrap();
                        Focus::Outline(previous - usize::from(previous > index))
                    }
                };
                let outline = Box::new(self.outlines.remove(index));
                self.record(Ok(vec![PageOp::Delete { object: outline.id }]));
                self.active = next_focus;
                History::Insert {
                    index,
                    outline,
                    focus,
                }
            }
            History::Restore {
                index,
                source,
                selection,
                focus,
            } => {
                let mut outline =
                    match TextOutline::from_outline(engine, &source, &self.definitions) {
                        Ok(outline) => outline,
                        Err(error) => {
                            return Err((
                                History::Restore {
                                    index,
                                    source,
                                    selection,
                                    focus,
                                },
                                error,
                            ));
                        }
                    };
                outline.selection = selection;
                return self.apply_history(
                    engine,
                    History::Insert {
                        index,
                        outline: Box::new(outline),
                        focus,
                    },
                );
            }
            History::Insert {
                index,
                outline,
                focus,
            } => {
                let id = outline.id;
                self.outlines.insert(index, *outline);
                self.record(self.outline_ops(id, None));
                self.active = Focus::Outline(index);
                History::Remove { outline: id, focus }
            }
        };
        self.preferred_x = None;
        Ok(inverse)
    }

    pub fn marked_range(&self) -> Option<Range<TextPosition>> {
        self.composition
            .as_ref()
            .map(|composition| composition.range.clone())
    }

    pub fn compose(
        &mut self,
        engine: &mut TextEngine,
        text: String,
        selected: Range<u32>,
    ) -> Result<(), EditorError> {
        if selected.start > selected.end {
            return Err(EditError::InvalidRange.into());
        }
        let range = self.marked_range().unwrap_or_else(|| {
            self.active_outline().selection.positions[0]
                .min(self.active_outline().selection.positions[1])
                ..self.active_outline().selection.positions[0]
                    .max(self.active_outline().selection.positions[1])
        });
        let positions = [
            inserted_position(range.start, &text, selected.start)?,
            inserted_position(range.start, &text, selected.end)?,
        ];
        let end = inserted_position(
            range.start,
            &text,
            text.encode_utf16()
                .count()
                .try_into()
                .map_err(|_| EditError::TextTooLong)?,
        )?;
        let format = self.typing_format(range.start)?;
        let replacement = text
            .split('\n')
            .map(|part| Paragraph::new(part.to_owned(), format.clone()))
            .collect();
        let mut edit = self
            .active_outline()
            .document
            .replace(range.clone(), replacement)?;
        self.own_lists(&mut edit)?;
        let inverse = self.apply(
            engine,
            edit,
            Selection {
                positions,
                affinities: [Affinity::Downstream; 2],
            },
            false,
            None,
        )?;
        let original = if let Some(composition) = self.composition.take() {
            let mut original = composition.original;
            // Nodes this edit reaches past what the composition changed are still as they were.
            let changed = original.edit.range.len();
            original
                .edit
                .replacement
                .extend(inverse.edit.replacement.iter().skip(changed).cloned());
            original.edit.range = inverse.edit.range;
            for (id, widths) in inverse.edit.columns {
                original.edit.columns.entry(id).or_insert(widths);
            }
            for position in inverse.positions {
                if !original
                    .positions
                    .iter()
                    .any(|original| original.id == position.id)
                {
                    original.positions.push(position);
                }
            }
            original
        } else {
            inverse
        };
        self.composition = Some(Composition {
            original,
            range: range.start..end,
        });
        Ok(())
    }

    pub fn finish_composition(&mut self) {
        if let Some(composition) = self.composition.take() {
            self.record_change(composition.original);
        }
    }

    pub fn cancel_composition(&mut self, engine: &mut TextEngine) -> Result<bool, EditorError> {
        let Some(composition) = &self.composition else {
            return Ok(false);
        };
        let original = composition.original.clone();
        self.apply(
            engine,
            original.edit,
            original.selection,
            false,
            Some(&original.positions),
        )?;
        self.composition = None;
        Ok(true)
    }

    pub fn commit_text(
        &mut self,
        engine: &mut TextEngine,
        text: String,
    ) -> Result<(), EditorError> {
        if self.composition.is_some() {
            let end = text
                .encode_utf16()
                .count()
                .try_into()
                .map_err(|_| EditError::TextTooLong)?;
            self.compose(engine, text, end..end)?;
            self.finish_composition();
            Ok(())
        } else {
            self.insert(engine, &text)
        }
    }

    fn commit(
        &mut self,
        engine: &mut TextEngine,
        mut edit: DocumentEdit,
        selection: Selection,
    ) -> Result<(), EditorError> {
        self.own_lists(&mut edit)?;
        let inverse = self.apply(engine, edit, selection, true, None)?;
        self.record_change(inverse);
        Ok(())
    }

    /// Gives each paragraph an edit adds its own copy of the lists it carries, as OneNote keeps
    /// a list node per paragraph.
    fn own_lists(&mut self, edit: &mut DocumentEdit) -> Result<(), EditError> {
        let existing = descendants(
            &self.active_outline().document.container(edit.container)?[edit.range.clone()],
            None,
        )
        .map(|(_, _, node)| node.id)
        .collect::<BTreeSet<_>>();
        let mut pending = edit.replacement.iter_mut().collect::<Vec<_>>();
        while let Some(node) = pending.pop() {
            if !existing.contains(&node.id) {
                for list in &mut node.lists {
                    let definition = self
                        .definitions
                        .get(list)
                        .ok_or(EditError::InvalidStructure)?
                        .clone();
                    *list = onestore::page::text::new_id()?;
                    self.definitions.insert(*list, definition);
                }
            }
            if let ParagraphContent::Table(table) = &mut node.content {
                pending.extend(
                    table
                        .rows
                        .iter_mut()
                        .flat_map(|row| &mut row.cells)
                        .flat_map(|cell| &mut cell.paragraphs),
                );
            }
        }
        Ok(())
    }

    fn record_change(&mut self, change: TextChange) {
        self.pending = None;
        if let Focus::Draft { index, .. } = self.active {
            let Focus::Draft { mut outline, .. } =
                std::mem::replace(&mut self.active, Focus::Outline(index))
            else {
                unreachable!()
            };
            let mut caret = outline.snapshot();
            crate::document::container_mut(&mut caret.paragraphs, change.edit.container)
                .expect("inverse edit container exists")
                .splice(change.edit.range, change.edit.replacement);
            let mut columns = change.edit.columns;
            crate::document::swap_columns(&mut caret.paragraphs, &mut columns);
            let last_selected = outline.selection.positions[0]
                .paragraph
                .max(outline.selection.positions[1].paragraph);
            let mut count = outline.document.nodes().len();
            let mut selected_end = 0;
            let selected_root = outline
                .document
                .nodes()
                .iter()
                .position(|node| {
                    selected_end += leaves(std::slice::from_ref(node), None).count();
                    selected_end > last_selected
                })
                .expect("selection addresses a text leaf");
            let original_ids = self.outlines[index]
                .document
                .nodes()
                .iter()
                .map(|node| node.id)
                .collect::<BTreeSet<_>>();
            while count > selected_root + 1 {
                let node = &outline.document.nodes()[count - 1];
                if node.text().is_none_or(|text| !text.text.text().is_empty())
                    || original_ids.contains(&node.id)
                {
                    break;
                }
                count -= 1;
            }
            if count < outline.document.nodes().len() {
                let edit = DocumentEdit {
                    columns: BTreeMap::new(),
                    container: None,
                    range: count..outline.document.nodes().len(),
                    replacement: Vec::new(),
                };
                let relayout = outline
                    .shaped
                    .relayout(
                        outline.document.nodes(),
                        &edit,
                        edit.range.clone(),
                        &outline.indents,
                        outline.wrap_width(),
                        outline.layout.width_set_by_user == Some(true),
                        &mut |_, _, _, _| Err(LayoutError::UnsupportedContent),
                    )
                    .expect("removing trailing text lays nothing out anew");
                outline
                    .document
                    .apply(edit)
                    .expect("removing a provisional suffix preserves a valid document");
                outline.shaped.commit(relayout);
            }
            if outline.document == self.outlines[index].document {
                self.outlines[index].selection = outline.selection;
                return;
            }
            let versions = Box::new([self.outlines[index].clone(), (*outline).clone()]);
            if outline.is_empty() {
                self.outlines.remove(index);
                self.active = Focus::Caret { outline, index };
            } else {
                self.outlines[index] = *outline;
            }
            self.record(self.outline_ops(versions[0].id, Some(&versions[0])));
            self.undo.push(History::Draft {
                outlines: versions,
                index,
                caret: Box::new(caret),
                selection: change.selection,
                restore_caret: true,
            });
        } else if self.caret_outline().is_some() {
            if self.active_outline().is_empty() {
                return;
            }
            let Focus::Caret { index, .. } = self.active else {
                unreachable!()
            };
            let Focus::Caret { outline, .. } =
                std::mem::replace(&mut self.active, Focus::Outline(index))
            else {
                unreachable!();
            };
            let mut source = outline.snapshot();
            source.paragraphs = change.edit.replacement;
            let id = outline.id;
            self.undo.push(History::Remove {
                outline: id,
                focus: RestoreFocus::Caret {
                    source: Box::new(source),
                    selection: change.selection,
                    index,
                },
            });
            self.outlines.insert(index, *outline);
            self.record(self.outline_ops(id, None));
        } else if self.active_outline().is_empty() && !self.active_outline().title {
            let Focus::Outline(index) = self.active else {
                unreachable!()
            };
            let outline = self.outlines.remove(index);
            self.record(Ok(vec![PageOp::Delete { object: outline.id }]));
            debug_assert_eq!(change.edit.range, 0..1);
            let mut source = outline.snapshot();
            source.paragraphs = change.edit.replacement;
            self.undo.push(History::Restore {
                index,
                source: Box::new(source),
                selection: change.selection,
                focus: RestoreFocus::Caret {
                    source: Box::new(outline.snapshot()),
                    selection: outline.selection,
                    index,
                },
            });
            self.active = Focus::Caret {
                outline: Box::new(outline),
                index,
            };
        } else {
            self.record(self.change_ops(self.active_outline(), &change));
            self.undo.push(History::Text {
                outline: self.active_outline().id,
                change: Box::new(change),
            });
        }
        self.redo.clear();
    }

    fn apply(
        &mut self,
        engine: &mut TextEngine,
        edit: DocumentEdit,
        selection: Selection,
        finish_composition: bool,
        positions: Option<&[Placement]>,
    ) -> Result<TextChange, EditorError> {
        let outline = self.active_outline();
        outline.document.validate_edit(&edit)?;
        let range = match edit.container {
            None => edit.range.clone(),
            Some(cell) => {
                let root = outline.document.root(cell)?;
                root..root + 1
            }
        };
        let relayout = outline.shaped.relayout(
            outline.document.nodes(),
            &edit,
            range,
            &outline.indents,
            outline.wrap_width(),
            outline.layout.width_set_by_user == Some(true),
            &mut |node, previous, width, indents| {
                ParagraphLayout::shape(engine, node, previous, width, indents, &self.definitions)
            },
        )?;
        let [replaced, ..] = outline.shaped.pieces(relayout.range.clone());
        for position in selection.positions {
            let (node, before) = outline
                .document
                .edited_leaf(&edit, position.paragraph)
                .ok_or(EditError::InvalidRange)?;
            node.text().unwrap().text.byte_offset(position.offset)?;
            // A paragraph the edit leaves keeps its layout unless that layout is replaced.
            if !relayout
                .segment
                .paragraphs
                .iter()
                .any(|paragraph| paragraph.id == node.id)
                && !before
                    .and_then(|before| outline.visible_index(before).ok())
                    .is_some_and(|index| !replaced.contains(&index))
            {
                return Err(EditError::InvalidRange.into());
            }
        }
        let placements = match positions {
            Some(positions) => positions.to_vec(),
            None => self.title_flow(relayout.size[1])?,
        };
        let previous_positions = placements
            .iter()
            .map(|placement| {
                let layout = self
                    .object_layouts()
                    .find_map(|(id, layout)| (id == placement.id).then_some(layout))
                    .ok_or(EditError::InvalidStructure)?;
                if placement
                    .position
                    .iter()
                    .flatten()
                    .any(|value| !value.is_finite())
                {
                    return Err(EditorError::InvalidGeometry);
                }
                Ok(Placement {
                    id: placement.id,
                    position: [layout.x, layout.y],
                })
            })
            .collect::<Result<Vec<_>, EditorError>>()?;
        if finish_composition && self.composition.is_some() {
            // Committing marked text can remove provisional paragraphs from the validated view.
            self.finish_composition();
            return self.apply(engine, edit, selection, false, positions);
        }
        let outline = self.active_outline_mut();
        let inverse = outline.document.splice(edit);
        outline.shaped.commit(relayout);
        self.preferred_x = None;
        let previous_selection =
            std::mem::replace(&mut self.active_outline_mut().selection, selection);
        for placement in placements {
            let layout = self
                .object_layout_mut(placement.id)
                .expect("validated object position");
            [layout.x, layout.y] = placement.position;
        }
        Ok(TextChange {
            edit: inverse,
            selection: previous_selection,
            positions: previous_positions,
        })
    }
}

/// Where `position` in `old` lies in `new`, the same outline changed elsewhere: in the same
/// paragraph, past what changed in its text when it lies after it; at the start of the
/// paragraph now at its place when that one is gone.
fn follow(old: &TextDocument, new: &TextDocument, position: TextPosition) -> TextPosition {
    let found = old.leaf(position.paragraph).and_then(|(_, _, node)| {
        let paragraph = new
            .text_nodes()
            .position(|candidate| candidate.id == node.id)?;
        Some((paragraph, node, new.leaf(paragraph)?.2))
    });
    let Some((paragraph, before, after)) = found else {
        let count = new.text_nodes().count();
        return TextPosition {
            paragraph: position.paragraph.min(count.saturating_sub(1)),
            offset: 0,
        };
    };
    let units = |text: &str| text.encode_utf16().count() as u32;
    let (a, b) = (
        before.text().unwrap().text.text(),
        after.text().unwrap().text.text(),
    );
    let prefix = units(
        &a[..a
            .char_indices()
            .zip(b.chars())
            .find(|((_, x), y)| x != y)
            .map_or(a.len().min(b.len()), |((at, _), _)| at)],
    );
    let suffix = a
        .chars()
        .rev()
        .zip(b.chars().rev())
        .take_while(|(x, y)| x == y)
        .map(|(x, _)| x.len_utf16() as u32)
        .scan(0, |sum, units| {
            *sum += units;
            Some(*sum)
        })
        .take_while(|sum| prefix + sum <= units(a).min(units(b)))
        .last()
        .unwrap_or(0);
    let (old_length, new_length) = (units(a), units(b));
    let offset = if position.offset <= prefix {
        position.offset
    } else if position.offset >= old_length - suffix {
        new_length - (old_length - position.offset)
    } else {
        new_length - suffix
    };
    TextPosition { paragraph, offset }
}

fn inserted_position(
    mut position: TextPosition,
    text: &str,
    offset: u32,
) -> Result<TextPosition, EditError> {
    let mut units = 0_u32;
    for character in text.chars() {
        if units == offset {
            return Ok(position);
        }
        units = units
            .checked_add(character.len_utf16() as u32)
            .ok_or(EditError::TextTooLong)?;
        if units > offset {
            return Err(EditError::InvalidRange);
        }
        if character == '\n' {
            position.paragraph = position
                .paragraph
                .checked_add(1)
                .ok_or(EditError::TextTooLong)?;
            position.offset = 0;
        } else {
            position.offset = position
                .offset
                .checked_add(character.len_utf16() as u32)
                .ok_or(EditError::TextTooLong)?;
        }
    }
    if units == offset {
        Ok(position)
    } else {
        Err(EditError::InvalidRange)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use onestore::document::Format;

    fn table_editor(engine: &mut TextEngine) -> CanvasEditor {
        use onestore::page::text::new_id;
        use onestore::page::{ParagraphContent, Table, TableCell, TableColumn, TableRow};
        let mut editor = CanvasEditor::new(
            engine,
            TextDocument::new(vec![Paragraph::new("Left".into(), Format::default())]).unwrap(),
            180.0,
        )
        .unwrap();
        let first = editor.active_outline().document.nodes()[0].clone();
        let wrapper = PageParagraph {
            id: new_id().unwrap(),
            parent: None,
            level: 1,
            style: None,
            format: Format::default(),
            lists: Vec::new(),
            tags: Vec::new(),
            media: Default::default(),
            collapsed: false,
            content: ParagraphContent::Table(Table {
                id: new_id().unwrap(),
                columns: vec![
                    TableColumn {
                        width: 72.0,
                        locked: true
                    };
                    2
                ],
                rows: [["Left", "Right"], ["Below", "Last"]]
                    .into_iter()
                    .enumerate()
                    .map(|(row, texts)| TableRow {
                        id: new_id().unwrap(),
                        cells: texts
                            .into_iter()
                            .enumerate()
                            .map(|(column, text)| TableCell {
                                id: new_id().unwrap(),
                                layout: Default::default(),
                                indents: vec![18.0, 0.0, 27.0],
                                shading: None,
                                paragraphs: vec![if row == 0 && column == 0 {
                                    first.clone()
                                } else {
                                    crate::document::node(
                                        Paragraph::new(text.into(), Format::default()),
                                        Format::default(),
                                    )
                                    .unwrap()
                                }],
                                unsupported: Vec::new(),
                            })
                            .collect(),
                    })
                    .collect(),
                borders: Some(true),
                layout: Default::default(),
                tags: Vec::new(),
            }),
        };
        editor
            .commit(
                engine,
                DocumentEdit {
                    columns: BTreeMap::new(),
                    container: None,
                    range: 0..1,
                    replacement: vec![wrapper],
                },
                [TextPosition {
                    paragraph: 1,
                    offset: 0,
                }; 2]
                    .into(),
            )
            .unwrap();
        editor
    }

    #[test]
    fn structured_construction_and_resize_preserve_cells_and_enclose_locked_columns() {
        let mut engine = TextEngine::default();
        let mut source = table_editor(&mut engine).active_outline().snapshot();
        let onestore::page::ParagraphContent::Table(table) = &mut source.paragraphs[0].content
        else {
            panic!()
        };
        table.rows[0].cells[0].paragraphs[0]
            .text_mut()
            .unwrap()
            .text = Paragraph::new(String::new(), Format::default());
        source.paragraphs.push(
            crate::document::node(
                Paragraph::new(
                    "Text below the table must use the same width during drag and after release."
                        .into(),
                    Format::default(),
                ),
                Format::default(),
            )
            .unwrap(),
        );
        let document = TextDocument::from_nodes(source.paragraphs.clone()).unwrap();
        let mut editor = CanvasEditor::new(&mut engine, document.clone(), 180.0).unwrap();
        let imported = TextOutline::from_outline(&mut engine, &source, &BTreeMap::new()).unwrap();
        let cells = |outline: &TextOutline| {
            outline
                .shaped
                .tables
                .iter()
                .flat_map(|table| {
                    table
                        .cells
                        .iter()
                        .map(move |cell| (table.id, table.borders, cell.id, cell.rect))
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(editor.active_outline().layout.width_set_by_user, Some(true));
        assert_eq!(editor.active_outline().shaped.size, imported.shaped.size);
        assert_eq!(cells(editor.active_outline()), cells(&imported));
        for (actual, expected) in editor
            .active_outline()
            .shaped
            .paragraphs
            .iter()
            .zip(&imported.shaped.paragraphs)
        {
            assert_eq!(actual.id, expected.id);
            assert_eq!(actual.origin, expected.origin);
            assert_eq!(actual.text.height(), expected.text.height());
        }
        let minimum = 2.0 * (72.0 + 4.98) - 1.83;
        let preview = editor.preview_resize(&mut engine, 36.0).unwrap();
        assert!((preview.bounds().width() as f32 - minimum).abs() < 0.0001);
        assert_eq!(preview.document, document);
        assert_eq!(cells(&preview), cells(&imported));
        assert!((preview.wrap_width() - minimum).abs() < 0.0001);
        let final_width_preview = editor
            .preview_resize(&mut engine, preview.bounds().width() as f32)
            .unwrap();
        assert_eq!(preview.shaped.size, final_width_preview.shaped.size);
        assert_eq!(
            preview
                .shaped
                .paragraphs
                .last()
                .unwrap()
                .text
                .lines()
                .count(),
            final_width_preview
                .shaped
                .paragraphs
                .last()
                .unwrap()
                .text
                .lines()
                .count()
        );
        assert_eq!(editor.active_outline().bounds().width(), 180.0);
        assert!(editor.undo.is_empty());
        editor.resize(&mut engine, 36.0).unwrap();
        assert_eq!(editor.active_outline().bounds(), preview.bounds());
        editor.undo(&mut engine).unwrap();
        assert_eq!(editor.active_outline().bounds().width(), 180.0);
        editor.redo(&mut engine).unwrap();
        assert_eq!(editor.active_outline().bounds(), preview.bounds());
        assert_eq!(editor.active_outline().document, document);
    }

    #[test]
    fn nested_table_extents_remain_reachable_beyond_the_parent_columns() {
        use onestore::page::ParagraphContent;
        let mut engine = TextEngine::default();
        let mut source = table_editor(&mut engine).active_outline().snapshot();
        let mut nested = table_editor(&mut engine).active_outline().document.nodes()[0].clone();
        let ParagraphContent::Table(table) = &mut nested.content else {
            panic!()
        };
        table.columns[0].width = 240.0;
        table.columns[1].width = 240.0;
        let ParagraphContent::Table(table) = &mut source.paragraphs[0].content else {
            panic!()
        };
        table.rows[0].cells[1].paragraphs = vec![nested];
        source.layout.width_set_by_user = Some(false);
        let mut editor =
            CanvasEditor::from_outlines(&mut engine, vec![source], BTreeMap::new()).unwrap();
        let paragraph = editor.active_outline().paragraph_layout(2).unwrap();
        let point = [
            paragraph.origin[0] + 1.0,
            paragraph.origin[1] + paragraph.text.height() * 0.5,
        ];
        assert!(point[0] > 180.0);
        assert!(f64::from(point[0]) < editor.active_outline().bounds().x1);
        assert_eq!(
            editor
                .selection_at(point[0], point[1], SelectionUnit::Grapheme)
                .unwrap()
                .positions[1]
                .paragraph,
            2
        );
        let preview = editor.preview_resize(&mut engine, 36.0).unwrap();
        assert!(preview.bounds().x1 > f64::from(point[0]));
        editor
            .select(
                [TextPosition {
                    paragraph: 2,
                    offset: 0,
                }; 2]
                    .into(),
            )
            .unwrap();
        editor.insert(&mut engine, "!").unwrap();
        assert!(editor.active_outline().bounds().x1 > f64::from(point[0]));
        editor.undo(&mut engine).unwrap();
        assert!(editor.active_outline().bounds().x1 > f64::from(point[0]));
    }

    #[test]
    fn automatic_table_bounds_add_typing_room_only_to_root_text() {
        let mut engine = TextEngine::default();
        let mut source = table_editor(&mut engine).active_outline().snapshot();
        source.layout.width_set_by_user = Some(false);
        let outline = TextOutline::from_outline(&mut engine, &source, &BTreeMap::new()).unwrap();
        assert_eq!(outline.bounds().width(), f64::from(outline.shaped.size[0]));
        source.paragraphs.push(
            crate::document::node(
                Paragraph::new(
                    "A root paragraph needs room to continue typing".into(),
                    Format::default(),
                ),
                Format::default(),
            )
            .unwrap(),
        );
        let outline = TextOutline::from_outline(&mut engine, &source, &BTreeMap::new()).unwrap();
        let paragraph = outline.shaped.paragraphs.last().unwrap();
        let expected = (paragraph.origin[0] + paragraph.text.shaped.width() + 36.0)
            .clamp(72.0, 180.0)
            .max(outline.shaped.size[0]);
        assert_eq!(outline.bounds().width(), f64::from(expected));
        assert!(outline.bounds().width() > f64::from(outline.shaped.size[0]));
    }

    #[test]
    fn cell_mouse_targets_and_paragraph_selection_follow_columns() {
        let mut engine = TextEngine::default();
        let mut editor = table_editor(&mut engine);
        editor
            .select(
                [TextPosition {
                    paragraph: 0,
                    offset: 4,
                }; 2]
                    .into(),
            )
            .unwrap();
        editor.insert(&mut engine, "\nSecond\nThird").unwrap();
        for source in 0..6 {
            let paragraph = editor.active_outline().paragraph_layout(source).unwrap();
            let (_, line) = paragraph.text.lines().next().unwrap();
            let x = paragraph.origin[0] + 1.0;
            let y = paragraph.origin[1] + line.top + line.height * 0.5;
            for unit in [
                SelectionUnit::Grapheme,
                SelectionUnit::Word,
                SelectionUnit::Paragraph,
            ] {
                let selection = editor.selection_at(x, y, unit).unwrap();
                assert_eq!(selection.positions[0].paragraph, source);
                if unit == SelectionUnit::Paragraph {
                    let end = if source < 2 {
                        TextPosition {
                            paragraph: source + 1,
                            offset: 0,
                        }
                    } else {
                        let text = editor
                            .active_outline()
                            .document
                            .paragraphs()
                            .nth(source)
                            .unwrap();
                        TextPosition {
                            paragraph: source,
                            offset: text.utf16_offset(text.text().len()).unwrap(),
                        }
                    };
                    assert_eq!(selection.positions[1], end);
                }
            }
        }
        let right = editor.active_outline().shaped.tables[0].cells[1].rect;
        for x in [right[0] + 0.5, right[2] - 0.5, f32::MAX] {
            let selection = editor
                .selection_at(x, right[3] - 1.0, SelectionUnit::Grapheme)
                .unwrap();
            assert_eq!(selection.positions[1].paragraph, 3);
        }
        let left = editor.active_outline().shaped.tables[0].cells[0].rect;
        let selection = editor
            .selection_at(-f32::MAX, left[3] - 1.0, SelectionUnit::Grapheme)
            .unwrap();
        assert_eq!(selection.positions[1].paragraph, 2);
        let history = editor.undo.len();
        let before = editor.active_outline().document.clone();
        editor
            .select_at(right[0] + 5.0, right[1] + 8.0, false)
            .unwrap();
        editor
            .select_at(left[0] + 5.0, left[3] - 2.0, true)
            .unwrap();
        assert_eq!(editor.selection().positions[0].paragraph, 3);
        assert_eq!(editor.selection().positions[1].paragraph, 2);
        assert!(!editor.selection_rects().unwrap().is_empty());
        assert_eq!(editor.active_outline().document, before);
        assert_eq!(editor.undo.len(), history);
    }

    #[test]
    fn nested_table_mouse_targets_resolve_the_source_cell() {
        use onestore::page::ParagraphContent;
        use onestore::page::text::new_id;
        let mut engine = TextEngine::default();
        let nested = table_editor(&mut engine).active_outline().document.nodes()[0].clone();
        let mut editor = table_editor(&mut engine);
        let mut wrapper = editor.active_outline().document.nodes()[0].clone();
        let ParagraphContent::Table(table) = &mut wrapper.content else {
            panic!()
        };
        table.columns[0].width = 180.0;
        let cell = &mut table.rows[0].cells[0];
        cell.paragraphs.push(nested);
        let mut tail = cell.paragraphs[0].clone();
        tail.id = new_id().unwrap();
        tail.text_mut().unwrap().id = new_id().unwrap();
        cell.paragraphs.push(tail);
        let before = crate::document::node(
            Paragraph::new("Before".into(), Format::default()),
            Format::default(),
        )
        .unwrap();
        let after = crate::document::node(
            Paragraph::new("After".into(), Format::default()),
            Format::default(),
        )
        .unwrap();
        editor
            .commit(
                &mut engine,
                DocumentEdit {
                    columns: BTreeMap::new(),
                    container: None,
                    range: 0..1,
                    replacement: vec![before, wrapper, after],
                },
                [TextPosition {
                    paragraph: 0,
                    offset: 0,
                }; 2]
                    .into(),
            )
            .unwrap();
        assert_eq!(editor.active_outline().document.paragraphs().count(), 11);
        for source in 0..11 {
            let paragraph = editor.active_outline().paragraph_layout(source).unwrap();
            let (_, line) = paragraph.text.lines().next().unwrap();
            let selection = editor
                .selection_at(
                    paragraph.origin[0] + 1.0,
                    paragraph.origin[1] + line.height * 0.5,
                    SelectionUnit::Grapheme,
                )
                .unwrap();
            assert_eq!(selection.positions[0].paragraph, source);
        }
        let paragraph = editor.active_outline().paragraph_layout(1).unwrap();
        let selection = editor
            .selection_at(
                paragraph.origin[0] + 1.0,
                paragraph.origin[1] + 5.0,
                SelectionUnit::Paragraph,
            )
            .unwrap();
        assert_eq!(
            selection.positions[1],
            TextPosition {
                paragraph: 1,
                offset: 4
            }
        );
        let original = editor.active_outline().document.clone();
        editor
            .select(
                [TextPosition {
                    paragraph: 10,
                    offset: 2,
                }; 2]
                    .into(),
            )
            .unwrap();
        assert!(editor.indent(&mut engine, false).unwrap());
        assert_eq!(editor.active_outline().document.nodes()[2].level, 2);
        editor.undo(&mut engine).unwrap();
        assert_eq!(editor.active_outline().document, original);
        for (position, backward) in [
            (
                TextPosition {
                    paragraph: 0,
                    offset: 6,
                },
                false,
            ),
            (
                TextPosition {
                    paragraph: 10,
                    offset: 0,
                },
                true,
            ),
        ] {
            editor.select([position; 2].into()).unwrap();
            assert!(!editor.delete(&mut engine, backward).unwrap());
            assert_eq!(editor.active_outline().document, original);
        }
    }

    #[test]
    fn cell_delete_boundaries_and_local_indentation_preserve_neighbors() {
        let mut engine = TextEngine::default();
        let mut editor = table_editor(&mut engine);
        let original = editor.active_outline().document.clone();
        let history = editor.undo.len();
        for paragraph in 0..4 {
            let text = original.paragraphs().nth(paragraph).unwrap();
            for (backward, offset) in [
                (true, 0),
                (false, text.utf16_offset(text.text().len()).unwrap()),
            ] {
                let selection = [TextPosition { paragraph, offset }; 2].into();
                editor.select(selection).unwrap();
                assert!(!editor.delete(&mut engine, backward).unwrap());
                assert_eq!(editor.active_outline().document, original);
                assert_eq!(editor.selection(), selection);
                assert_eq!(editor.undo.len(), history);
            }
        }
        editor
            .select(
                [TextPosition {
                    paragraph: 2,
                    offset: 2,
                }; 2]
                    .into(),
            )
            .unwrap();
        editor.insert(&mut engine, "\n").unwrap();
        let split = editor.active_outline().document.clone();
        for backward in [true, false] {
            let position = if backward {
                TextPosition {
                    paragraph: 3,
                    offset: 0,
                }
            } else {
                TextPosition {
                    paragraph: 2,
                    offset: 2,
                }
            };
            let selection = [position; 2].into();
            editor.select(selection).unwrap();
            assert!(editor.delete(&mut engine, backward).unwrap());
            assert_eq!(editor.active_outline().document, original);
            editor.undo(&mut engine).unwrap();
            assert_eq!(editor.active_outline().document, split);
            assert_eq!(editor.selection(), selection);
        }
        let selection = [
            TextPosition {
                paragraph: 2,
                offset: 1,
            },
            TextPosition {
                paragraph: 3,
                offset: 0,
            },
        ]
        .into();
        editor.select(selection).unwrap();
        assert!(editor.indent(&mut engine, false).unwrap());
        assert_eq!(
            editor
                .active_outline()
                .document
                .text_nodes()
                .map(|node| node.level)
                .collect::<Vec<_>>(),
            [1, 1, 2, 1, 1]
        );
        assert_eq!(editor.selection(), selection);
        editor.undo(&mut engine).unwrap();
        assert_eq!(editor.active_outline().document, split);
        editor.redo(&mut engine).unwrap();
        assert!(editor.indent(&mut engine, true).unwrap());
        assert_eq!(editor.active_outline().document, split);
    }

    #[test]
    fn overflowing_cell_caret_moves_to_the_next_rows_visual_column() {
        use onestore::page::ParagraphContent;
        let mut engine = TextEngine::default();
        let mut source = table_editor(&mut engine).active_outline().snapshot();
        let ParagraphContent::Table(table) = &mut source.paragraphs[0].content else {
            panic!()
        };
        table.rows[0].cells[0].paragraphs[0]
            .text_mut()
            .unwrap()
            .text = Paragraph::new(
            "M".into(),
            Format {
                font_size: Some(130.0),
                ..Default::default()
            },
        );
        let mut editor =
            CanvasEditor::from_outlines(&mut engine, vec![source], BTreeMap::new()).unwrap();
        let original = editor.active_outline().document.clone();
        editor
            .select(
                [
                    TextPosition {
                        paragraph: 0,
                        offset: 0,
                    },
                    TextPosition {
                        paragraph: 0,
                        offset: 1,
                    },
                ]
                .into(),
            )
            .unwrap();
        let [_, _, right, _] = editor
            .active_outline()
            .shaped
            .paragraph_cell(0)
            .unwrap()
            .text_bounds();
        let selection = editor.selection_rects().unwrap();
        assert!(!selection.is_empty());
        assert!(selection.iter().all(|rect| rect.x1 <= f64::from(right)));
        for extend in [false, true] {
            editor
                .select(
                    [TextPosition {
                        paragraph: 0,
                        offset: 1,
                    }; 2]
                        .into(),
                )
                .unwrap();
            let caret = editor.caret(1.0).unwrap();
            assert!(
                caret.x0 > f64::from(editor.active_outline().shaped.tables[0].cells[0].rect[2])
            );
            editor
                .move_selection(&mut engine, Movement::Down, extend)
                .unwrap();
            assert_eq!(editor.selection().positions[1].paragraph, 3);
            editor
                .move_selection(&mut engine, Movement::Up, extend)
                .unwrap();
            assert_eq!(
                editor.selection().positions[1],
                TextPosition {
                    paragraph: 1,
                    offset: 5
                }
            );
            assert_eq!(editor.active_outline().document, original);
            assert!(editor.undo.is_empty());
        }
    }

    #[test]
    fn table_vertical_arrows_preserve_columns_and_cell_paragraphs() {
        let mut engine = TextEngine::default();
        let mut editor = table_editor(&mut engine);
        let original = editor.active_outline().document.clone();
        for column in 0..2 {
            editor
                .select(
                    [TextPosition {
                        paragraph: column,
                        offset: 2,
                    }; 2]
                        .into(),
                )
                .unwrap();
            editor
                .move_selection(&mut engine, Movement::Down, false)
                .unwrap();
            assert_eq!(editor.selection().positions[1].paragraph, column + 2);
            editor
                .move_selection(&mut engine, Movement::Up, false)
                .unwrap();
            assert_eq!(
                editor.selection().positions[1],
                TextPosition {
                    paragraph: column,
                    offset: 2
                }
            );
        }
        editor
            .select(
                [TextPosition {
                    paragraph: 0,
                    offset: 2,
                }; 2]
                    .into(),
            )
            .unwrap();
        editor.insert(&mut engine, "\n").unwrap();
        editor
            .select(
                [TextPosition {
                    paragraph: 0,
                    offset: 1,
                }; 2]
                    .into(),
            )
            .unwrap();
        editor
            .move_selection(&mut engine, Movement::Down, false)
            .unwrap();
        assert_eq!(editor.selection().positions[1].paragraph, 1);
        editor
            .move_selection(&mut engine, Movement::Down, false)
            .unwrap();
        assert_eq!(editor.selection().positions[1].paragraph, 3);
        editor
            .select(
                [TextPosition {
                    paragraph: 2,
                    offset: 2,
                }; 2]
                    .into(),
            )
            .unwrap();
        editor
            .move_selection(&mut engine, Movement::Down, false)
            .unwrap();
        assert_eq!(editor.selection().positions[1].paragraph, 4);
        editor
            .move_selection(&mut engine, Movement::Up, false)
            .unwrap();
        assert_eq!(
            editor.selection().positions[1],
            TextPosition {
                paragraph: 2,
                offset: 2
            }
        );
        editor.undo(&mut engine).unwrap();
        assert_eq!(editor.active_outline().document, original);
    }

    #[test]
    fn table_edges_create_provisional_space_and_restore_it_with_history() {
        let mut engine = TextEngine::default();
        let mut editor = table_editor(&mut engine);
        editor.active_outline_mut().layout.x = Some(120.0);
        editor.active_outline_mut().layout.y = Some(120.0);
        let source = editor.active_outline().document.clone();
        editor
            .select(
                [TextPosition {
                    paragraph: 3,
                    offset: 4,
                }; 2]
                    .into(),
            )
            .unwrap();
        let history = editor.undo.len();
        editor
            .move_selection(&mut engine, Movement::Down, false)
            .unwrap();
        assert_eq!(editor.outlines()[0].document, source);
        assert_eq!(editor.undo.len(), history);
        assert_eq!(
            editor.selection().positions,
            [TextPosition {
                paragraph: 4,
                offset: 0
            }; 2]
        );
        let draft = editor.active_outline().document.clone();
        let caret = editor.selection();
        editor.insert(&mut engine, "X").unwrap();
        let committed = editor.active_outline().document.clone();
        assert_eq!(committed.nodes().len(), 2);
        assert_eq!(committed.paragraphs().last().unwrap().text(), "X");
        for _ in 0..2 {
            editor.undo(&mut engine).unwrap();
            assert_eq!(editor.outlines()[0].document, source);
            assert_eq!(editor.active_outline().document, draft);
            assert_eq!(editor.selection(), caret);
            editor.redo(&mut engine).unwrap();
            assert_eq!(editor.active_outline().document, committed);
        }
        editor.undo(&mut engine).unwrap();
        editor
            .select(
                [TextPosition {
                    paragraph: 0,
                    offset: 0,
                }; 2]
                    .into(),
            )
            .unwrap();
        editor
            .move_selection(&mut engine, Movement::Up, false)
            .unwrap();
        assert_eq!(editor.caret_outline().unwrap().origin(), [120.0, 86.4]);
        assert_eq!(editor.outlines()[0].document, source);
        editor.insert(&mut engine, "X").unwrap();
        assert_eq!(editor.outlines().len(), 2);
        editor.undo(&mut engine).unwrap();
        assert!(editor.caret_outline().is_some());
        assert_eq!(editor.outlines().len(), 1);
        assert_eq!(editor.outlines()[0].document, source);
    }

    #[test]
    fn editing_a_cell_trims_only_provisional_root_paragraphs() {
        let mut engine = TextEngine::default();
        for compose in [false, true] {
            let mut editor = table_editor(&mut engine);
            let source = editor.active_outline().document.clone();
            editor
                .select(
                    [TextPosition {
                        paragraph: 3,
                        offset: 4,
                    }; 2]
                        .into(),
                )
                .unwrap();
            for _ in 0..2 {
                editor
                    .move_selection(&mut engine, Movement::Down, false)
                    .unwrap();
            }
            for _ in 0..2 {
                editor
                    .move_selection(&mut engine, Movement::Up, false)
                    .unwrap();
            }
            assert_eq!(editor.selection().positions[1].paragraph, 3);
            let draft = editor.active_outline().document.clone();
            assert_eq!(draft.nodes().len(), 3);
            let selection = editor.selection();
            if compose {
                editor
                    .compose(&mut engine, "temporary".into(), 9..9)
                    .unwrap();
                editor.cancel_composition(&mut engine).unwrap();
                assert_eq!(editor.active_outline().document, draft);
                editor.compose(&mut engine, "marked".into(), 6..6).unwrap();
                editor.commit_text(&mut engine, "X".into()).unwrap();
            } else {
                editor.insert(&mut engine, "X").unwrap();
            }
            let after = editor.active_outline().document.clone();
            assert_eq!(after.nodes().len(), 1);
            assert_eq!(after.paragraphs().nth(3).unwrap().text(), "LastX");
            let fresh = editor
                .active_outline()
                .snapshot()
                .layout(&mut engine, &BTreeMap::new())
                .unwrap();
            assert_eq!(editor.active_outline().shaped.size, fresh.size);
            assert_eq!(editor.active_outline().shaped.paragraphs.len(), 4);
            editor.undo(&mut engine).unwrap();
            assert_eq!(editor.outlines()[0].document, source);
            assert_eq!(editor.active_outline().document, draft);
            assert_eq!(editor.selection(), selection);
            editor.redo(&mut engine).unwrap();
            assert_eq!(editor.active_outline().document, after);
        }
    }

    #[test]
    fn editing_during_cell_composition_keeps_table_flow_and_history() {
        let mut engine = TextEngine::default();
        let mut editor = table_editor(&mut engine);
        let source = editor.active_outline().document.clone();
        editor
            .select(
                [TextPosition {
                    paragraph: 3,
                    offset: 4,
                }; 2]
                    .into(),
            )
            .unwrap();
        for _ in 0..2 {
            editor
                .move_selection(&mut engine, Movement::Down, false)
                .unwrap();
        }
        for _ in 0..2 {
            editor
                .move_selection(&mut engine, Movement::Up, false)
                .unwrap();
        }
        let draft = editor.active_outline().document.clone();
        editor.compose(&mut engine, "a".into(), 1..1).unwrap();
        editor.insert(&mut engine, "b").unwrap();
        assert_eq!(
            editor
                .active_outline()
                .document
                .paragraphs()
                .last()
                .unwrap()
                .text(),
            "Lastab"
        );
        let fresh = editor
            .active_outline()
            .snapshot()
            .layout(&mut engine, &BTreeMap::new())
            .unwrap();
        assert_eq!(editor.active_outline().shaped.size, fresh.size);
        assert_eq!(
            editor.active_outline().shaped.paragraphs.len(),
            fresh.paragraphs.len()
        );
        editor.undo(&mut engine).unwrap();
        assert_eq!(
            editor
                .active_outline()
                .document
                .paragraphs()
                .last()
                .unwrap()
                .text(),
            "Lasta"
        );
        editor.undo(&mut engine).unwrap();
        assert_eq!(editor.outlines()[0].document, source);
        assert_eq!(editor.active_outline().document, draft);
    }

    #[test]
    fn table_horizontal_arrows_follow_reading_order() {
        let mut engine = TextEngine::default();
        let mut editor = table_editor(&mut engine);
        let original = editor.active_outline().document.clone();
        let history = editor.undo.len();
        for paragraph in 0..4 {
            let text = original.paragraphs().nth(paragraph).unwrap();
            let end = text.utf16_offset(text.text().len()).unwrap();
            editor
                .select(
                    [TextPosition {
                        paragraph,
                        offset: end,
                    }; 2]
                        .into(),
                )
                .unwrap();
            editor
                .move_selection(&mut engine, Movement::Right, false)
                .unwrap();
            let next = if paragraph < 3 {
                TextPosition {
                    paragraph: paragraph + 1,
                    offset: 0,
                }
            } else {
                TextPosition {
                    paragraph,
                    offset: end,
                }
            };
            assert_eq!(editor.selection().positions, [next; 2]);
            if paragraph < 3 {
                editor
                    .move_selection(&mut engine, Movement::Left, false)
                    .unwrap();
                assert_eq!(
                    editor.selection().positions,
                    [TextPosition {
                        paragraph,
                        offset: end
                    }; 2]
                );
            }
        }
        assert_eq!(editor.active_outline().document, original);
        assert_eq!(editor.undo.len(), history);
        assert!(editor.caret_outline().is_none());
    }

    #[test]
    fn column_width_edits_reflow_and_restore_structured_drafts() {
        use onestore::page::ParagraphContent;
        let mut engine = TextEngine::default();
        for draft in [false, true] {
            let mut editor = table_editor(&mut engine);
            let stored = editor.outlines()[0].document.clone();
            editor
                .select(
                    [TextPosition {
                        paragraph: 3,
                        offset: 4,
                    }; 2]
                        .into(),
                )
                .unwrap();
            if draft {
                for _ in 0..2 {
                    editor
                        .move_selection(&mut engine, Movement::Down, false)
                        .unwrap();
                }
                for _ in 0..2 {
                    editor
                        .move_selection(&mut engine, Movement::Up, false)
                        .unwrap();
                }
            }
            let before = editor.active_outline().document.clone();
            let selection = editor.selection();
            let ParagraphContent::Table(table) = &before.nodes()[0].content else {
                panic!()
            };
            let position = selection.positions[1];
            let mut edit = before
                .replace(
                    position..position,
                    vec![Paragraph::new(
                        " long cell content".into(),
                        Format::default(),
                    )],
                )
                .unwrap();
            edit.columns.insert(table.id, vec![96.375, 36.0]);
            editor.commit(&mut engine, edit, selection).unwrap();
            let after = editor.active_outline().document.clone();
            let fresh = editor
                .active_outline()
                .snapshot()
                .layout(&mut engine, &BTreeMap::new())
                .unwrap();
            assert_eq!(editor.active_outline().shaped.size, fresh.size);
            assert_eq!(
                editor.active_outline().shaped.tables[0]
                    .cells
                    .iter()
                    .map(|c| c.rect)
                    .collect::<Vec<_>>(),
                fresh.tables[0]
                    .cells
                    .iter()
                    .map(|c| c.rect)
                    .collect::<Vec<_>>()
            );
            for _ in 0..2 {
                editor.undo(&mut engine).unwrap();
                assert_eq!(editor.active_outline().document, before);
                assert_eq!(editor.outlines()[0].document, stored);
                assert_eq!(editor.selection(), selection);
                editor.redo(&mut engine).unwrap();
                assert_eq!(editor.active_outline().document, after);
            }
            let selection = editor.selection();
            let history = editor.undo.len();
            for widths in [vec![f32::MAX; 2], vec![36.0, f32::NAN]] {
                let position = selection.positions[1];
                let mut edit = after
                    .replace(
                        position..position,
                        vec![Paragraph::new("invalid".into(), Format::default())],
                    )
                    .unwrap();
                edit.columns.insert(table.id, widths);
                assert!(editor.commit(&mut engine, edit, selection).is_err());
                assert_eq!(editor.active_outline().document, after);
                assert_eq!(editor.selection(), selection);
                assert_eq!(editor.undo.len(), history);
            }
        }
    }

    #[test]
    fn cell_edits_reuse_neighbor_layouts_and_restore_table_geometry() {
        let mut engine = TextEngine::default();
        let mut editor = table_editor(&mut engine);
        let before = editor.active_outline().document.clone();
        let layout_ids = editor
            .active_outline()
            .shaped
            .paragraphs
            .iter()
            .map(|p| p.text.id())
            .collect::<Vec<_>>();
        let size = editor.active_outline().shaped.size;
        let cell_rects = editor.active_outline().shaped.tables[0]
            .cells
            .iter()
            .map(|cell| cell.rect)
            .collect::<Vec<_>>();
        let right_text = editor
            .active_outline()
            .document
            .paragraphs()
            .nth(1)
            .unwrap()
            .text()
            .as_ptr();
        let selection = [TextPosition {
            paragraph: 0,
            offset: 2,
        }; 2]
            .into();
        editor.select(selection).unwrap();
        editor
            .insert(&mut engine, " long text that wraps\nsecond paragraph")
            .unwrap();
        let outline = editor.active_outline();
        assert_eq!(outline.document.paragraphs().count(), 5);
        assert!(outline.shaped.size[1] > size[1]);
        assert_eq!(
            outline
                .document
                .paragraphs()
                .nth(2)
                .unwrap()
                .text()
                .as_ptr(),
            right_text
        );
        for (index, original) in layout_ids.iter().enumerate().skip(1) {
            assert_eq!(
                outline.paragraph_layout(index + 1).unwrap().text.id(),
                *original
            );
        }
        assert_eq!(
            outline
                .layouts()
                .map(|(index, _)| index)
                .collect::<Vec<_>>(),
            [0, 1, 2, 3, 4]
        );
        assert_eq!(
            outline.shaped.tables[0].cells[0].rect[3],
            outline.shaped.tables[0].cells[1].rect[3]
        );
        assert!(outline.shaped.tables[0].cells[2].rect[1] > cell_rects[2][1]);
        let after = outline.document.clone();
        let after_selection = outline.selection;
        let after_rects = outline.shaped.tables[0]
            .cells
            .iter()
            .map(|cell| cell.rect)
            .collect::<Vec<_>>();
        for _ in 0..3 {
            editor.undo(&mut engine).unwrap();
            let outline = editor.active_outline();
            assert_eq!(outline.document, before);
            assert_eq!(outline.selection, selection);
            assert_eq!(outline.shaped.size, size);
            assert_eq!(
                outline.shaped.tables[0]
                    .cells
                    .iter()
                    .map(|cell| cell.rect)
                    .collect::<Vec<_>>(),
                cell_rects
            );
            editor.redo(&mut engine).unwrap();
            let outline = editor.active_outline();
            assert_eq!(outline.document, after);
            assert_eq!(outline.selection, after_selection);
            assert_eq!(
                outline.shaped.tables[0]
                    .cells
                    .iter()
                    .map(|cell| cell.rect)
                    .collect::<Vec<_>>(),
                after_rects
            );
        }
        editor.undo(&mut engine).unwrap();
        editor.undo(&mut engine).unwrap();
        assert_eq!(
            editor
                .active_outline()
                .document
                .paragraphs()
                .next()
                .unwrap()
                .text(),
            "Left"
        );
        assert!(editor.active_outline().shaped.tables.is_empty());
        editor.redo(&mut engine).unwrap();
        assert_eq!(editor.active_outline().document, before);
        assert_eq!(editor.active_outline().shaped.size, size);
    }

    #[test]
    fn cell_composition_and_failed_layout_leave_atomic_history() {
        let mut engine = TextEngine::default();
        let mut editor = table_editor(&mut engine);
        editor
            .select(
                [TextPosition {
                    paragraph: 2,
                    offset: 2,
                }; 2]
                    .into(),
            )
            .unwrap();
        let before = editor.active_outline().document.clone();
        let selection = editor.selection();
        let cells = editor.active_outline().shaped.tables[0]
            .cells
            .iter()
            .map(|cell| cell.rect)
            .collect::<Vec<_>>();
        let ids = editor
            .active_outline()
            .shaped
            .paragraphs
            .iter()
            .map(|p| p.text.id())
            .collect::<Vec<_>>();
        let history = editor.undo.len();
        editor.compose(&mut engine, "🧊\n中".into(), 4..4).unwrap();
        editor.compose(&mut engine, "short".into(), 5..5).unwrap();
        assert!(editor.cancel_composition(&mut engine).unwrap());
        assert_eq!(editor.active_outline().document, before);
        assert_eq!(editor.selection(), selection);
        assert_eq!(editor.undo.len(), history);
        assert_eq!(
            editor.active_outline().shaped.tables[0]
                .cells
                .iter()
                .map(|cell| cell.rect)
                .collect::<Vec<_>>(),
            cells
        );
        for index in [0, 1, 3] {
            assert_eq!(
                editor
                    .active_outline()
                    .paragraph_layout(index)
                    .unwrap()
                    .text
                    .id(),
                ids[index]
            );
        }
        editor.compose(&mut engine, "🧊\n中".into(), 4..4).unwrap();
        editor.commit_text(&mut engine, "done".into()).unwrap();
        assert_eq!(editor.undo.len(), history + 1);
        editor.undo(&mut engine).unwrap();
        assert_eq!(editor.active_outline().document, before);
        assert_eq!(editor.selection(), selection);
        let redo = editor.redo.len();
        assert!(
            editor
                .replace(
                    &mut engine,
                    vec![Paragraph::new(
                        "bad".into(),
                        Format {
                            font_size: Some(f32::NAN),
                            ..Format::default()
                        }
                    )]
                )
                .is_err()
        );
        assert_eq!(editor.active_outline().document, before);
        assert_eq!(editor.selection(), selection);
        assert_eq!(editor.undo.len(), history);
        assert_eq!(editor.redo.len(), redo);
        editor.redo(&mut engine).unwrap();
        assert_eq!(
            editor
                .active_outline()
                .document
                .paragraphs()
                .nth(2)
                .unwrap()
                .text(),
            "Bedonelow"
        );
    }

    #[test]
    fn retained_cell_edits_match_fresh_layout_for_every_text_range() {
        let mut engine = TextEngine::default();
        let base = table_editor(&mut engine);
        for (index, paragraph) in base.active_outline().document.paragraphs().enumerate() {
            let offsets = paragraph
                .text()
                .char_indices()
                .map(|(byte, _)| paragraph.utf16_offset(byte).unwrap())
                .chain(std::iter::once(
                    paragraph.utf16_offset(paragraph.text().len()).unwrap(),
                ))
                .collect::<Vec<_>>();
            for &start in &offsets {
                for &end in offsets.iter().filter(|&&end| end >= start) {
                    for text in ["", "🧊", "x\ny"] {
                        let mut editor = CanvasEditor::from_text_outlines(
                            vec![base.active_outline().clone()],
                            BTreeMap::new(),
                            None,
                        )
                        .unwrap();
                        let selection = [
                            TextPosition {
                                paragraph: index,
                                offset: start,
                            },
                            TextPosition {
                                paragraph: index,
                                offset: end,
                            },
                        ]
                        .into();
                        editor.select(selection).unwrap();
                        editor.insert(&mut engine, text).unwrap();
                        let retained = &editor.active_outline().shaped;
                        let fresh = editor
                            .active_outline()
                            .snapshot()
                            .layout(&mut engine, &editor.definitions)
                            .unwrap();
                        assert_eq!(retained.size, fresh.size);
                        assert_eq!(
                            retained.tables[0]
                                .cells
                                .iter()
                                .map(|c| c.rect)
                                .collect::<Vec<_>>(),
                            fresh.tables[0]
                                .cells
                                .iter()
                                .map(|c| c.rect)
                                .collect::<Vec<_>>()
                        );
                        for (retained, fresh) in retained.paragraphs.iter().zip(&fresh.paragraphs) {
                            assert_eq!(retained.id, fresh.id);
                            assert_eq!(retained.origin, fresh.origin);
                            assert_eq!(retained.text.height(), fresh.text.height());
                            assert_eq!(
                                retained
                                    .text
                                    .lines()
                                    .map(|(line, bounds)| (
                                        line.metrics().advance,
                                        bounds.source.clone()
                                    ))
                                    .collect::<Vec<_>>(),
                                fresh
                                    .text
                                    .lines()
                                    .map(|(line, bounds)| (
                                        line.metrics().advance,
                                        bounds.source.clone()
                                    ))
                                    .collect::<Vec<_>>()
                            );
                        }
                        editor.undo(&mut engine).unwrap();
                        assert_eq!(
                            editor.active_outline().document,
                            base.active_outline().document
                        );
                        assert_eq!(editor.selection(), selection);
                    }
                }
            }
        }
    }

    #[test]
    fn cross_outline_validation_includes_table_row_and_cell_ids() {
        use onestore::page::ParagraphContent;
        let mut engine = TextEngine::default();
        let first = table_editor(&mut engine).active_outline().clone();
        let second = table_editor(&mut engine).active_outline().clone();
        let ParagraphContent::Table(first_table) = &first.document.nodes()[0].content else {
            panic!()
        };
        for alias in 0..3 {
            let mut second = second.clone();
            let mut nodes = second.document.nodes().to_vec();
            let ParagraphContent::Table(table) = &mut nodes[0].content else {
                panic!()
            };
            match alias {
                0 => table.id = first_table.id,
                1 => table.rows[0].id = first_table.rows[0].id,
                _ => table.rows[0].cells[0].id = first_table.rows[0].cells[0].id,
            }
            second.document = TextDocument::from_nodes(nodes).unwrap();
            assert!(matches!(
                CanvasEditor::from_text_outlines(
                    vec![first.clone(), second],
                    BTreeMap::new(),
                    None
                ),
                Err(EditorError::Edit(EditError::InvalidStructure))
            ));
        }
    }

    #[test]
    fn pictures_move_and_resize_through_history_and_backgrounds_stay_put() {
        use onestore::page::{Image, Page, PageObject};
        let mut engine = TextEngine::default();
        let [picture, background] = [false, true].map(|background| Image {
            size: None,
            id: onestore::page::text::new_id().unwrap(),
            layout: onestore::document::Layout {
                x: Some(468.75),
                y: Some(86.4),
                max_width: Some(333.0),
                max_height: Some(200.1),
                ..Default::default()
            },
            bytes: Some(std::sync::Arc::from(b"deferred image payload".as_slice())),
            alt: None,
            background,
        });
        let (id, background_id) = (picture.id, background.id);
        let mut editor = CanvasEditor::from_page(
            Page {
                title: String::new(),
                identity: None,
                created: None,
                margin_origin: [36.0, 14.4],
                definitions: BTreeMap::new(),
                objects: vec![PageObject::Image(background), PageObject::Image(picture)],
            },
            &mut engine,
        )
        .unwrap();
        let stored = |editor: &CanvasEditor| {
            editor
                .page()
                .unwrap()
                .objects
                .into_iter()
                .find_map(|object| match object {
                    PageObject::Image(image) if image.id == id => Some(image.layout),
                    _ => None,
                })
                .unwrap()
        };
        let original = stored(&editor);
        assert_eq!(
            editor.image_placement(id),
            Some(([468.75, 86.4], [333.0, 200.1]))
        );
        assert!(editor.image_placement(background_id).is_none());
        assert!(matches!(
            editor.place_image(&mut engine, background_id, [0.0; 2], [10.0; 2]),
            Err(EditorError::Edit(EditError::InvalidRange))
        ));
        assert!(matches!(
            editor.place_image(&mut engine, id, [0.0; 2], [0.0, 10.0]),
            Err(EditorError::Edit(EditError::InvalidRange))
        ));
        editor
            .place_image(&mut engine, id, [513.75, 104.4], [333.0, 200.1])
            .unwrap();
        let moved = stored(&editor);
        assert_eq!([moved.x, moved.y], [Some(513.75), Some(104.4)]);
        assert_eq!(moved.width_set_by_user, None);
        editor
            .place_image(&mut engine, id, [513.75, 104.4], [281.8, 169.4])
            .unwrap();
        let resized = stored(&editor);
        assert_eq!(
            [resized.max_width, resized.max_height],
            [Some(281.8), Some(169.4)]
        );
        assert_eq!(resized.width_set_by_user, Some(true));
        assert!(editor.undo(&mut engine).unwrap());
        assert_eq!(stored(&editor), moved);
        assert!(editor.undo(&mut engine).unwrap());
        assert_eq!(stored(&editor), original);
        assert!(editor.redo(&mut engine).unwrap());
        assert!(editor.redo(&mut engine).unwrap());
        assert_eq!(stored(&editor), resized);

        let ids = |editor: &CanvasEditor| {
            editor
                .page()
                .unwrap()
                .objects
                .iter()
                .map(PageObject::id)
                .collect::<Vec<_>>()
        };
        let before = ids(&editor);
        assert!(matches!(
            editor.remove_image(&mut engine, background_id),
            Err(EditorError::Edit(EditError::InvalidRange))
        ));
        editor.remove_image(&mut engine, id).unwrap();
        assert!(editor.image_placement(id).is_none());
        assert_eq!(ids(&editor), [background_id]);
        assert!(editor.undo(&mut engine).unwrap());
        assert_eq!(ids(&editor), before);
        assert_eq!(stored(&editor), resized);
        assert!(editor.redo(&mut engine).unwrap());
        assert_eq!(ids(&editor), [background_id]);
    }

    #[test]
    fn arrows_leave_a_picture_for_the_neighbouring_outlines_in_page_order() {
        use onestore::page::{Image, Page, PageObject};
        let mut engine = TextEngine::default();
        let outline = |engine: &mut TextEngine, text: &str, x| {
            PageObject::Outline(
                TextOutline::new(
                    engine,
                    TextDocument::new(vec![Paragraph::new(text.into(), Default::default())])
                        .unwrap(),
                    120.0,
                    [x, 0.0],
                )
                .unwrap()
                .snapshot(),
            )
        };
        let picture = Image {
            size: None,
            id: onestore::page::text::new_id().unwrap(),
            layout: onestore::document::Layout {
                x: Some(200.0),
                y: Some(0.0),
                max_width: Some(40.0),
                max_height: Some(40.0),
                ..Default::default()
            },
            bytes: Some(std::sync::Arc::from(b"deferred image payload".as_slice())),
            alt: None,
            background: false,
        };
        let id = picture.id;
        let objects = vec![
            outline(&mut engine, "Before", 0.0),
            PageObject::Image(picture),
            outline(&mut engine, "After", 300.0),
        ];
        let [before, after] = [0, 2].map(|index| objects[index].id());
        let mut editor = CanvasEditor::from_page(
            Page {
                title: String::new(),
                identity: None,
                created: None,
                margin_origin: [36.0, 14.4],
                definitions: BTreeMap::new(),
                objects,
            },
            &mut engine,
        )
        .unwrap();
        let caret = |paragraph, offset| [TextPosition { paragraph, offset }; 2];
        assert!(editor.step_from_image(&mut engine, id, false).unwrap());
        assert_eq!(editor.active_outline().id, before);
        assert_eq!(editor.selection().positions, caret(0, 6));
        assert!(editor.step_from_image(&mut engine, id, true).unwrap());
        assert_eq!(editor.active_outline().id, after);
        assert_eq!(editor.selection().positions, caret(0, 0));

        editor.select_all().unwrap();
        assert!(editor.delete(&mut engine, true).unwrap());
        editor.focus_outline(before).unwrap();
        assert!(editor.outlines().iter().all(|outline| outline.id != after));
        assert!(!editor.step_from_image(&mut engine, id, true).unwrap());
        assert_eq!(editor.active_outline().id, before);
    }

    #[test]
    fn text_edits_flow_around_a_picture_inside_an_outline() {
        use onestore::page::{Image, Page, PageObject, ParagraphContent};
        let mut engine = TextEngine::default();
        let mut source = TextOutline::new(
            &mut engine,
            TextDocument::new(vec![
                Paragraph::new("Before".into(), Default::default()),
                Paragraph::new("After".into(), Default::default()),
            ])
            .unwrap(),
            240.0,
            [36.0, 36.0],
        )
        .unwrap()
        .snapshot();
        let mut picture = source.paragraphs[0].clone();
        picture.id = onestore::page::text::new_id().unwrap();
        let image = Image {
            size: Some([40.0, 30.0]),
            id: onestore::page::text::new_id().unwrap(),
            layout: Default::default(),
            bytes: Some(std::sync::Arc::from(b"deferred image payload".as_slice())),
            alt: None,
            background: false,
        };
        picture.content = ParagraphContent::Image(image.clone());
        source.paragraphs.insert(1, picture);
        let outline_id = source.id;
        let mut editor = CanvasEditor::from_page(
            Page {
                title: String::new(),
                identity: None,
                created: None,
                margin_origin: [36.0, 14.4],
                definitions: BTreeMap::new(),
                objects: vec![PageObject::Outline(source)],
            },
            &mut engine,
        )
        .unwrap();
        let outline = editor.active_outline();
        assert_eq!(outline.id, outline_id);
        let [before, after] = [0, 1].map(|i| outline.shaped().paragraphs[i].origin[1]);
        let object = &outline.shaped().objects[0];
        assert_eq!(object.id, image.id);
        assert_eq!(object.rect[3] - object.rect[1], 30.0);
        assert!(before < object.rect[1] && object.bottom <= after);

        editor
            .move_selection(&mut engine, Movement::Down, false)
            .unwrap();
        assert_eq!(editor.selection().positions[0].paragraph, 1);
        editor
            .move_selection(&mut engine, Movement::Up, false)
            .unwrap();
        editor
            .move_selection(&mut engine, Movement::LineEnd, false)
            .unwrap();
        editor.insert(&mut engine, "\nMore").unwrap();
        let shaped = editor.active_outline().shaped();
        assert_eq!(shaped.paragraphs.len(), 3);
        assert!(shaped.objects[0].rect[1] > shaped.paragraphs[1].origin[1]);
        assert!(shaped.paragraphs[2].origin[1] >= shaped.objects[0].bottom);
        // Joining across the picture is refused rather than deleting it.
        editor
            .move_selection(&mut engine, Movement::Down, false)
            .unwrap();
        editor
            .move_selection(&mut engine, Movement::LineStart, false)
            .unwrap();
        assert!(!editor.delete(&mut engine, true).unwrap());
        let saved = editor.page().unwrap();
        let PageObject::Outline(saved) = &saved.objects[0] else {
            panic!()
        };
        assert!(matches!(&saved.paragraphs[2].content, ParagraphContent::Image(i) if *i == image));
        assert!(editor.undo(&mut engine).unwrap());
        assert_eq!(editor.active_outline().shaped().paragraphs.len(), 2);
        assert_eq!(editor.active_outline().shaped().objects.len(), 1);

        // Selected, it resizes and deletes in place through the outline's history.
        assert!(editor.image_in_outline(image.id));
        let (origin, size) = editor.image_placement(image.id).unwrap();
        assert_eq!(size, [40.0, 30.0]);
        editor
            .place_image(&mut engine, image.id, [0.0; 2], [80.0, 60.0])
            .unwrap();
        assert_eq!(
            editor.image_placement(image.id),
            Some((origin, [80.0, 60.0]))
        );
        let shaped = editor.active_outline().shaped();
        assert!(shaped.paragraphs[1].origin[1] >= shaped.objects[0].bottom);
        assert!(editor.step_from_image(&mut engine, image.id, true).unwrap());
        assert_eq!(
            editor.selection().positions[0],
            TextPosition {
                paragraph: 1,
                offset: 0
            }
        );
        assert!(
            editor
                .step_from_image(&mut engine, image.id, false)
                .unwrap()
        );
        assert_eq!(
            editor.selection().positions[0],
            TextPosition {
                paragraph: 0,
                offset: 6
            }
        );
        editor.remove_image(&mut engine, image.id).unwrap();
        assert!(editor.image_placement(image.id).is_none());
        assert!(editor.active_outline().shaped().objects.is_empty());
        assert!(editor.undo(&mut engine).unwrap());
        assert_eq!(
            editor.image_placement(image.id),
            Some((origin, [80.0, 60.0]))
        );
        assert!(editor.undo(&mut engine).unwrap());
        assert_eq!(
            editor.image_placement(image.id),
            Some((origin, [40.0, 30.0]))
        );
    }

    #[test]
    fn unknown_paragraphs_and_text_free_outlines_stay_on_the_page() {
        use onestore::page::{Image, Page, PageObject, ParagraphContent, Unsupported};
        let mut engine = TextEngine::default();
        let outline = |engine: &mut TextEngine| {
            TextOutline::new(
                engine,
                TextDocument::new(vec![
                    Paragraph::new("Before".into(), Default::default()),
                    Paragraph::new("After".into(), Default::default()),
                ])
                .unwrap(),
                240.0,
                [36.0, 36.0],
            )
            .unwrap()
            .snapshot()
        };
        let mut mixed = outline(&mut engine);
        let mut unknown = mixed.paragraphs[0].clone();
        unknown.id = onestore::page::text::new_id().unwrap();
        unknown.content = ParagraphContent::Unsupported(Unsupported {
            id: onestore::page::text::new_id().unwrap(),
            jcid: 0x60012,
            layout: Default::default(),
        });
        mixed.paragraphs.insert(1, unknown);
        let mut picture_only = outline(&mut engine);
        picture_only.id = onestore::page::text::new_id().unwrap();
        picture_only.paragraphs.truncate(1);
        picture_only.paragraphs[0].content = ParagraphContent::Image(Image {
            size: Some([40.0, 30.0]),
            id: onestore::page::text::new_id().unwrap(),
            layout: Default::default(),
            bytes: Some(std::sync::Arc::from(b"deferred image payload".as_slice())),
            alt: None,
            background: false,
        });
        let (mixed_id, picture_id) = (mixed.id, picture_only.id);
        let editor = CanvasEditor::from_page(
            Page {
                title: String::new(),
                identity: None,
                created: None,
                margin_origin: [36.0, 14.4],
                definitions: BTreeMap::new(),
                objects: vec![
                    PageObject::Outline(mixed),
                    PageObject::Outline(picture_only),
                ],
            },
            &mut engine,
        )
        .unwrap();
        let mixed = editor.outlines().iter().find(|o| o.id == mixed_id).unwrap();
        let [placeholder] = &mixed.shaped().objects[..] else {
            panic!()
        };
        assert!(matches!(
            placeholder.kind,
            crate::outline::ObjectKind::Unsupported(_)
        ));
        assert_eq!(placeholder.rect[2] - placeholder.rect[0], 160.0);
        assert!(mixed.shaped().paragraphs[1].origin[1] >= placeholder.bottom);
        assert!(editor.objects.iter().any(|object| matches!(
            object,
            page::Content::Outline { source, .. } if source.id == picture_id
        )));
        assert_eq!(editor.page().unwrap().objects.len(), 2);
    }

    #[test]
    fn title_flow_moves_page_objects_atomically_and_cancels_composition() {
        use onestore::page::{Image, Page, PageObject, Title};
        let mut engine = TextEngine::default();
        let mut title = TextOutline::new(
            &mut engine,
            TextDocument::new(vec![Paragraph::new("Header".into(), Default::default())]).unwrap(),
            468.0,
            [0.0; 2],
        )
        .unwrap()
        .snapshot();
        title.title = true;
        title.layout.max_width = None;
        title.layout.width_set_by_user = None;
        let body = TextOutline::new(
            &mut engine,
            TextDocument::new(vec![Paragraph::new("Anchor".into(), Default::default())]).unwrap(),
            72.0,
            [120.0, 80.0],
        )
        .unwrap()
        .snapshot();
        let body_id = body.id;
        let images = [
            (120.0, 90.0, false),
            (600.0, 150.0, false),
            (120.0, 0.0, false),
            (120.0, 80.0, true),
        ]
        .map(|(x, y, background)| Image {
            size: None,
            id: onestore::page::text::new_id().unwrap(),
            layout: onestore::document::Layout {
                x: Some(x),
                y: Some(y),
                max_width: Some(72.0),
                max_height: Some(40.0),
                ..Default::default()
            },
            bytes: Some(std::sync::Arc::from(b"deferred image payload".as_slice())),
            alt: None,
            background,
        });
        let image_ids = images.each_ref().map(|image| image.id);
        let image_bytes = images
            .each_ref()
            .map(|image| image.bytes.as_ref().unwrap().clone());
        let mut objects = vec![
            PageObject::Title(Title {
                id: onestore::page::text::new_id().unwrap(),
                date: None,
                layout: Default::default(),
                outlines: vec![title],
            }),
            PageObject::Outline(body),
        ];
        objects.extend(images.into_iter().map(PageObject::Image));
        let mut editor = CanvasEditor::from_page(
            Page {
                title: "Header".into(),
                identity: None,
                created: None,
                margin_origin: [36.0, 14.4],
                definitions: BTreeMap::new(),
                objects,
            },
            &mut engine,
        )
        .unwrap();
        let positions = |editor: &CanvasEditor| {
            editor
                .object_layouts()
                .map(|(id, layout)| (id, layout.clone()))
                .collect::<Vec<_>>()
        };
        let original = positions(&editor);
        let text = "Header\u{000b}Second\u{000b}Third\u{000b}Fourth\u{000b}Fifth\u{000b}Sixth";
        let end = text.encode_utf16().count().try_into().unwrap();
        editor.select_all().unwrap();
        editor.compose(&mut engine, text.into(), end..end).unwrap();
        let moved = positions(&editor);
        let y = |items: &Vec<(ExGuid, onestore::document::Layout)>, id| {
            items
                .iter()
                .find(|(candidate, _)| *candidate == id)
                .unwrap()
                .1
                .y
                .unwrap()
        };
        let delta = y(&moved, body_id) - y(&original, body_id);
        assert!(delta > 0.0);
        for id in &image_ids[..2] {
            assert!((y(&moved, *id) - y(&original, *id) - delta).abs() < 0.001);
        }
        for id in &image_ids[2..] {
            assert_eq!(y(&moved, *id), y(&original, *id));
        }
        editor.compose(&mut engine, "Header".into(), 6..6).unwrap();
        assert_eq!(positions(&editor), moved);
        editor.cancel_composition(&mut engine).unwrap();
        assert_eq!(positions(&editor), original);
        assert!(!editor.undo(&mut engine).unwrap());
        editor.select_all().unwrap();
        editor.compose(&mut engine, text.into(), end..end).unwrap();
        let taller = format!("{text}\u{000b}Seventh\u{000b}Eighth");
        let taller_end = taller.encode_utf16().count().try_into().unwrap();
        editor
            .compose(&mut engine, taller, taller_end..taller_end)
            .unwrap();
        let farther = positions(&editor);
        assert!(y(&farther, body_id) > y(&moved, body_id));
        editor.commit_text(&mut engine, "Header".into()).unwrap();
        assert_eq!(positions(&editor), farther);
        editor.undo(&mut engine).unwrap();
        assert_eq!(positions(&editor), original);
        assert!(!editor.undo(&mut engine).unwrap());
        editor.redo(&mut engine).unwrap();
        assert_eq!(positions(&editor), farther);
        editor.undo(&mut engine).unwrap();
        editor.select_all().unwrap();
        editor.insert(&mut engine, text).unwrap();
        assert_eq!(positions(&editor), moved);
        editor.select_all().unwrap();
        editor.insert(&mut engine, "Header").unwrap();
        assert_eq!(positions(&editor), moved);
        editor.undo(&mut engine).unwrap();
        assert_eq!(positions(&editor), moved);
        editor.undo(&mut engine).unwrap();
        assert_eq!(positions(&editor), original);
        editor.redo(&mut engine).unwrap();
        editor.redo(&mut engine).unwrap();
        assert_eq!(positions(&editor), moved);
        for (id, bytes) in image_ids.into_iter().zip(image_bytes) {
            let image = editor
                .objects
                .iter()
                .find_map(|object| match object {
                    page::Content::Image(image) if image.id == id => Some(image),
                    _ => None,
                })
                .unwrap();
            assert!(std::sync::Arc::ptr_eq(
                image.bytes.as_ref().unwrap(),
                &bytes
            ));
        }
    }

    #[test]
    fn floating_outline_exit_uses_document_position_and_keeps_real_blank_lines() {
        let mut engine = TextEngine::default();
        for (x, real_blanks, size, exit_y) in [
            (179.9, 0, 11.0, None),
            (180.0, 0, 11.0, Some(230.4)),
            (270.0, 1, 11.0, Some(230.4)),
            (270.0, 0, 22.0, Some(248.4)),
            (270.0, 0, 44.0, Some(320.4)),
        ] {
            let mut paragraphs = vec![Paragraph::new(
                "Text".into(),
                Format {
                    font_size: Some(size),
                    ..Default::default()
                },
            )];
            paragraphs
                .extend((0..real_blanks).map(|_| Paragraph::new(String::new(), Format::default())));
            let source = TextDocument::new(paragraphs).unwrap();
            let outline = TextOutline::new(&mut engine, source.clone(), 72.0, [x, 158.4]).unwrap();
            let mut editor =
                CanvasEditor::from_text_outlines(vec![outline], BTreeMap::new(), None).unwrap();
            editor
                .move_selection(&mut engine, Movement::DocumentEnd, false)
                .unwrap();
            for _ in 0..3 - real_blanks {
                editor
                    .move_selection(&mut engine, Movement::Down, false)
                    .unwrap();
            }
            assert_eq!(editor.outlines[0].document, source);
            assert_eq!(editor.caret_outline().is_some(), exit_y.is_some());
            if let Some(y) = exit_y {
                assert_eq!(editor.active_outline().origin(), [x, y]);
                assert_eq!(
                    editor.active_outline().layout.max_width,
                    Some(DEFAULT_OUTLINE_WIDTH)
                );
                editor.insert(&mut engine, "New").unwrap();
                assert_eq!(editor.outlines.len(), 2);
                editor.undo(&mut engine).unwrap();
                assert_eq!(editor.outlines.len(), 1);
                assert_eq!(editor.active_outline().origin(), [x, y]);
            } else {
                assert_eq!(editor.active_outline().document.nodes().len(), 4);
            }
            assert!(editor.undo.is_empty());
        }
    }

    #[test]
    fn vertical_exit_and_reentry_preserve_the_text_column_and_source() {
        let mut engine = TextEngine::default();
        for (x, y, above) in [
            (36.0, 158.4, 122.4),
            (180.0, 158.4, 122.4),
            (180.0, 178.0, 140.4),
            (180.0, 180.0, 140.4),
            (180.0, 184.0, 158.4),
        ] {
            let source =
                TextDocument::new(vec![Paragraph::new("Text".into(), Format::default())]).unwrap();
            let outline = TextOutline::new(&mut engine, source.clone(), 72.0, [x, y]).unwrap();
            let id = outline.id;
            let mut editor =
                CanvasEditor::from_text_outlines(vec![outline], BTreeMap::new(), None).unwrap();
            editor
                .move_selection(&mut engine, Movement::DocumentEnd, false)
                .unwrap();
            let column = editor.caret(1.0).unwrap().x0 as f32;
            editor
                .move_selection(&mut engine, Movement::Up, false)
                .unwrap();
            assert_eq!(
                editor.caret_outline().unwrap().origin(),
                [x + column, above]
            );
            editor.focus_outline(id).unwrap();
            if x >= 180.0 {
                for _ in 0..3 {
                    editor
                        .move_selection(&mut engine, Movement::Down, false)
                        .unwrap();
                }
                assert!(editor.caret_outline().is_some());
                editor
                    .move_selection(&mut engine, Movement::Up, false)
                    .unwrap();
                assert!(editor.caret_outline().is_some());
                editor
                    .move_selection(&mut engine, Movement::Up, false)
                    .unwrap();
                if y == 184.0 {
                    assert!(editor.caret_outline().is_some());
                    editor
                        .move_selection(&mut engine, Movement::Up, false)
                        .unwrap();
                }
                assert_eq!(editor.active_outline().id, id);
                assert_eq!(editor.selection().positions[1].offset, 4);
            }
            assert_eq!(editor.outlines[0].document, source);
            assert!(editor.undo.is_empty());
        }
    }

    #[test]
    fn provisional_paragraphs_use_the_base_style_instead_of_local_run_formatting() {
        let mut engine = TextEngine::default();
        let base = Format {
            font: Some("Arial".into()),
            font_size: Some(11.0),
            ..Default::default()
        };
        let style = ExGuid {
            n: 9,
            ..Default::default()
        };
        let mut nodes = TextDocument::new(vec![Paragraph::new(
            "Text".into(),
            Format {
                font_size: Some(22.0),
                bold: Some(true),
                ..base.clone()
            },
        )])
        .unwrap()
        .nodes()
        .to_vec();
        nodes[0].style = Some(style);
        let outline = TextOutline::new(
            &mut engine,
            TextDocument::from_nodes(nodes).unwrap(),
            240.0,
            [0.0; 2],
        )
        .unwrap();
        let mut editor = CanvasEditor::from_text_outlines(
            vec![outline],
            BTreeMap::from([(
                style,
                Definition {
                    kind: onestore::document::Kind::Style {
                        name: Some("p".into()),
                    },
                    format: base.clone(),
                },
            )]),
            None,
        )
        .unwrap();
        editor
            .move_selection(&mut engine, Movement::Down, false)
            .unwrap();
        let paragraph = &editor.active_outline().document.nodes()[1];
        assert_eq!(paragraph.style, Some(style));
        assert_eq!(paragraph.text().unwrap().text.spans()[0].format, base);
        assert_eq!(
            editor.active_outline().shaped.paragraphs[1].text.height(),
            engine
                .layout(&Paragraph::new(String::new(), base), 240.0)
                .unwrap()
                .height()
        );
        assert_eq!(editor.outlines[0].document.nodes().len(), 1);
    }

    #[test]
    fn clicks_below_use_base_style_preserve_source_and_share_draft_history() {
        let mut engine = TextEngine::default();
        for font_size in [11.0, 22.0] {
            let style = ExGuid {
                n: 900,
                ..Default::default()
            };
            let base = Format {
                font: Some("Arial".into()),
                font_size: Some(font_size),
                ..Default::default()
            };
            let mut nodes = TextDocument::new(vec![Paragraph::new(
                "Body".into(),
                Format {
                    font_size: Some(44.0),
                    bold: Some(true),
                    ..base.clone()
                },
            )])
            .unwrap()
            .nodes()
            .to_vec();
            nodes[0].style = Some(style);
            let outline = TextOutline::new(
                &mut engine,
                TextDocument::from_nodes(nodes).unwrap(),
                240.0,
                [180.0, 90.0],
            )
            .unwrap();
            let id = outline.id;
            let source = outline.document.clone();
            let layout = outline.layout.clone();
            let text_id = outline.shaped.paragraphs[0].text.id();
            let bottom = outline.shaped.size[1];
            let mut editor = CanvasEditor::from_text_outlines(
                vec![outline],
                BTreeMap::from([(
                    style,
                    Definition {
                        kind: onestore::document::Kind::Style {
                            name: Some("p".into()),
                        },
                        format: base.clone(),
                    },
                )]),
                None,
            )
            .unwrap();
            assert!(
                editor
                    .select_below(&mut engine, id, [10.0, bottom + 20.0])
                    .unwrap()
            );
            let blanks = if font_size == 11.0 { 2 } else { 1 };
            assert_eq!(editor.active_outline().document.nodes().len(), blanks + 1);
            assert_eq!(
                editor.selection().positions[1],
                TextPosition {
                    paragraph: blanks,
                    offset: 0
                }
            );
            assert_eq!(editor.outlines[0].document, source);
            assert_eq!(
                editor.active_outline().shaped.paragraphs[0].text.id(),
                text_id
            );
            for paragraph in &editor.active_outline().document.nodes()[1..] {
                assert_eq!(paragraph.style, Some(style));
                assert_eq!(paragraph.text().unwrap().text.spans()[0].format, base);
            }
            let pending = editor.active_outline().document.clone();
            let selection = editor.selection();
            assert!(
                !editor
                    .select_below(&mut engine, id, [10.0, bottom + 28.0])
                    .unwrap()
            );
            assert_eq!(editor.active_outline().document, pending);
            assert_eq!(editor.selection(), selection);
            assert!(editor.undo.is_empty());
            editor.compose(&mut engine, "日本".into(), 2..2).unwrap();
            assert!(editor.cancel_composition(&mut engine).unwrap());
            assert_eq!(editor.active_outline().document, pending);
            editor.insert(&mut engine, "X").unwrap();
            let edited = editor.active_outline().document.clone();
            assert_eq!(editor.outlines[0].layout, layout);
            assert_eq!(edited.nodes()[0], source.nodes()[0]);
            assert!(editor.undo(&mut engine).unwrap());
            assert_eq!(editor.active_outline().document, pending);
            assert_eq!(editor.outlines[0].document, source);
            assert_eq!(editor.selection(), selection);
            assert!(editor.redo(&mut engine).unwrap());
            assert_eq!(editor.active_outline().document, edited);
            assert!(editor.undo(&mut engine).unwrap());
            assert!(
                editor
                    .select_below(&mut engine, id, [10.0, bottom + 1.0])
                    .unwrap()
            );
            assert_eq!(editor.active_outline().document.nodes().len(), blanks + 1);
            editor
                .select([selection.positions[1], editor.selection().positions[1]].into())
                .unwrap();
            assert_eq!(editor.outlines[0].document, source);
            assert_eq!(editor.redo.len(), 1);
            editor
                .place_caret(&mut engine, [600.0, 600.0], 240.0)
                .unwrap();
            let pending = editor.active_outline().document.clone();
            let selection = editor.selection();
            editor.definitions.get_mut(&style).unwrap().format.font_size = Some(0.001);
            assert!(
                editor
                    .select_below(&mut engine, id, [10.0, bottom + 20.0])
                    .is_err()
            );
            assert_eq!(editor.active_outline().document, pending);
            assert_eq!(editor.selection(), selection);
            assert_eq!(editor.outlines[0].document, source);
            assert_eq!(editor.redo.len(), 1);
        }
    }

    #[test]
    fn editing_during_provisional_composition_keeps_only_committed_layout_height() {
        let mut engine = TextEngine::default();
        let source =
            TextDocument::new(vec![Paragraph::new("body".into(), Format::default())]).unwrap();
        let mut editor = CanvasEditor::new(&mut engine, source.clone(), 240.0).unwrap();
        for _ in 0..3 {
            editor
                .move_selection(&mut engine, Movement::Down, false)
                .unwrap();
        }
        for _ in 0..2 {
            editor
                .move_selection(&mut engine, Movement::Up, false)
                .unwrap();
        }
        editor.compose(&mut engine, "a".into(), 1..1).unwrap();
        editor.insert(&mut engine, "b").unwrap();
        let expected = TextOutline::from_outline(
            &mut engine,
            &editor.active_outline().snapshot(),
            &BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(editor.active_outline().shaped.size, expected.shaped.size);
        assert_eq!(editor.active_outline().document.nodes().len(), 2);
        editor.undo(&mut engine).unwrap();
        assert_eq!(
            editor
                .active_outline()
                .document
                .paragraphs()
                .last()
                .unwrap()
                .text(),
            "a"
        );
        editor.undo(&mut engine).unwrap();
        assert_eq!(editor.outlines[0].document, source);
        assert_eq!(editor.active_outline().document.nodes().len(), 4);
    }

    #[test]
    fn arrow_created_paragraphs_are_provisional_and_share_existing_shaping() {
        let mut engine = TextEngine::default();
        let source =
            TextDocument::new(vec![Paragraph::new("body".into(), Format::default())]).unwrap();
        let mut editor = CanvasEditor::new(&mut engine, source.clone(), 240.0).unwrap();
        let shaped = editor.outlines[0].shaped.paragraphs[0]
            .text
            .shaped
            .styles()
            .as_ptr();
        for _ in 0..3 {
            editor
                .move_selection(&mut engine, Movement::Down, false)
                .unwrap();
        }
        assert_eq!(editor.outlines[0].document, source);
        assert_eq!(editor.active_outline().document.nodes().len(), 4);
        assert_eq!(
            editor
                .visible_outlines()
                .next()
                .unwrap()
                .document
                .nodes()
                .len(),
            4
        );
        assert_eq!(
            editor.active_outline().shaped.paragraphs[0]
                .text
                .shaped
                .styles()
                .as_ptr(),
            shaped
        );
        assert!(editor.undo.is_empty());
        editor
            .move_selection(&mut engine, Movement::Up, false)
            .unwrap();
        editor
            .move_selection(&mut engine, Movement::Up, false)
            .unwrap();
        let pending = editor.active_outline().document.clone();
        let selection = editor.selection();
        editor.insert(&mut engine, "X").unwrap();
        assert_eq!(
            editor.outlines[0]
                .document
                .paragraphs()
                .map(Paragraph::text)
                .collect::<Vec<_>>(),
            ["body", "X"]
        );
        assert_eq!(
            editor.outlines[0].shaped.paragraphs[0]
                .text
                .shaped
                .styles()
                .as_ptr(),
            shaped
        );
        editor.undo(&mut engine).unwrap();
        assert_eq!(editor.outlines[0].document, source);
        assert_eq!(editor.active_outline().document, pending);
        assert_eq!(editor.selection(), selection);
        editor
            .place_caret(&mut engine, [500.0, 500.0], 200.0)
            .unwrap();
        editor.redo(&mut engine).unwrap();
        assert_eq!(
            editor.outlines[0]
                .document
                .paragraphs()
                .map(Paragraph::text)
                .collect::<Vec<_>>(),
            ["body", "X"]
        );
        editor.undo(&mut engine).unwrap();
        assert_eq!(editor.active_outline().document, pending);
        assert_eq!(editor.selection(), selection);
        editor
            .place_caret(&mut engine, [500.0, 500.0], 200.0)
            .unwrap();
        assert_eq!(editor.outlines[0].document, source);
    }

    #[test]
    fn deleting_through_provisional_paragraphs_removes_the_outline_and_restores_draft_on_undo() {
        let mut engine = TextEngine::default();
        let source =
            TextDocument::new(vec![Paragraph::new("body".into(), Format::default())]).unwrap();
        let mut editor = CanvasEditor::new(&mut engine, source.clone(), 240.0).unwrap();
        for _ in 0..2 {
            editor
                .move_selection(&mut engine, Movement::Down, false)
                .unwrap();
        }
        let pending = editor.active_outline().document.clone();
        editor.select_all().unwrap();
        let selection = editor.selection();
        editor.delete(&mut engine, true).unwrap();
        assert!(editor.outlines.is_empty());
        assert!(editor.caret_outline().is_some());
        editor
            .place_caret(&mut engine, [500.0, 500.0], 200.0)
            .unwrap();
        for _ in 0..2 {
            editor.undo(&mut engine).unwrap();
            assert_eq!(editor.outlines[0].document, source);
            assert_eq!(editor.active_outline().document, pending);
            assert_eq!(editor.selection(), selection);
            editor.redo(&mut engine).unwrap();
            assert!(editor.outlines.is_empty());
        }
    }

    #[test]
    fn provisional_composition_cancels_and_backspace_reenters_without_history() {
        let mut engine = TextEngine::default();
        let source =
            TextDocument::new(vec![Paragraph::new("body".into(), Format::default())]).unwrap();
        let mut editor = CanvasEditor::new(&mut engine, source.clone(), 240.0).unwrap();
        editor
            .move_selection(&mut engine, Movement::Down, false)
            .unwrap();
        let pending = editor.active_outline().document.clone();
        editor.compose(&mut engine, "日本".into(), 2..2).unwrap();
        assert_eq!(editor.outlines[0].document, source);
        editor.cancel_composition(&mut engine).unwrap();
        assert_eq!(editor.active_outline().document, pending);
        editor.compose(&mut engine, "日本".into(), 2..2).unwrap();
        editor.finish_composition();
        assert_eq!(
            editor.outlines[0]
                .document
                .paragraphs()
                .map(Paragraph::text)
                .collect::<Vec<_>>(),
            ["body", "日本"]
        );
        editor.undo(&mut engine).unwrap();
        assert_eq!(editor.active_outline().document, pending);
        editor.delete(&mut engine, true).unwrap();
        assert_eq!(editor.outlines[0].document, source);
        assert!(matches!(editor.active, Focus::Outline(0)));
        assert!(editor.undo.is_empty());
        assert!(editor.redo(&mut engine).unwrap());
    }

    #[test]
    fn word_navigation_terminates_at_soft_wrapped_bidi_cycles() {
        let mut engine = TextEngine::default();
        let paragraph = Paragraph::from_runs(vec![
            (
                "العربية".into(),
                Format {
                    font_size: Some(12.0),
                    bold: Some(false),
                    italic: Some(false),
                    ..Default::default()
                },
            ),
            ("ية 日本".into(), Format::default()),
        ]);
        let source = TextDocument::new(vec![paragraph]).unwrap();
        let mut editor = CanvasEditor::new(&mut engine, source.clone(), 1.0).unwrap();
        let selection = Selection {
            positions: [TextPosition {
                paragraph: 0,
                offset: 7,
            }; 2],
            affinities: [Affinity::Upstream; 2],
        };
        editor.select(selection).unwrap();
        editor.delete_to(&mut engine, Movement::WordRight).unwrap();
        assert_eq!(
            editor
                .active_outline()
                .document
                .paragraphs()
                .next()
                .unwrap()
                .text(),
            "ية 日本"
        );
        editor.undo(&mut engine).unwrap();
        assert_eq!(editor.active_outline().document, source);
        assert_eq!(editor.selection(), selection);
        for offset in 0..=12 {
            for affinity in [Affinity::Upstream, Affinity::Downstream] {
                for movement in [Movement::WordLeft, Movement::WordRight] {
                    editor
                        .select(Selection {
                            positions: [TextPosition {
                                paragraph: 0,
                                offset,
                            }; 2],
                            affinities: [affinity; 2],
                        })
                        .unwrap();
                    editor.move_selection(&mut engine, movement, true).unwrap();
                    editor.select(editor.selection()).unwrap();
                }
            }
        }
    }

    #[test]
    fn indentation_reflows_and_split_join_preserves_levels_and_undo() {
        let mut engine = TextEngine::default();
        let source = TextDocument::new(vec![
            Paragraph::new("alpha beta gamma delta".into(), Format::default()),
            Paragraph::new("untouched".into(), Format::default()),
        ])
        .unwrap();
        let mut editor = CanvasEditor::new(&mut engine, source.clone(), 110.0).unwrap();
        let untouched = editor.active_outline().shaped.paragraphs[1]
            .text
            .shaped
            .styles()
            .as_ptr();
        let selection: Selection = [TextPosition {
            paragraph: 0,
            offset: 6,
        }; 2]
            .into();
        editor.select(selection).unwrap();
        editor.indent(&mut engine, false).unwrap();
        assert_eq!(editor.active_outline().document.nodes()[0].level, 2);
        assert_eq!(editor.active_outline().shaped.paragraphs[0].origin[0], 27.0);
        assert_eq!(
            editor.active_outline().shaped.paragraphs[1]
                .text
                .shaped
                .styles()
                .as_ptr(),
            untouched
        );
        editor.insert(&mut engine, "\n").unwrap();
        assert_eq!(
            editor
                .active_outline()
                .document
                .nodes()
                .iter()
                .map(|n| n.level)
                .collect::<Vec<_>>(),
            [2, 2, 1]
        );
        // Backspace outdents an indented paragraph before joining it (`c4-child-1.xml`).
        editor.delete(&mut engine, true).unwrap();
        assert_eq!(editor.active_outline().document.nodes()[1].level, 1);
        editor.delete(&mut engine, true).unwrap();
        assert_eq!(
            editor.active_outline().document.nodes()[0]
                .text()
                .unwrap()
                .text
                .text(),
            "alpha beta gamma delta"
        );
        for _ in 0..4 {
            editor.undo(&mut engine).unwrap();
        }
        assert_eq!(editor.active_outline().document, source);
        assert_eq!(editor.selection(), selection);
        assert!(!editor.indent(&mut engine, true).unwrap());
        assert!(editor.redo(&mut engine).unwrap());
    }

    #[test]
    fn indentation_excludes_selected_next_paragraph_start_and_rejects_overflow_atomically() {
        let mut engine = TextEngine::default();
        let source = TextDocument::new(vec![
            Paragraph::new("first".into(), Format::default()),
            Paragraph::new("second".into(), Format::default()),
        ])
        .unwrap();
        let mut editor = CanvasEditor::new(&mut engine, source, 50.0).unwrap();
        let selection: Selection = [
            TextPosition {
                paragraph: 1,
                offset: 0,
            },
            TextPosition {
                paragraph: 0,
                offset: 0,
            },
        ]
        .into();
        editor.select(selection).unwrap();
        editor.indent(&mut engine, false).unwrap();
        let indented = editor.active_outline().document.clone();
        assert_eq!(
            indented.nodes().iter().map(|n| n.level).collect::<Vec<_>>(),
            [2, 1]
        );
        assert!(editor.indent(&mut engine, false).is_err());
        assert_eq!(editor.active_outline().document, indented);
        assert_eq!(editor.selection(), selection);
        editor.undo(&mut engine).unwrap();
        assert!(!editor.undo(&mut engine).unwrap());
    }

    #[test]
    fn paragraph_and_document_navigation_preserve_selection_anchor() {
        let mut engine = TextEngine::default();
        let source = TextDocument::new(vec![
            Paragraph::new("one two three four five".into(), Format::default()),
            Paragraph::new("🌳 last".into(), Format::default()),
        ])
        .unwrap();
        let mut editor = CanvasEditor::new(&mut engine, source.clone(), 45.0).unwrap();
        let start = TextPosition {
            paragraph: 0,
            offset: 8,
        };
        editor.select([start; 2].into()).unwrap();
        editor
            .move_selection(&mut engine, Movement::ParagraphEnd, true)
            .unwrap();
        assert_eq!(
            editor.selection().positions,
            [
                start,
                TextPosition {
                    paragraph: 0,
                    offset: 23
                }
            ]
        );
        editor
            .move_selection(&mut engine, Movement::ParagraphEnd, true)
            .unwrap();
        assert_eq!(
            editor.selection().positions,
            [
                start,
                TextPosition {
                    paragraph: 1,
                    offset: 7
                }
            ]
        );
        editor
            .move_selection(&mut engine, Movement::DocumentStart, true)
            .unwrap();
        assert_eq!(
            editor.selection().positions,
            [
                start,
                TextPosition {
                    paragraph: 0,
                    offset: 0
                }
            ]
        );
        editor
            .move_selection(&mut engine, Movement::DocumentEnd, false)
            .unwrap();
        assert_eq!(
            editor.selection().positions,
            [TextPosition {
                paragraph: 1,
                offset: 7
            }; 2]
        );
        editor
            .move_selection(&mut engine, Movement::ParagraphStart, false)
            .unwrap();
        assert_eq!(
            editor.selection().positions,
            [TextPosition {
                paragraph: 1,
                offset: 0
            }; 2]
        );
        editor
            .move_selection(&mut engine, Movement::ParagraphStart, false)
            .unwrap();
        assert_eq!(
            editor.selection().positions,
            [TextPosition {
                paragraph: 0,
                offset: 0
            }; 2]
        );
        assert_eq!(editor.active_outline().document, source);
        assert!(!editor.undo(&mut engine).unwrap());
    }

    #[test]
    fn word_deletion_restores_original_caret_and_styled_source_on_undo() {
        let mut engine = TextEngine::default();
        let source = TextDocument::new(vec![Paragraph::new(
            "first café 🌳 last".into(),
            Format {
                bold: Some(true),
                ..Default::default()
            },
        )])
        .unwrap();
        let mut editor = CanvasEditor::new(&mut engine, source.clone(), 200.0).unwrap();
        let selection: Selection = [TextPosition {
            paragraph: 0,
            offset: 10,
        }; 2]
            .into();
        editor.select(selection).unwrap();
        assert!(editor.delete_to(&mut engine, Movement::WordLeft).unwrap());
        assert_eq!(
            editor
                .active_outline()
                .document
                .paragraphs()
                .next()
                .unwrap()
                .text(),
            "first  🌳 last"
        );
        editor.undo(&mut engine).unwrap();
        assert_eq!(editor.active_outline().document, source);
        assert_eq!(editor.selection(), selection);
        editor.redo(&mut engine).unwrap();
        editor.undo(&mut engine).unwrap();
        editor
            .move_selection(&mut engine, Movement::DocumentStart, false)
            .unwrap();
        assert!(!editor.delete_to(&mut engine, Movement::WordLeft).unwrap());
        assert!(editor.redo(&mut engine).unwrap());
    }

    #[test]
    fn horizontal_navigation_and_blank_grid_keep_documents_and_history_unchanged() {
        let mut engine = TextEngine::default();
        let mut outlines = Vec::new();
        for (text, origin) in [
            ("Solo", [36.0, 158.4]),
            ("Right", [504.0, 158.4]),
            ("Below", [504.0, 320.4]),
        ] {
            let document =
                TextDocument::new(vec![Paragraph::new(text.into(), Format::default())]).unwrap();
            outlines.push(TextOutline::new(&mut engine, document, 72.0, origin).unwrap());
        }
        let ids: Vec<_> = outlines.iter().map(|outline| outline.id).collect();
        let originals: Vec<_> = outlines
            .iter()
            .map(|outline| outline.document.clone())
            .collect();
        let mut editor = CanvasEditor::from_text_outlines(outlines, BTreeMap::new(), None).unwrap();
        editor
            .move_selection(&mut engine, Movement::LineEnd, false)
            .unwrap();
        editor
            .move_selection(&mut engine, Movement::Right, true)
            .unwrap();
        assert_eq!(editor.active_outline().id, ids[0]);
        editor
            .move_selection(&mut engine, Movement::Right, false)
            .unwrap();
        assert_eq!(editor.active_outline().id, ids[1]);
        assert_eq!(editor.selection().positions[1].offset, 0);
        editor
            .move_selection(&mut engine, Movement::Left, false)
            .unwrap();
        assert_eq!(editor.active_outline().id, ids[0]);
        assert_eq!(editor.selection().positions[1].offset, 4);
        editor.focus_outline(ids[1]).unwrap();
        editor
            .move_selection(&mut engine, Movement::LineEnd, false)
            .unwrap();
        editor
            .move_selection(&mut engine, Movement::Right, false)
            .unwrap();
        assert_eq!(editor.active_outline().id, ids[1]);
        assert_eq!(editor.selection().positions[1].offset, 5);
        editor
            .place_caret(&mut engine, [486.0, 194.4], 468.0)
            .unwrap();
        editor
            .move_selection(&mut engine, Movement::Right, false)
            .unwrap();
        assert_eq!(editor.active_outline().origin(), [504.0, 194.4]);
        editor
            .move_selection(&mut engine, Movement::Up, false)
            .unwrap();
        assert_eq!(editor.active_outline().id, ids[1]);
        assert_eq!(editor.selection().positions[1].offset, 0);
        assert!(
            editor
                .outlines
                .iter()
                .zip(&originals)
                .all(|(outline, original)| &outline.document == original)
        );
        assert!(!editor.undo(&mut engine).unwrap());
    }

    #[test]
    fn word_and_paragraph_selection_use_source_ranges_across_wraps() {
        let mut engine = TextEngine::default();
        let document = TextDocument::new(vec![Paragraph::new(
            "alpha beta gamma 🌳 delta".into(),
            Format::default(),
        )])
        .unwrap();
        let mut editor = CanvasEditor::new(&mut engine, document, 70.0).unwrap();
        let point = TextPosition {
            paragraph: 0,
            offset: 7,
        };
        editor.select([point; 2].into()).unwrap();
        let caret = editor.caret(1.0).unwrap();
        let x = caret.x0 as f32;
        let y = ((caret.y0 + caret.y1) * 0.5) as f32;
        let word = editor.selection_at(x, y, SelectionUnit::Word).unwrap();
        assert_eq!(word.positions.map(|p| p.offset), [6, 10]);
        let paragraph = editor.selection_at(x, y, SelectionUnit::Paragraph).unwrap();
        assert_eq!(paragraph.positions.map(|p| p.offset), [0, 25]);
        assert_eq!(editor.selection().positions, [point; 2]);
    }

    #[test]
    fn pointer_selection_uses_canvas_line_boxes_after_many_emoji_lines() {
        let text = "alpha 🌳 beta\n".repeat(30);
        let start = text[..text.rfind("beta").unwrap()].encode_utf16().count() as u32;
        let mut engine = TextEngine::default();
        let document = TextDocument::new(vec![Paragraph::new(text, Format::default())]).unwrap();
        let mut editor = CanvasEditor::new(&mut engine, document, 70.0).unwrap();
        let position = TextPosition {
            paragraph: 0,
            offset: start + 1,
        };
        editor.select([position; 2].into()).unwrap();
        let caret = editor.caret(1.0).unwrap();
        let point = [caret.x0 as f32, ((caret.y0 + caret.y1) * 0.5) as f32];
        assert_eq!(
            editor
                .selection_at(point[0], point[1], SelectionUnit::Word)
                .unwrap()
                .positions
                .map(|p| p.offset),
            [start, start + 4]
        );
        assert_eq!(
            editor
                .selection_at(point[0], point[1], SelectionUnit::Grapheme)
                .unwrap()
                .positions,
            [position; 2]
        );
    }

    #[test]
    fn retiring_text_restores_source_selection_order_and_geometry_through_history() {
        let mut engine = TextEngine::default();
        let mut outlines = Vec::new();
        for text in ["before", "bold 🌳\nannotation", "after"] {
            let document = TextDocument::new(
                text.split('\n')
                    .map(|line| {
                        Paragraph::new(
                            line.into(),
                            Format {
                                bold: Some(true),
                                ..Default::default()
                            },
                        )
                    })
                    .collect(),
            )
            .unwrap();
            outlines.push(TextOutline::new(&mut engine, document, 123.0, [43.5, 230.4]).unwrap());
        }
        let id = outlines[1].id;
        let document = outlines[1].document.clone();
        let geometry = layout_snapshot(&outlines[1].shaped);
        let layout = outlines[1].layout.clone();
        let ids = outlines
            .iter()
            .map(|outline| outline.id)
            .collect::<Vec<_>>();
        let mut editor = CanvasEditor::from_text_outlines(outlines, BTreeMap::new(), None).unwrap();
        editor.focus_outline(id).unwrap();
        editor.select_all().unwrap();
        let selection = editor.selection();
        editor.delete(&mut engine, true).unwrap();
        assert_eq!(
            editor.outlines.iter().map(|o| o.id).collect::<Vec<_>>(),
            [ids[0], ids[2]]
        );
        assert_eq!(editor.caret_outline().unwrap().id, id);
        assert_eq!(editor.active_outline().layout, layout);
        editor.insert(&mut engine, "again").unwrap();
        assert_eq!(
            editor.outlines.iter().map(|o| o.id).collect::<Vec<_>>(),
            ids
        );
        assert_eq!(editor.active_outline().id, id);
        editor.undo(&mut engine).unwrap();
        assert_eq!(editor.caret_outline().unwrap().id, id);
        editor
            .place_caret(&mut engine, [900.0, 500.0], 77.0)
            .unwrap();
        editor.undo(&mut engine).unwrap();
        assert_eq!(editor.active_outline().document, document);
        assert_eq!(editor.active_outline().layout, layout);
        assert_eq!(editor.selection(), selection);
        assert_eq!(layout_snapshot(&editor.active_outline().shaped), geometry);
        assert_eq!(
            editor.outlines.iter().map(|o| o.id).collect::<Vec<_>>(),
            ids
        );
        editor.redo(&mut engine).unwrap();
        assert_eq!(editor.outlines.len(), 2);
        assert_eq!(editor.active_outline().id, id);
        assert_eq!(editor.active_outline().origin(), [43.5, 230.4]);
        editor.redo(&mut engine).unwrap();
        assert_eq!(editor.active_outline().id, id);
        assert_eq!(
            editor.outlines.iter().map(|o| o.id).collect::<Vec<_>>(),
            ids
        );
        assert_eq!(
            editor.outlines[0]
                .document
                .paragraphs()
                .next()
                .unwrap()
                .text(),
            "before"
        );
        assert_eq!(
            editor.outlines[2]
                .document
                .paragraphs()
                .next()
                .unwrap()
                .text(),
            "after"
        );
    }

    #[test]
    fn empty_composition_commits_safely_when_focus_or_position_changes() {
        let mut engine = TextEngine::default();
        for action in 0..5 {
            let mut outlines = Vec::new();
            for text in ["delete", "keep"] {
                let document =
                    TextDocument::new(vec![Paragraph::new(text.into(), Format::default())])
                        .unwrap();
                outlines.push(TextOutline::new(&mut engine, document, 240.0, [0.0; 2]).unwrap());
            }
            let first = outlines[0].id;
            let second = outlines[1].id;
            let original = outlines[0].document.clone();
            let mut editor =
                CanvasEditor::from_text_outlines(outlines, BTreeMap::new(), None).unwrap();
            editor.select_all().unwrap();
            editor.compose(&mut engine, String::new(), 0..0).unwrap();
            assert_eq!(editor.outlines.len(), 2);
            editor.cancel_composition(&mut engine).unwrap();
            assert_eq!(editor.active_outline().document, original);
            editor.compose(&mut engine, String::new(), 0..0).unwrap();
            match action {
                0 => editor.focus_outline(first).unwrap(),
                1 => editor.focus_outline(second).unwrap(),
                2 => editor.move_outline(first, [30.0, 40.0]).unwrap(),
                3 => editor.move_outline(second, [30.0, 40.0]).unwrap(),
                _ => editor.commit_text(&mut engine, String::new()).unwrap(),
            }
            assert!(editor.marked_range().is_none());
            assert_eq!(editor.outlines.len(), 1);
            assert_eq!(editor.outlines[0].id, second);
            assert_eq!(
                editor.outlines[0]
                    .document
                    .paragraphs()
                    .next()
                    .unwrap()
                    .text(),
                "keep"
            );
            if action == 3 {
                editor.undo(&mut engine).unwrap();
            }
            editor.undo(&mut engine).unwrap();
            assert_eq!(editor.active_outline().id, first);
            assert_eq!(editor.active_outline().document, original);
            assert_eq!(editor.outlines.len(), 2);
            editor.redo(&mut engine).unwrap();
            assert_eq!(editor.outlines.len(), 1);
        }
        let document =
            TextDocument::new(vec![Paragraph::new(String::new(), Format::default())]).unwrap();
        let mut editor = CanvasEditor::new(&mut engine, document.clone(), 240.0).unwrap();
        editor.compose(&mut engine, "preedit".into(), 0..7).unwrap();
        editor.delete(&mut engine, true).unwrap();
        assert_eq!(editor.undo.len(), 2);
        assert!(editor.outlines.is_empty());
        editor.undo(&mut engine).unwrap();
        assert_eq!(
            editor
                .active_outline()
                .document
                .paragraphs()
                .next()
                .unwrap()
                .text(),
            "preedit"
        );
        editor.undo(&mut engine).unwrap();
        assert_eq!(editor.active_outline().document, document);
        assert!(editor.outlines.is_empty());
    }

    #[test]
    fn space_only_caret_preserves_spacing_but_blank_paragraphs_and_metadata_are_content() {
        let mut engine = TextEngine::default();
        let document =
            TextDocument::new(vec![Paragraph::new(String::new(), Format::default())]).unwrap();
        let mut editor = CanvasEditor::new(&mut engine, document, 240.0).unwrap();
        editor.insert(&mut engine, "   ").unwrap();
        assert!(editor.outlines.is_empty());
        assert!(editor.undo.is_empty());
        let spaces = editor.active_outline().document.clone();
        let selection = editor.selection();
        editor.insert(&mut engine, "x").unwrap();
        assert_eq!(editor.outlines.len(), 1);
        editor.delete(&mut engine, true).unwrap();
        assert!(editor.outlines.is_empty());
        assert_eq!(editor.active_outline().document, spaces);
        let undo_count = editor.undo.len();
        editor.select_all().unwrap();
        assert!(editor.delete_to(&mut engine, Movement::LineStart).unwrap());
        assert!(editor.outlines.is_empty());
        assert_eq!(editor.undo.len(), undo_count);
        assert!(editor.undo(&mut engine).unwrap());
        assert_eq!(
            editor
                .active_outline()
                .document
                .paragraphs()
                .next()
                .unwrap()
                .text(),
            "   x"
        );
        assert!(editor.redo(&mut engine).unwrap());
        assert_eq!(editor.active_outline().document, spaces);
        for _ in 0..2 {
            editor.undo(&mut engine).unwrap();
        }
        assert!(editor.outlines.is_empty());
        assert_eq!(editor.active_outline().document, spaces);
        assert_eq!(editor.selection(), selection);
        editor.insert(&mut engine, "\n").unwrap();
        assert_eq!(editor.outlines.len(), 1);
        assert_eq!(editor.active_outline().document.nodes().len(), 2);
        for text in ["\t", "\u{a0}", "\u{200b}"] {
            editor.select_all().unwrap();
            editor.insert(&mut engine, text).unwrap();
            assert_eq!(editor.outlines.len(), 1);
        }
        let mut source = editor.active_outline().snapshot();
        source.paragraphs[0].collapsed = true;
        let mut editor =
            CanvasEditor::from_outlines(&mut engine, vec![source], BTreeMap::new()).unwrap();
        editor.select_all().unwrap();
        editor.delete(&mut engine, true).unwrap();
        assert_eq!(editor.outlines.len(), 1);
        assert!(editor.active_outline().document.nodes()[0].collapsed);
    }

    #[test]
    fn caret_placement_is_not_document_history_and_first_input_is_one_undo() {
        let mut engine = TextEngine::default();
        let original = TextDocument::new(vec![Paragraph::new(
            "Existing notes".into(),
            Format::default(),
        )])
        .unwrap();
        let mut editor = CanvasEditor::new(&mut engine, original.clone(), 240.0).unwrap();
        let existing = editor.active_outline().id;
        for step in 0..100 {
            editor
                .place_caret(&mut engine, [step as f32, 100.0], 120.0)
                .unwrap();
            assert_eq!(editor.outlines.len(), 1);
            assert!(editor.undo.is_empty());
            assert!(editor.redo.is_empty());
        }
        let id = editor.active_outline().id;
        let empty = editor.active_outline().document.clone();
        editor
            .insert(&mut engine, "annotation\nsecond line")
            .unwrap();
        let document = editor.active_outline().document.clone();
        assert_eq!(editor.active_outline().id, id);
        assert!(editor.caret_outline().is_none());
        assert_eq!(editor.outlines.len(), 2);
        assert_eq!(editor.undo.len(), 1);
        editor.undo(&mut engine).unwrap();
        assert_eq!(editor.outlines.len(), 1);
        assert_eq!(editor.active_outline().document, empty);
        assert_eq!(editor.active_outline().origin(), [99.0, 100.0]);
        editor
            .place_caret(&mut engine, [600.0, 400.0], 180.0)
            .unwrap();
        editor.insert(&mut engine, "").unwrap();
        assert_eq!(editor.redo.len(), 1);
        editor.redo(&mut engine).unwrap();
        assert_eq!(editor.active_outline().id, id);
        assert_eq!(editor.active_outline().document, document);
        assert_eq!(editor.active_outline().origin(), [99.0, 100.0]);
        editor.focus_outline(existing).unwrap();
        assert_eq!(editor.active_outline().document, original);
    }

    #[test]
    fn provisional_input_preserves_the_empty_document_and_redo_until_commit() {
        let mut engine = TextEngine::default();
        let document = TextDocument::new(vec![Paragraph::new(
            String::new(),
            Format {
                bold: Some(true),
                ..Default::default()
            },
        )])
        .unwrap();
        let mut editor = CanvasEditor::new(&mut engine, document.clone(), 240.0).unwrap();
        assert!(editor.outlines.is_empty());
        editor.insert(&mut engine, "first").unwrap();
        editor.undo(&mut engine).unwrap();
        let id = editor.active_outline().id;
        editor.compose(&mut engine, "に".into(), 1..1).unwrap();
        assert!(editor.outlines.is_empty());
        assert_eq!(editor.redo.len(), 1);
        assert!(
            editor
                .place_caret(&mut engine, [f32::NAN, 0.0], 240.0)
                .is_err()
        );
        assert_eq!(editor.active_outline().id, id);
        assert!(editor.marked_range().is_some());
        editor.cancel_composition(&mut engine).unwrap();
        assert_eq!(editor.active_outline().document, document);
        assert_eq!(editor.redo.len(), 1);
        editor.compose(&mut engine, "に".into(), 1..1).unwrap();
        editor.commit_text(&mut engine, "日本".into()).unwrap();
        assert_eq!(editor.outlines.len(), 1);
        assert_eq!(editor.active_outline().id, id);
        assert_eq!(editor.undo.len(), 1);
        assert!(editor.redo.is_empty());
        editor.undo(&mut engine).unwrap();
        assert!(editor.outlines.is_empty());
        assert_eq!(editor.active_outline().document, document);
    }

    fn ys(outline: &TextOutline) -> Vec<f32> {
        outline
            .shaped
            .paragraphs
            .iter()
            .map(|paragraph| paragraph.origin[1])
            .collect()
    }

    fn layout_snapshot(layout: &OutlineLayout) -> serde_json::Value {
        serde_json::json!({
            "size": layout.size,
            "paragraphs": layout.paragraphs.iter().map(|paragraph| serde_json::json!({
                "id": paragraph.id, "origin": paragraph.origin,
                "text": paragraph.projection.text().text(),
                "lines": paragraph.text.lines().map(|(line, bounds)| serde_json::json!({
                    "source": bounds.source, "top": bounds.top, "baseline": bounds.baseline,
                    "height": bounds.height, "advance": line.metrics().advance,
                })).collect::<Vec<_>>(),
                "markers": paragraph.markers.iter().map(|(text, origin)| serde_json::json!({
                    "origin": origin, "height": text.height(),
                    "baseline": text.lines().next().unwrap().1.baseline,
                    "advance": text.lines().next().unwrap().0.metrics().advance,
                })).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
        })
    }

    #[test]
    fn native_outline_omits_outer_paragraph_spacing_through_split_and_undo() {
        use onestore::page::{Page, PageObject};
        use onestore::{RevisionIndex, Store, document::Document};
        let store = Store::parse(include_bytes!(
            "../../../corpus/canvas/baseline-anchors.one"
        ))
        .unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let page =
            Page::from_document(&Document::parse(&index).unwrap(), "Baseline anchors").unwrap();
        let outline = page
            .objects
            .into_iter()
            .find_map(|object| match object {
                PageObject::Outline(outline)
                    if outline.paragraphs[0].text().unwrap().text.text() == "Spacing first" =>
                {
                    Some(outline)
                }
                _ => None,
            })
            .unwrap();
        let mut engine = TextEngine::default();
        let mut editor =
            CanvasEditor::from_outlines(&mut engine, vec![outline], page.definitions).unwrap();
        let source = editor.active_outline().document.nodes().to_vec();
        for step in 0..3 {
            let outline = editor.active_outline();
            let paragraphs = &outline.shaped.paragraphs;
            assert_eq!(paragraphs[0].origin[1], 0.0);
            for pair in paragraphs.windows(2) {
                assert!(
                    (pair[1].origin[1] - pair[0].origin[1] - pair[0].text.height() - 144.0).abs()
                        < 0.0001
                );
            }
            let height = paragraphs.iter().map(|p| p.text.height()).sum::<f32>()
                + (paragraphs.len() - 1) as f32 * 144.0;
            assert!((outline.shaped.size[1] - height).abs() < 0.0001);
            match step {
                0 => editor.insert(&mut engine, "\n").unwrap(),
                1 => assert!(editor.undo(&mut engine).unwrap()),
                _ => assert_eq!(outline.document.nodes(), source),
            }
        }
    }

    #[test]
    fn resize_preview_preserves_history_and_commit_restores_automatic_width_on_undo() {
        let mut engine = TextEngine::default();
        let mut editor = CanvasEditor::new(
            &mut engine,
            TextDocument::new(vec![Paragraph::new(String::new(), Format::default())]).unwrap(),
            468.0,
        )
        .unwrap();
        editor.insert(&mut engine, "One").unwrap();
        let source = editor.active_outline().snapshot();
        let selection = editor.selection();
        editor.insert(&mut engine, "!").unwrap();
        editor.undo(&mut engine).unwrap();
        for width in [120.0, 360.0, 180.0] {
            let preview = editor.preview_resize(&mut engine, width).unwrap();
            assert_eq!(preview.bounds().width(), f64::from(width));
            assert_eq!(preview.document, editor.active_outline().document);
            assert_eq!(editor.active_outline().layout, source.layout);
            assert_eq!(editor.undo.len(), 1);
            assert_eq!(editor.redo.len(), 1);
        }
        assert!(editor.preview_resize(&mut engine, f32::NAN).is_err());
        editor.resize(&mut engine, 180.0).unwrap();
        assert!(editor.redo.is_empty());
        assert_eq!(editor.undo.len(), 2);
        editor.undo(&mut engine).unwrap();
        assert_eq!(editor.active_outline().layout, source.layout);
        assert_eq!(editor.active_outline().bounds().width(), 72.0);
        assert_eq!(editor.selection(), selection);
        editor.redo(&mut engine).unwrap();
        assert_eq!(editor.active_outline().bounds().width(), 180.0);
        editor.resize(&mut engine, 180.0).unwrap();
        assert_eq!(editor.undo.len(), 2);
    }

    #[test]
    fn new_empty_outline_grows_shrinks_and_keeps_a_manually_set_width() {
        let mut engine = TextEngine::default();
        let mut editor = CanvasEditor::new(
            &mut engine,
            TextDocument::new(vec![Paragraph::new(String::new(), Format::default())]).unwrap(),
            468.0,
        )
        .unwrap();
        assert_eq!(editor.active_outline().bounds().width(), 72.0);
        assert_eq!(
            editor.active_outline().layout.width_set_by_user,
            Some(false)
        );
        editor
            .insert(&mut engine, "A short sentence expands the container")
            .unwrap();
        let expanded = editor.active_outline().bounds().width();
        assert!(expanded > 72.0 && expanded < 468.0);
        editor.select_all().unwrap();
        editor.insert(&mut engine, &"words ".repeat(80)).unwrap();
        assert_eq!(editor.active_outline().bounds().width(), 468.0);
        assert!(
            editor
                .active_outline()
                .paragraph_layout(0)
                .unwrap()
                .text
                .lines()
                .count()
                > 1
        );
        editor.select_all().unwrap();
        editor.insert(&mut engine, "One").unwrap();
        assert_eq!(editor.active_outline().bounds().width(), 72.0);
        editor.resize(&mut engine, 180.0).unwrap();
        editor.select_all().unwrap();
        editor.insert(&mut engine, "Tiny").unwrap();
        assert_eq!(editor.active_outline().bounds().width(), 180.0);
        editor
            .insert(&mut engine, &" longer words".repeat(10))
            .unwrap();
        assert_eq!(editor.active_outline().bounds().width(), 180.0);
        editor.undo(&mut engine).unwrap();
        assert_eq!(editor.active_outline().bounds().width(), 180.0);
    }

    #[test]
    fn resizing_an_automatic_outline_sets_its_explicit_width_and_preserves_spacing_metadata() {
        let document = TextDocument::new(vec![Paragraph::new(
            "short".into(),
            Format {
                space_after: Some(7.0),
                ..Format::default()
            },
        )])
        .unwrap();
        let source = document.nodes().to_vec();
        let outline = Outline {
            title: false,
            min_width: None,
            id: onestore::page::text::new_id().unwrap(),
            layout: onestore::document::Layout {
                max_width: Some(220.0),
                reserved_width: Some(180.0),
                width_set_by_user: Some(false),
                ..Default::default()
            },
            indents: vec![18.0, 0.0],
            paragraphs: source.clone(),
            unsupported: Vec::new(),
        };
        let mut engine = TextEngine::default();
        let mut editor =
            CanvasEditor::from_outlines(&mut engine, vec![outline], BTreeMap::new()).unwrap();
        assert!(editor.active_outline().bounds().width() < 180.0);
        let height = editor
            .active_outline()
            .paragraph_layout(0)
            .unwrap()
            .text
            .height();
        assert_eq!(editor.active_outline().bounds().height(), f64::from(height));
        editor.resize(&mut engine, 180.0).unwrap();
        assert_eq!(editor.active_outline().bounds().width(), 180.0);
        assert_eq!(editor.active_outline().layout.reserved_width, None);
        assert_eq!(editor.active_outline().layout.width_set_by_user, Some(true));
        assert_eq!(editor.active_outline().document.nodes(), source);
        let cached = editor
            .active_outline()
            .paragraph_layout(0)
            .unwrap()
            .text
            .shaped
            .styles()
            .as_ptr();
        editor.resize(&mut engine, 180.0).unwrap();
        assert_eq!(
            editor
                .active_outline()
                .paragraph_layout(0)
                .unwrap()
                .text
                .shaped
                .styles()
                .as_ptr(),
            cached
        );
    }

    #[test]
    fn imported_indentation_markers_and_collapsed_children_share_edit_geometry() {
        let document = TextDocument::new(
            [
                "First line",
                "Indented text with enough words to wrap",
                "Collapsed",
                "Hidden",
                "Last",
            ]
            .into_iter()
            .map(|s| Paragraph::new(s.into(), Format::default()))
            .collect(),
        )
        .unwrap();
        let mut nodes = document.nodes().to_vec();
        nodes[1].parent = Some(nodes[0].id);
        nodes[1].level = 2;
        nodes[2].parent = Some(nodes[0].id);
        nodes[2].level = 2;
        nodes[2].collapsed = true;
        nodes[3].parent = Some(nodes[2].id);
        nodes[3].level = 3;
        let marker = onestore::page::text::new_id().unwrap();
        nodes[1].lists.push(marker);
        let definitions = BTreeMap::from([(
            marker,
            Definition {
                kind: onestore::document::Kind::List {
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
        let outline = Outline {
            title: false,
            min_width: None,
            id: onestore::page::text::new_id().unwrap(),
            layout: onestore::document::Layout {
                x: Some(30.0),
                y: Some(45.0),
                max_width: Some(150.0),
                width_set_by_user: Some(true),
                ..Default::default()
            },
            indents: vec![18.0, 0.0, 27.0],
            paragraphs: nodes.clone(),
            unsupported: Vec::new(),
        };
        let mut engine = TextEngine::default();
        let expected = layout_snapshot(&outline.layout(&mut engine, &definitions).unwrap());
        let id = outline.id;
        let mut editor =
            CanvasEditor::from_outlines(&mut engine, vec![outline], definitions).unwrap();
        assert_eq!(editor.active_outline().id, id);
        assert_eq!(editor.active_outline().origin(), [30.0, 45.0]);
        assert_eq!(
            editor
                .active_outline()
                .layouts()
                .map(|(i, _)| i)
                .collect::<Vec<_>>(),
            [0, 1, 2, 4]
        );
        assert_eq!(layout_snapshot(&editor.active_outline().shaped), expected);
        let second = editor.active_outline().paragraph_layout(1).unwrap();
        assert_eq!(second.origin[0], 27.0);
        let y = second.origin[1] + second.text.lines().next().unwrap().1.height * 0.5;
        editor.select_at(27.0, y, false).unwrap();
        assert_eq!(
            editor.selection().positions[1],
            TextPosition {
                paragraph: 1,
                offset: 0
            }
        );
        assert_eq!(editor.caret(1.0).unwrap().x0, 27.0);
        let retained = editor
            .active_outline()
            .paragraph_layout(4)
            .unwrap()
            .text
            .shaped
            .styles()
            .as_ptr();
        editor.insert(&mut engine, "More words ").unwrap();
        assert_eq!(
            editor
                .active_outline()
                .paragraph_layout(4)
                .unwrap()
                .text
                .shaped
                .styles()
                .as_ptr(),
            retained
        );
        let rebuilt = Outline {
            id,
            title: false,
            min_width: None,
            layout: editor.active_outline().layout.clone(),
            indents: vec![18.0, 0.0, 27.0],
            paragraphs: editor.active_outline().document.nodes().to_vec(),
            unsupported: Vec::new(),
        }
        .layout(&mut engine, &editor.definitions)
        .unwrap();
        assert_eq!(
            layout_snapshot(&editor.active_outline().shaped),
            layout_snapshot(&rebuilt)
        );
        let edited = layout_snapshot(&editor.active_outline().shaped);
        let before = editor.active_outline().document.clone();
        let selection = editor.selection();
        editor.enter(&mut engine, false).unwrap();
        let split = &editor.active_outline().document.nodes()[2];
        assert_eq!((split.parent, split.level), (Some(nodes[0].id), 2));
        assert_ne!(split.lists, [marker]);
        assert_eq!(
            editor.definitions[&split.lists[0]],
            editor.definitions[&marker]
        );
        editor.undo(&mut engine).unwrap();
        assert_eq!(editor.active_outline().document, before);
        assert!(editor.resize(&mut engine, 20.0).is_err());
        assert_eq!(editor.active_outline().document, before);
        assert_eq!(editor.selection(), selection);
        assert_eq!(layout_snapshot(&editor.active_outline().shaped), edited);
        editor.undo(&mut engine).unwrap();
        assert_eq!(editor.active_outline().document.nodes(), nodes);
        assert_eq!(layout_snapshot(&editor.active_outline().shaped), expected);
        editor.redo(&mut engine).unwrap();
        assert_eq!(layout_snapshot(&editor.active_outline().shaped), edited);
        assert!(
            editor
                .select(
                    [TextPosition {
                        paragraph: 3,
                        offset: 0
                    }; 2]
                        .into()
                )
                .is_err()
        );
        editor
            .select(
                [TextPosition {
                    paragraph: 2,
                    offset: 9,
                }; 2]
                    .into(),
            )
            .unwrap();
        editor
            .move_selection(&mut engine, Movement::Right, false)
            .unwrap();
        assert_eq!(editor.selection().positions[1].paragraph, 4);
        editor
            .move_selection(&mut engine, Movement::Left, false)
            .unwrap();
        assert_eq!(editor.selection().positions[1].paragraph, 2);
        editor
            .move_selection(&mut engine, Movement::Down, false)
            .unwrap();
        assert_eq!(editor.selection().positions[1].paragraph, 4);
        editor
            .move_selection(&mut engine, Movement::Up, false)
            .unwrap();
        assert_eq!(editor.selection().positions[1].paragraph, 2);
        editor.select_all().unwrap();
        assert!(!editor.selection_rects().unwrap().is_empty());
    }

    #[test]
    fn joins_pass_over_hidden_children_and_a_split_hands_them_on_still_collapsed() {
        let mut engine = TextEngine::default();
        let mut nodes = TextDocument::new(
            ["Collapsed", "Hidden", "After"]
                .into_iter()
                .map(|text| Paragraph::new(text.into(), Format::default()))
                .collect(),
        )
        .unwrap()
        .nodes()
        .to_vec();
        nodes[0].collapsed = true;
        nodes[1].parent = Some(nodes[0].id);
        nodes[1].level = 2;
        let outline = Outline {
            paragraphs: nodes.clone(),
            ..TextOutline::new(
                &mut engine,
                TextDocument::from_nodes(nodes.clone()).unwrap(),
                300.0,
                [0.0; 2],
            )
            .unwrap()
            .snapshot()
        };
        let mut editor =
            CanvasEditor::from_outlines(&mut engine, vec![outline], BTreeMap::new()).unwrap();
        let texts = |editor: &CanvasEditor| {
            editor
                .active_outline()
                .document
                .paragraphs()
                .map(|text| text.text().to_owned())
                .collect::<Vec<_>>()
        };
        let at = |paragraph, offset| [TextPosition { paragraph, offset }; 2].into();
        editor.select(at(2, 0)).unwrap();
        assert!(editor.delete(&mut engine, true).unwrap());
        assert_eq!(texts(&editor), ["CollapsedAfter", "Hidden"]);
        assert_eq!(
            editor.selection().positions,
            [TextPosition {
                paragraph: 0,
                offset: 9
            }; 2]
        );
        editor.undo(&mut engine).unwrap();
        editor.select(at(0, 9)).unwrap();
        assert!(editor.delete(&mut engine, false).unwrap());
        assert_eq!(texts(&editor), ["CollapsedAfter", "Hidden"]);
        editor.undo(&mut engine).unwrap();
        editor.select(at(0, 9)).unwrap();
        editor.enter(&mut engine, false).unwrap();
        let split = editor.active_outline().document.nodes();
        assert!(!split[0].collapsed && split[1].collapsed);
        assert_eq!(split[2].parent, Some(split[1].id));
        assert_eq!(
            editor
                .active_outline()
                .layouts()
                .map(|(index, _)| index)
                .collect::<Vec<_>>(),
            [0, 1, 3]
        );
        editor.undo(&mut engine).unwrap();
        assert_eq!(editor.active_outline().document.nodes(), nodes);
    }

    #[test]
    #[ignore = "requires CANVAS_TEST_SECTION and CANVAS_TEST_PAGE private fixture inputs"]
    fn imported_editor_reflows_and_restores_native_outline_geometry() {
        use onestore::page::{Page, PageObject};
        use onestore::{RevisionIndex, Store, document::Document};
        let bytes = std::fs::read(std::env::var_os("CANVAS_TEST_SECTION").unwrap()).unwrap();
        let page = {
            let store = Store::parse(&bytes).unwrap();
            let index = RevisionIndex::parse(&store).unwrap();
            Page::from_document(
                &Document::parse(&index).unwrap(),
                &std::env::var("CANVAS_TEST_PAGE").unwrap(),
            )
            .unwrap()
        };
        drop(bytes);
        let mut engine = TextEngine::default();
        if let Some(font) = std::env::var_os("CANVAS_TEST_SUBSTITUTE") {
            engine
                .register_substitute(parley::fontique::Blob::new(std::sync::Arc::new(
                    std::fs::read(font).unwrap(),
                )))
                .unwrap();
        }
        let outlines: Vec<_> = page
            .objects
            .into_iter()
            .filter_map(|object| match object {
                PageObject::Outline(outline) => Some(outline),
                _ => None,
            })
            .collect();
        let original: Vec<_> = outlines
            .iter()
            .map(|outline| {
                (
                    outline.id,
                    outline.layout.clone(),
                    outline.paragraphs.clone(),
                    layout_snapshot(&outline.layout(&mut engine, &page.definitions).unwrap()),
                )
            })
            .collect();
        let mut editor =
            CanvasEditor::from_outlines(&mut engine, outlines, page.definitions).unwrap();
        let mut count = 0;
        for (id, source_layout, nodes, expected) in original {
            editor.focus_outline(id).unwrap();
            assert_eq!(editor.active_outline().layout, source_layout);
            assert_eq!(layout_snapshot(&editor.active_outline().shaped), expected);
            let visible: Vec<_> = editor.active_outline().layouts().map(|(i, _)| i).collect();
            for paragraph in visible {
                let text = &nodes[paragraph].text().unwrap().text;
                let byte = text
                    .text()
                    .char_indices()
                    .nth(text.text().chars().count() / 2)
                    .map(|(byte, _)| byte)
                    .unwrap_or(0);
                let position = TextPosition {
                    paragraph,
                    offset: text.utf16_offset(byte).unwrap(),
                };
                editor.select([position; 2].into()).unwrap();
                editor.insert(&mut engine, "probe ").unwrap();
                let edited = editor.active_outline().document.clone();
                let rebuilt = Outline {
                    id,
                    title: false,
                    min_width: None,
                    layout: source_layout.clone(),
                    indents: editor.active_outline().indents.clone(),
                    paragraphs: edited.nodes().to_vec(),
                    unsupported: Vec::new(),
                }
                .layout(&mut engine, &editor.definitions)
                .unwrap();
                let after = layout_snapshot(&editor.active_outline().shaped);
                assert_eq!(after, layout_snapshot(&rebuilt));
                editor.undo(&mut engine).unwrap();
                assert_eq!(editor.active_outline().document.nodes(), nodes);
                assert_eq!(layout_snapshot(&editor.active_outline().shaped), expected);
                editor.redo(&mut engine).unwrap();
                assert_eq!(editor.active_outline().document, edited);
                assert_eq!(layout_snapshot(&editor.active_outline().shaped), after);
                editor.undo(&mut engine).unwrap();
                count += 1;
            }
        }
        eprintln!(
            "Reflowed and restored {count} visible paragraphs in {} native outlines",
            editor.outlines().len()
        );
        assert!(count > 0);
    }

    #[test]
    fn visual_navigation_does_not_bounce_between_opposite_direction_paragraphs() {
        let mut engine = TextEngine::default();
        let document = TextDocument::new(
            ["a", "שלום", "end"]
                .into_iter()
                .map(|text| Paragraph::new(text.into(), Format::default()))
                .collect(),
        )
        .unwrap();
        let mut editor = CanvasEditor::new(&mut engine, document, 100.0).unwrap();
        let first = TextPosition {
            paragraph: 0,
            offset: 0,
        };
        let last = TextPosition {
            paragraph: 2,
            offset: 3,
        };
        for _ in 0..32 {
            editor
                .move_selection(&mut engine, Movement::Right, false)
                .unwrap();
            if editor.selection().positions[1] == last {
                break;
            }
        }
        assert_eq!(editor.selection().positions[1], last);
        for _ in 0..32 {
            editor
                .move_selection(&mut engine, Movement::Left, false)
                .unwrap();
            if editor.selection().positions[1] == first {
                break;
            }
        }
        assert_eq!(editor.selection().positions[1], first);
    }

    #[test]
    fn styled_reversed_selection_survives_reflow_undo_and_navigation_reuses_layout() {
        let mut engine = TextEngine::default();
        let original = TextDocument::new(vec![Paragraph::from_runs([
            ("plain ".into(), Format::default()),
            (
                "bold text".into(),
                Format {
                    bold: Some(true),
                    ..Format::default()
                },
            ),
        ])])
        .unwrap();
        let mut editor = CanvasEditor::new(&mut engine, original.clone(), 100.0).unwrap();
        let anchor = TextPosition {
            paragraph: 0,
            offset: 10,
        };
        let focus = TextPosition {
            paragraph: 0,
            offset: 2,
        };
        editor.select([anchor, focus].into()).unwrap();
        editor.insert(&mut engine, "🌳").unwrap();
        let after = editor.active_outline().document.clone();
        editor.resize(&mut engine, 35.0).unwrap();
        editor.undo(&mut engine).unwrap();
        assert_eq!(editor.active_outline().document, after);
        assert_eq!(editor.active_outline().layout.max_width, Some(100.0));
        editor.undo(&mut engine).unwrap();
        assert_eq!(editor.active_outline().document, original);
        assert_eq!(editor.selection().positions, [anchor, focus]);
        editor.redo(&mut engine).unwrap();
        assert_eq!(editor.active_outline().document, after);
        assert_eq!(editor.selection().positions[1].offset, 4);
        editor.redo(&mut engine).unwrap();
        assert_eq!(editor.active_outline().layout.max_width, Some(35.0));
        let styles = editor.active_outline().shaped.paragraphs[0]
            .text
            .shaped
            .styles()
            .as_ptr();
        for movement in [
            Movement::Left,
            Movement::WordRight,
            Movement::Down,
            Movement::Up,
            Movement::LineStart,
            Movement::LineEnd,
        ] {
            editor.move_selection(&mut engine, movement, false).unwrap();
            assert_eq!(
                editor.active_outline().shaped.paragraphs[0]
                    .text
                    .shaped
                    .styles()
                    .as_ptr(),
                styles
            );
        }
        editor.resize(&mut engine, 35.0).unwrap();
        editor.select_at(15.0, 20.0, true).unwrap();
        assert_eq!(
            editor.active_outline().shaped.paragraphs[0]
                .text
                .shaped
                .styles()
                .as_ptr(),
            styles
        );
    }

    #[test]
    fn multiline_composition_is_provisional_and_commits_as_one_structural_edit() {
        let mut engine = TextEngine::default();
        let original = TextDocument::new(vec![
            Paragraph::new("abcd".into(), Format::default()),
            Paragraph::new(
                "middle".into(),
                Format {
                    bold: Some(true),
                    ..Format::default()
                },
            ),
            Paragraph::new("tail".into(), Format::default()),
            Paragraph::new("untouched".into(), Format::default()),
        ])
        .unwrap();
        let mut editor = CanvasEditor::new(&mut engine, original.clone(), 70.0).unwrap();
        let untouched = editor.active_outline().shaped.paragraphs[3]
            .text
            .shaped
            .styles()
            .as_ptr();
        let start = TextPosition {
            paragraph: 0,
            offset: 1,
        };
        let end = TextPosition {
            paragraph: 2,
            offset: 2,
        };
        editor.select([end, start].into()).unwrap();
        editor.compose(&mut engine, "ni\nhao".into(), 2..3).unwrap();
        assert_eq!(
            editor.selection().positions,
            [
                TextPosition {
                    paragraph: 0,
                    offset: 3
                },
                TextPosition {
                    paragraph: 1,
                    offset: 0
                }
            ]
        );
        assert_eq!(
            editor.marked_range(),
            Some(
                start..TextPosition {
                    paragraph: 1,
                    offset: 3
                }
            )
        );
        editor
            .compose(&mut engine, "e\u{301}".into(), 1..1)
            .unwrap();
        assert_eq!(
            editor.selection().positions,
            [TextPosition {
                paragraph: 0,
                offset: 2
            }; 2]
        );
        editor.commit_text(&mut engine, "日\n本".into()).unwrap();
        assert_eq!(
            editor
                .active_outline()
                .document
                .paragraphs()
                .map(Paragraph::text)
                .collect::<Vec<_>>(),
            ["a日", "本il", "untouched"]
        );
        assert!(editor.marked_range().is_none());
        assert_eq!(
            editor.active_outline().shaped.paragraphs[2]
                .text
                .shaped
                .styles()
                .as_ptr(),
            untouched
        );
        editor.undo(&mut engine).unwrap();
        assert_eq!(editor.active_outline().document, original);
        assert_eq!(editor.selection().positions, [end, start]);
        assert!(!editor.undo(&mut engine).unwrap());
        editor.redo(&mut engine).unwrap();
        assert_eq!(
            editor
                .active_outline()
                .document
                .paragraphs()
                .nth(1)
                .unwrap()
                .text(),
            "本il"
        );
    }

    #[test]
    fn composition_failure_and_cancel_preserve_source_selection_and_redo() {
        let mut engine = TextEngine::default();
        let original = TextDocument::new(vec![
            Paragraph::new("first".into(), Format::default()),
            Paragraph::new("last".into(), Format::default()),
        ])
        .unwrap();
        let mut editor = CanvasEditor::new(&mut engine, original.clone(), 70.0).unwrap();
        editor.insert(&mut engine, "!").unwrap();
        editor.undo(&mut engine).unwrap();
        let first = TextPosition {
            paragraph: 0,
            offset: 0,
        };
        let last = TextPosition {
            paragraph: 1,
            offset: 4,
        };
        editor.select([last, first].into()).unwrap();
        editor
            .compose(&mut engine, "pre\nedit".into(), 8..8)
            .unwrap();
        let provisional = editor.active_outline().document.clone();
        let selection = editor.selection().positions;
        let marked = editor.marked_range();
        assert!(editor.compose(&mut engine, "🌳".into(), 1..1).is_err());
        assert_eq!(editor.active_outline().document, provisional);
        assert_eq!(editor.selection().positions, selection);
        assert_eq!(editor.marked_range(), marked);
        let preview = editor.preview_resize(&mut engine, 25.0).unwrap();
        assert_eq!(preview.document, provisional);
        assert_eq!(preview.layout.max_width, Some(25.0));
        assert_eq!(editor.active_outline().layout.max_width, Some(70.0));
        editor.cancel_composition(&mut engine).unwrap();
        assert_eq!(editor.active_outline().document, original);
        assert_eq!(editor.selection().positions, [last, first]);
        editor.redo(&mut engine).unwrap();
        assert_eq!(
            editor
                .active_outline()
                .document
                .paragraphs()
                .next()
                .unwrap()
                .text(),
            "!first"
        );
    }

    #[test]
    fn navigation_crosses_paragraphs_and_keeps_the_preferred_column() {
        let mut engine = TextEngine::default();
        let document = TextDocument::new(vec![
            Paragraph::new("abcdefghij".into(), Format::default()),
            Paragraph::new(String::new(), Format::default()),
            Paragraph::new("abcdefghij".into(), Format::default()),
        ])
        .unwrap();
        let mut editor = CanvasEditor::new(&mut engine, document, 200.0).unwrap();
        let start = TextPosition {
            paragraph: 0,
            offset: 8,
        };
        editor.select([start, start].into()).unwrap();
        let x = editor.caret(1.0).unwrap().x0;
        editor
            .move_selection(&mut engine, Movement::Down, false)
            .unwrap();
        assert_eq!(
            editor.selection().positions[1],
            TextPosition {
                paragraph: 1,
                offset: 0
            }
        );
        editor
            .move_selection(&mut engine, Movement::Down, false)
            .unwrap();
        assert_eq!(
            editor.selection().positions[1],
            TextPosition {
                paragraph: 2,
                offset: 8
            }
        );
        assert_eq!(editor.caret(1.0).unwrap().x0, x);
        editor
            .move_selection(&mut engine, Movement::Up, true)
            .unwrap();
        editor
            .move_selection(&mut engine, Movement::Up, true)
            .unwrap();
        assert_eq!(
            editor.selection().positions,
            [
                TextPosition {
                    paragraph: 2,
                    offset: 8
                },
                start
            ]
        );
        editor
            .move_selection(&mut engine, Movement::Left, false)
            .unwrap();
        assert_eq!(editor.selection().positions, [start; 2]);
        let boundary = TextPosition {
            paragraph: 1,
            offset: 0,
        };
        editor.select([boundary, boundary].into()).unwrap();
        editor
            .move_selection(&mut engine, Movement::Left, false)
            .unwrap();
        assert_eq!(
            editor.selection().positions[1],
            TextPosition {
                paragraph: 0,
                offset: 10
            }
        );
        editor
            .move_selection(&mut engine, Movement::Right, false)
            .unwrap();
        assert_eq!(editor.selection().positions[1], boundary);
    }

    #[test]
    fn typing_enter_and_boundary_delete_form_one_reversible_history() {
        let mut engine = TextEngine::default();
        let original =
            TextDocument::new(vec![Paragraph::new(String::new(), Format::default())]).unwrap();
        let mut editor = CanvasEditor::new(&mut engine, original.clone(), 80.0).unwrap();
        assert!(!editor.delete(&mut engine, true).unwrap());
        assert!(!editor.delete(&mut engine, false).unwrap());
        editor.insert(&mut engine, "left\n\nright").unwrap();
        assert_eq!(
            editor
                .active_outline()
                .document
                .paragraphs()
                .map(Paragraph::text)
                .collect::<Vec<_>>(),
            ["left", "", "right"]
        );
        let point = TextPosition {
            paragraph: 2,
            offset: 0,
        };
        editor.select([point, point].into()).unwrap();
        editor.delete(&mut engine, true).unwrap();
        assert_eq!(
            editor
                .active_outline()
                .document
                .paragraphs()
                .map(Paragraph::text)
                .collect::<Vec<_>>(),
            ["left", "right"]
        );
        let point = TextPosition {
            paragraph: 0,
            offset: 4,
        };
        editor.select([point, point].into()).unwrap();
        editor.delete(&mut engine, false).unwrap();
        assert_eq!(
            editor
                .active_outline()
                .document
                .paragraphs()
                .next()
                .unwrap()
                .text(),
            "leftright"
        );
        for _ in 0..3 {
            assert!(editor.undo(&mut engine).unwrap());
        }
        assert_eq!(editor.active_outline().document, original);
        for _ in 0..3 {
            assert!(editor.redo(&mut engine).unwrap());
        }
        assert_eq!(
            editor
                .active_outline()
                .document
                .paragraphs()
                .next()
                .unwrap()
                .text(),
            "leftright"
        );
    }

    #[test]
    fn grapheme_deletion_preserves_adjacent_hidden_fields() {
        let mut engine = TextEngine::default();
        for grapheme in ["e\u{301}", "👩🏽‍💻", "🇨🇦", "👩‍👩‍👧‍👦"] {
            let original = TextDocument::new(vec![Paragraph::from_runs([
                (
                    "field".into(),
                    Format {
                        hidden: Some(true),
                        ..Format::default()
                    },
                ),
                (format!("a{grapheme}z"), Format::default()),
            ])])
            .unwrap();
            let mut editor = CanvasEditor::new(&mut engine, original.clone(), 80.0).unwrap();
            for backward in [false, true] {
                let offset = 6 + if backward {
                    grapheme.encode_utf16().count() as u32
                } else {
                    0
                };
                let point = TextPosition {
                    paragraph: 0,
                    offset,
                };
                editor.select([point, point].into()).unwrap();
                editor.delete(&mut engine, backward).unwrap();
                assert_eq!(
                    editor
                        .active_outline()
                        .document
                        .paragraphs()
                        .next()
                        .unwrap()
                        .text(),
                    "fieldaz"
                );
                assert_eq!(
                    editor.active_outline().shaped.paragraphs[0]
                        .projection
                        .text()
                        .text(),
                    "az"
                );
                editor.undo(&mut engine).unwrap();
                assert_eq!(editor.active_outline().document, original);
                assert_eq!(editor.selection().positions, [point; 2]);
            }
        }
    }

    #[test]
    fn pointer_selection_maps_hidden_source_and_paragraph_boundaries() {
        let mut engine = TextEngine::default();
        let text = TextDocument::new(vec![
            Paragraph::from_runs([
                ("first ".into(), Format::default()),
                (
                    "hidden🌳".into(),
                    Format {
                        hidden: Some(true),
                        ..Format::default()
                    },
                ),
                ("visible words wrap here".into(), Format::default()),
            ]),
            Paragraph::new(String::new(), Format::default()),
            Paragraph::new("last".into(), Format::default()),
        ])
        .unwrap();
        let mut editor = CanvasEditor::new(&mut engine, text.clone(), 70.0).unwrap();
        let last = TextPosition {
            paragraph: 2,
            offset: 4,
        };
        editor.select([last, last].into()).unwrap();
        let caret = editor.caret(1.0).unwrap();
        editor
            .select_at(caret.x0 as f32, ((caret.y0 + caret.y1) / 2.0) as f32, false)
            .unwrap();
        assert_eq!(editor.selection().positions, [last; 2]);
        let first = TextPosition {
            paragraph: 0,
            offset: 0,
        };
        editor.select([last, first].into()).unwrap();
        let rectangles = editor.selection_rects().unwrap();
        assert!(
            rectangles
                .iter()
                .any(|r| r.y0 == f64::from(ys(editor.active_outline())[1]) && r.width() == 4.0)
        );
        editor
            .replace(
                &mut engine,
                vec![Paragraph::new("replacement".into(), Format::default())],
            )
            .unwrap();
        editor.undo(&mut engine).unwrap();
        assert_eq!(editor.active_outline().document, text);
        assert_eq!(editor.selection().positions, [last, first]);

        let source = &editor.active_outline().shaped.paragraphs[0];
        let cursor = source.text.cursor(6, Affinity::Downstream);
        let caret = source.text.caret(cursor, 1.0);
        editor
            .select_at(caret.x0 as f32, ((caret.y0 + caret.y1) / 2.0) as f32, false)
            .unwrap();
        let selected = editor.selection().positions[1];
        assert!(selected.offset == 6 || selected.offset == 14);
        assert_eq!(editor.caret(1.0).unwrap().y0, caret.y0);
    }

    #[test]
    fn mixed_structural_edit_history_matches_full_layout_recomputation() {
        let mut engine = TextEngine::default();
        let text = TextDocument::new(vec![
            Paragraph::new("one two three".into(), Format::default()),
            Paragraph::new("日本 🌳 e\u{301}".into(), Format::default()),
        ])
        .unwrap();
        let mut editor = CanvasEditor::new(&mut engine, text, 72.0).unwrap();
        let mut seed = 0x8d45_u64;
        let mut next = || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            seed as usize
        };
        let mut before = Vec::new();
        let mut after = Vec::new();
        for step in 0..64 {
            let positions: Vec<_> = editor
                .active_outline()
                .document
                .paragraphs()
                .enumerate()
                .flat_map(|(paragraph, text)| {
                    (0..=text.text().len()).filter_map(move |byte| {
                        text.utf16_offset(byte)
                            .ok()
                            .map(|offset| TextPosition { paragraph, offset })
                    })
                })
                .collect();
            let anchor = positions[next() % positions.len()];
            let focus = positions[next() % positions.len()];
            editor.select([anchor, focus].into()).unwrap();
            before.push((
                editor.active_outline().document.clone(),
                editor.selection().positions,
            ));
            let format = Format {
                bold: Some(step % 2 == 0),
                font_size: Some(10.0 + (step % 4) as f32),
                space_before: Some((step % 3) as f32),
                space_after: Some((step % 5) as f32),
                line_spacing: Some(12.0 + (step % 2) as f32),
                ..Format::default()
            };
            let replacement = match step % 4 {
                0 => vec![Paragraph::new(String::new(), format); 2],
                1 => vec![Paragraph::new("x👩🏽‍💻".into(), format)],
                2 => vec![Paragraph::new(String::new(), format)],
                _ => vec![
                    Paragraph::new("wrapped words".into(), format.clone()),
                    Paragraph::new("tail".into(), format),
                ],
            };
            editor.replace(&mut engine, replacement).unwrap();
            after.push(editor.active_outline().document.clone());
            let rebuilt =
                CanvasEditor::new(&mut engine, editor.active_outline().document.clone(), 72.0)
                    .unwrap();
            assert_eq!(
                ys(editor.active_outline()),
                ys(rebuilt.active_outline()),
                "step {step}"
            );
            for (actual, expected) in editor
                .active_outline()
                .shaped
                .paragraphs
                .iter()
                .zip(&rebuilt.active_outline().shaped.paragraphs)
            {
                let geometry = |p: &ParagraphLayout| {
                    p.text
                        .lines()
                        .map(|(line, b)| {
                            (
                                b.source.clone(),
                                b.top,
                                b.baseline,
                                b.height,
                                line.metrics().advance,
                            )
                        })
                        .collect::<Vec<_>>()
                };
                assert_eq!(geometry(actual), geometry(expected), "step {step}");
            }
        }
        for (text, selection) in before.into_iter().rev() {
            assert!(editor.undo(&mut engine).unwrap());
            assert_eq!(editor.active_outline().document, text);
            assert_eq!(editor.selection().positions, selection);
        }
        assert!(!editor.undo(&mut engine).unwrap());
        for text in after {
            assert!(editor.redo(&mut engine).unwrap());
            assert_eq!(editor.active_outline().document, text);
        }
        assert!(!editor.redo(&mut engine).unwrap());
    }

    #[test]
    fn typing_in_place_matches_a_fresh_layout_for_automatic_and_fixed_widths() {
        use onestore::page::{Image, ParagraphContent, text::new_id};
        let mut engine = TextEngine::default();
        let mut seed = 0x51ed_u64;
        let mut next = |n: usize| {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (seed >> 33) as usize % n
        };
        for (text, objects) in [
            (vec!["x"], false),
            (vec!["one two", "three", "four five six seven"], false),
            (vec!["x"], true),
            (vec!["one two", "three", "four five six seven"], true),
        ] {
            let fixed = text.len() > 1;
            let empty = TextDocument::new(vec![Paragraph::new(String::new(), Format::default())]);
            let mut editor = CanvasEditor::new(&mut engine, empty.unwrap(), 120.0).unwrap();
            editor.insert(&mut engine, &text.join("\n")).unwrap();
            if fixed {
                editor.resize(&mut engine, 120.0).unwrap();
            }
            let table = table_editor(&mut engine).active_outline().document.nodes()[0].clone();
            let mut picture = table.clone();
            picture.id = new_id().unwrap();
            picture.content = ParagraphContent::Image(Image {
                id: new_id().unwrap(),
                layout: onestore::document::Layout {
                    max_width: Some(90.0),
                    max_height: Some(40.0),
                    ..Default::default()
                },
                size: None,
                bytes: None,
                alt: None,
                background: false,
            });
            if objects {
                let selection = [TextPosition {
                    paragraph: 0,
                    offset: 0,
                }; 2]
                    .into();
                editor
                    .commit(
                        &mut engine,
                        DocumentEdit {
                            columns: BTreeMap::new(),
                            container: None,
                            range: 1..1,
                            replacement: vec![picture.clone(), table],
                        },
                        selection,
                    )
                    .unwrap();
            }
            for step in 0..300 {
                let outline = editor.active_outline();
                let paragraph = next(outline.document.paragraphs().count());
                let text = outline.document.paragraph(paragraph).unwrap();
                let offset = text.utf16_offset(text.text().len()).unwrap();
                let position = TextPosition {
                    paragraph,
                    offset: next(offset as usize + 1) as u32,
                };
                // Paragraphs under a collapsed one hold no caret.
                let _ = editor.select([position; 2].into());
                let nodes = editor.active_outline().document.nodes();
                let root = next(nodes.len());
                let mut node = nodes[root].clone();
                let edit = |range, replacement| DocumentEdit {
                    columns: BTreeMap::new(),
                    container: None,
                    range,
                    replacement,
                };
                let edit = match next(14) {
                    10 => {
                        node.collapsed = !node.collapsed;
                        Some(edit(root..root + 1, vec![node]))
                    }
                    11 if root > 0 && node.text().is_some() => {
                        let parent = &nodes[root - 1];
                        node.parent = Some(parent.id);
                        node.level = parent.level + 1;
                        Some(edit(root..root + 1, vec![node]))
                    }
                    12 if node.text().is_none() || nodes.len() > 3 => {
                        Some(edit(root..root + 1, Vec::new()))
                    }
                    13 => {
                        let mut picture = picture.clone();
                        picture.id = new_id().unwrap();
                        let ParagraphContent::Image(image) = &mut picture.content else {
                            unreachable!()
                        };
                        image.id = new_id().unwrap();
                        Some(edit(root..root, vec![picture]))
                    }
                    _ => None,
                };
                let _ = match (edit, next(10)) {
                    (Some(edit), _) => {
                        let start = [TextPosition {
                            paragraph: 0,
                            offset: 0,
                        }; 2];
                        editor.commit(&mut engine, edit, start.into())
                    }
                    (None, 0) => editor.insert(&mut engine, "\n"),
                    (None, 1 | 2) => editor.delete(&mut engine, next(2) == 0).map(drop),
                    (None, 3) => editor.undo(&mut engine).map(drop),
                    (None, _) => editor.insert(&mut engine, ["a", " ", "wide", "W"][next(4)]),
                };
                let outline = editor.active_outline();
                let fresh =
                    TextOutline::from_outline(&mut engine, &outline.snapshot(), &BTreeMap::new())
                        .unwrap();
                let layout = |outline: &TextOutline| {
                    let shaped = &outline.shaped;
                    let paragraphs = shaped.paragraphs.iter().map(|paragraph| {
                        (
                            paragraph.id,
                            paragraph.origin,
                            paragraph
                                .markers
                                .iter()
                                .map(|marker| marker.1)
                                .collect::<Vec<_>>(),
                            paragraph
                                .tags
                                .iter()
                                .map(|tag| tag.origin)
                                .collect::<Vec<_>>(),
                        )
                    });
                    let cells = shaped.tables.iter().flat_map(|table| {
                        table
                            .cells
                            .iter()
                            .map(|cell| (cell.id, cell.rect, cell.paragraphs.clone()))
                    });
                    let objects = shaped.objects.iter().map(|object| {
                        let label = object.label().map(|label| label.origin);
                        (object.id, object.rect, object.bottom, label)
                    });
                    format!(
                        "{:?}",
                        (
                            shaped.size,
                            shaped.widths,
                            outline.bounds(),
                            paragraphs.collect::<Vec<_>>(),
                            cells.collect::<Vec<_>>(),
                            objects.collect::<Vec<_>>(),
                        )
                    )
                };
                assert_eq!(layout(outline), layout(&fresh), "step {step}");
            }
            assert_eq!(
                editor.active_outline().layout.width_set_by_user,
                Some(fixed)
            );
        }
    }

    #[test]
    fn structural_edits_reflow_only_affected_paragraphs_and_restore_selection() {
        let mut engine = TextEngine::default();
        let text = TextDocument::new(vec![
            Paragraph::new("before".into(), Format::default()),
            Paragraph::new("split this paragraph".into(), Format::default()),
            Paragraph::new("after".into(), Format::default()),
        ])
        .unwrap();
        let mut editor = CanvasEditor::new(&mut engine, text.clone(), 60.0).unwrap();
        let before = editor.active_outline().shaped.paragraphs[0]
            .text
            .shaped
            .styles()
            .as_ptr();
        let after = editor.active_outline().shaped.paragraphs[2]
            .text
            .shaped
            .styles()
            .as_ptr();
        editor.resize(&mut engine, 60.0).unwrap();
        assert_eq!(
            editor.active_outline().shaped.paragraphs[0]
                .text
                .shaped
                .styles()
                .as_ptr(),
            before
        );
        let point = TextPosition {
            paragraph: 1,
            offset: 6,
        };
        editor.select([point, point].into()).unwrap();
        editor
            .replace(
                &mut engine,
                vec![Paragraph::new(String::new(), Format::default()); 2],
            )
            .unwrap();
        assert_eq!(
            editor
                .active_outline()
                .document
                .paragraphs()
                .map(Paragraph::text)
                .collect::<Vec<_>>(),
            ["before", "split ", "this paragraph", "after"]
        );
        assert_eq!(
            editor.selection().positions,
            [TextPosition {
                paragraph: 2,
                offset: 0
            }; 2]
        );
        assert_eq!(
            editor.active_outline().shaped.paragraphs[0]
                .text
                .shaped
                .styles()
                .as_ptr(),
            before
        );
        assert_eq!(
            editor.active_outline().shaped.paragraphs[3]
                .text
                .shaped
                .styles()
                .as_ptr(),
            after
        );
        let rebuilt =
            CanvasEditor::new(&mut engine, editor.active_outline().document.clone(), 60.0).unwrap();
        assert_eq!(ys(editor.active_outline()), ys(rebuilt.active_outline()));
        for (actual, expected) in editor
            .active_outline()
            .shaped
            .paragraphs
            .iter()
            .zip(&rebuilt.active_outline().shaped.paragraphs)
        {
            let lines = |p: &ParagraphLayout| {
                p.text
                    .lines()
                    .map(|(_, b)| (b.source.clone(), b.top, b.baseline, b.height))
                    .collect::<Vec<_>>()
            };
            assert_eq!(lines(actual), lines(expected));
        }
        editor.undo(&mut engine).unwrap();
        assert_eq!(editor.active_outline().document, text);
        assert_eq!(editor.selection().positions, [point; 2]);
        assert_eq!(
            editor.active_outline().shaped.paragraphs[2]
                .text
                .shaped
                .styles()
                .as_ptr(),
            after
        );
        editor.redo(&mut engine).unwrap();
        assert_eq!(
            editor.active_outline().document,
            rebuilt.active_outline().document
        );
        let selection = editor.selection().positions;
        let previous_width = editor.active_outline().layout.max_width.unwrap();
        editor.resize(&mut engine, 30.0).unwrap();
        let rebuilt =
            CanvasEditor::new(&mut engine, editor.active_outline().document.clone(), 30.0).unwrap();
        assert_eq!(ys(editor.active_outline()), ys(rebuilt.active_outline()));
        assert_eq!(editor.selection().positions, selection);
        assert!(editor.resize(&mut engine, f32::NAN).is_err());
        assert_eq!(ys(editor.active_outline()), ys(rebuilt.active_outline()));
        editor.undo(&mut engine).unwrap();
        assert_eq!(
            editor.active_outline().layout.max_width,
            Some(previous_width)
        );
        let rebuilt = CanvasEditor::new(
            &mut engine,
            editor.active_outline().document.clone(),
            previous_width,
        )
        .unwrap();
        assert_eq!(ys(editor.active_outline()), ys(rebuilt.active_outline()));
    }

    #[test]
    fn failed_layout_preserves_text_selection_history_and_cached_layout() {
        let mut engine = TextEngine::default();
        let text =
            TextDocument::new(vec![Paragraph::new("original".into(), Format::default())]).unwrap();
        let mut editor = CanvasEditor::new(&mut engine, text.clone(), 60.0).unwrap();
        editor
            .replace(
                &mut engine,
                vec![Paragraph::new("!".into(), Format::default())],
            )
            .unwrap();
        editor.undo(&mut engine).unwrap();
        let before = editor.active_outline().shaped.paragraphs[0]
            .text
            .shaped
            .styles()
            .as_ptr();
        for format in [
            Format {
                font_size: Some(f32::NAN),
                ..Format::default()
            },
            Format {
                line_spacing: Some(-1.0),
                ..Format::default()
            },
            Format {
                space_after: Some(f32::INFINITY),
                ..Format::default()
            },
        ] {
            assert!(
                editor
                    .replace(&mut engine, vec![Paragraph::new("bad".into(), format)])
                    .is_err()
            );
            assert_eq!(editor.active_outline().document, text);
            assert_eq!(
                editor.selection().positions,
                [TextPosition {
                    paragraph: 0,
                    offset: 0
                }; 2]
            );
            assert_eq!(
                editor.active_outline().shaped.paragraphs[0]
                    .text
                    .shaped
                    .styles()
                    .as_ptr(),
                before
            );
        }
        assert!(editor.redo(&mut engine).unwrap());
        assert_eq!(
            editor
                .active_outline()
                .document
                .paragraphs()
                .next()
                .unwrap()
                .text(),
            "!original"
        );
    }
    #[test]
    fn outline_creation_movement_and_typing_share_one_history() {
        let mut engine = TextEngine::default();
        let original =
            TextDocument::new(vec![Paragraph::new("lyrics".into(), Format::default())]).unwrap();
        let mut editor = CanvasEditor::new(&mut engine, original.clone(), 240.0).unwrap();
        let first = editor.active_outline().id;
        editor.insert(&mut engine, "A ").unwrap();
        let second = editor
            .create_outline(&mut engine, [300.0, 0.0], 120.0)
            .unwrap();
        editor
            .insert(&mut engine, "annotation\nsecond line")
            .unwrap();
        let annotation = editor.active_outline().document.clone();
        let pointers: Vec<_> = editor
            .outlines
            .iter()
            .map(|outline| outline.shaped.paragraphs[0].text.shaped.styles().as_ptr())
            .collect();
        editor.move_outline(first, [-50.0, 60.0]).unwrap();
        assert_eq!(editor.active_outline().id, first);
        assert_eq!(
            editor
                .outlines
                .iter()
                .map(|outline| outline.shaped.paragraphs[0].text.shaped.styles().as_ptr())
                .collect::<Vec<_>>(),
            pointers
        );
        assert_eq!(editor.outlines[1].document, annotation);
        editor.undo(&mut engine).unwrap();
        assert_eq!(editor.active_outline().origin(), [0.0; 2]);
        editor.undo(&mut engine).unwrap();
        assert_eq!(editor.active_outline().id, second);
        assert_eq!(
            editor
                .active_outline()
                .document
                .paragraphs()
                .next()
                .unwrap()
                .text(),
            ""
        );
        let retained = editor.active_outline().shaped.paragraphs[0]
            .text
            .shaped
            .styles()
            .as_ptr();
        editor.undo(&mut engine).unwrap();
        assert_eq!(editor.outlines.len(), 1);
        assert_eq!(editor.active_outline().id, first);
        editor.undo(&mut engine).unwrap();
        assert_eq!(editor.active_outline().document, original);
        assert!(!editor.undo(&mut engine).unwrap());
        editor.redo(&mut engine).unwrap();
        editor.redo(&mut engine).unwrap();
        assert_eq!(editor.active_outline().id, second);
        assert_eq!(
            editor.active_outline().shaped.paragraphs[0]
                .text
                .shaped
                .styles()
                .as_ptr(),
            retained
        );
        editor.redo(&mut engine).unwrap();
        assert_eq!(editor.active_outline().document, annotation);
        editor.redo(&mut engine).unwrap();
        assert_eq!(editor.active_outline().id, first);
        assert_eq!(editor.active_outline().origin(), [-50.0, 60.0]);
        assert!(!editor.redo(&mut engine).unwrap());
    }

    #[test]
    fn invalid_outline_operations_preserve_composition_and_history() {
        let mut engine = TextEngine::default();
        let original =
            TextDocument::new(vec![Paragraph::new("original".into(), Format::default())]).unwrap();
        let mut editor = CanvasEditor::new(&mut engine, original.clone(), 240.0).unwrap();
        let first = editor.active_outline().id;
        editor.compose(&mut engine, "é".into(), 1..1).unwrap();
        let provisional = editor.active_outline().document.clone();
        for position in [[f32::NAN, 0.0], [0.0, f32::INFINITY]] {
            assert!(editor.create_outline(&mut engine, position, 120.0).is_err());
            assert!(editor.move_outline(first, position).is_err());
        }
        assert!(editor.create_outline(&mut engine, [0.0; 2], 0.0).is_err());
        assert!(editor.focus_outline(onestore::ExGuid::default()).is_err());
        assert!(
            editor
                .move_outline(onestore::ExGuid::default(), [1.0; 2])
                .is_err()
        );
        assert_eq!(editor.active_outline().document, provisional);
        assert!(editor.marked_range().is_some());
        assert!(editor.undo.is_empty());
        assert!(editor.redo.is_empty());
        let second = editor
            .create_outline(&mut engine, [300.0, 0.0], 120.0)
            .unwrap();
        assert!(editor.marked_range().is_none());
        editor.undo(&mut engine).unwrap();
        assert_eq!(editor.active_outline().document, provisional);
        editor.undo(&mut engine).unwrap();
        assert_eq!(editor.active_outline().document, original);
        editor.redo(&mut engine).unwrap();
        let replacement = editor
            .create_outline(&mut engine, [500.0, 0.0], 120.0)
            .unwrap();
        assert_ne!(replacement, second);
        assert!(!editor.redo(&mut engine).unwrap());
        assert_eq!(editor.outlines[0].document, provisional);
    }
    #[test]
    fn mixed_outline_history_restores_every_source_and_placement() {
        let mut engine = TextEngine::default();
        let original = TextDocument::new(vec![Paragraph::from_runs([
            (
                "hidden".into(),
                Format {
                    hidden: Some(true),
                    ..Format::default()
                },
            ),
            ("lyrics 🌳".into(), Format::default()),
        ])])
        .unwrap();
        let mut editor = CanvasEditor::new(&mut engine, original, 120.0).unwrap();
        let snapshot = |editor: &CanvasEditor| {
            editor
                .outlines
                .iter()
                .map(|outline| (outline.id, outline.layout.clone(), outline.document.clone()))
                .collect::<Vec<_>>()
        };
        let mut steps = Vec::new();
        let mut random = 0x29a7_u32;
        for step in 0..80 {
            random = random.wrapping_mul(1664525).wrapping_add(1013904223);
            let index = random as usize % editor.outlines.len();
            let id = editor.outlines[index].id;
            editor.focus_outline(id).unwrap();
            let before = snapshot(&editor);
            let undo_count = editor.undo.len();
            match step % 5 {
                0 if editor.outlines.len() < 8 => {
                    editor
                        .create_outline(
                            &mut engine,
                            [step as f32 * 9.0 - 100.0, step as f32 * 3.0],
                            72.0 + step as f32,
                        )
                        .unwrap();
                }
                1 => {
                    editor
                        .insert(
                            &mut engine,
                            ["x", "é", "🌳", "a\nשלום"][random as usize % 4],
                        )
                        .unwrap();
                }
                2 => {
                    editor.delete(&mut engine, random & 1 == 0).unwrap();
                }
                3 => {
                    editor
                        .compose(&mut engine, "e\u{301}".into(), 2..2)
                        .unwrap();
                    editor.finish_composition();
                }
                _ => {
                    let [x, y] = editor.active_outline().origin();
                    editor.move_outline(id, [x + 7.0, y - 3.0]).unwrap();
                }
            }
            if editor.undo.len() == undo_count {
                assert_eq!(snapshot(&editor), before);
                continue;
            }
            assert_eq!(editor.undo.len(), undo_count + 1);
            steps.push((before, snapshot(&editor)));
            for outline in &editor.outlines {
                let rebuilt = TextOutline::new(
                    &mut engine,
                    outline.document.clone(),
                    outline.layout.max_width.unwrap(),
                    outline.origin(),
                )
                .unwrap();
                assert_eq!(ys(outline), ys(&rebuilt));
                for (actual, expected) in outline
                    .shaped
                    .paragraphs
                    .iter()
                    .zip(&rebuilt.shaped.paragraphs)
                {
                    assert_eq!(actual.projection.text(), expected.projection.text());
                    assert_eq!(actual.text.height(), expected.text.height());
                    assert_eq!(
                        actual
                            .text
                            .lines()
                            .map(|(_, line)| (line.source.clone(), line.baseline))
                            .collect::<Vec<_>>(),
                        expected
                            .text
                            .lines()
                            .map(|(_, line)| (line.source.clone(), line.baseline))
                            .collect::<Vec<_>>()
                    );
                }
            }
        }
        assert!(steps.len() > 60);
        for (before, _) in steps.iter().rev() {
            assert!(editor.undo(&mut engine).unwrap());
            assert_eq!(&snapshot(&editor), before);
        }
        assert!(!editor.undo(&mut engine).unwrap());
        for (_, after) in &steps {
            assert!(editor.redo(&mut engine).unwrap());
            assert_eq!(&snapshot(&editor), after);
        }
        assert!(!editor.redo(&mut engine).unwrap());
    }

    const CORPUS: [&[u8]; 3] = [
        include_bytes!("../../../corpus/canvas/baseline-anchors.one"),
        include_bytes!("../../../corpus/outline-edit/before/notebook/synthetic.one"),
        include_bytes!("../../../corpus/paragraph-edit/before/notebook/synthetic.one"),
    ];

    fn corpus_pages(section: &[u8]) -> Vec<(ExGuid, Page)> {
        use onestore::{RevisionIndex, Store, document::Document};
        let store = Store::parse(section).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let document = Document::parse(&index).unwrap();
        let mut spaces = document
            .pages()
            .unwrap()
            .into_iter()
            .map(|(space, _)| space)
            .collect::<Vec<_>>();
        spaces.dedup();
        spaces
            .into_iter()
            .map(|space| (space, Page::from_space(&document, space).unwrap()))
            .collect()
    }

    fn corpus_page(section: &[u8], title: &str) -> (ExGuid, Page) {
        corpus_pages(section)
            .into_iter()
            .find(|(_, page)| page.title == title)
            .unwrap()
    }

    fn body_text(page: &Page, id: ExGuid) -> Option<String> {
        page.objects.iter().find_map(|object| match object {
            PageObject::Outline(outline) if outline.id == id => Some(
                outline
                    .paragraphs
                    .iter()
                    .map(|paragraph| paragraph.text().unwrap().text.text())
                    .collect(),
            ),
            _ => None,
        })
    }

    #[test]
    fn an_unedited_page_rebuilds_into_the_model_it_was_imported_from() {
        let mut pages = 0;
        for section in CORPUS {
            for (_, page) in corpus_pages(section) {
                let mut engine = TextEngine::default();
                let editor = CanvasEditor::from_page(page.clone(), &mut engine).unwrap();
                assert_eq!(editor.page().unwrap(), page);
                pages += 1;
            }
        }
        assert_eq!(pages, 30);
    }

    #[test]
    fn typing_moving_resizing_adding_and_deleting_reach_the_rebuilt_page() {
        let mut engine = TextEngine::default();
        let (_, source) = corpus_page(CORPUS[0], "Baseline anchors");
        let mut editor = CanvasEditor::from_page(source.clone(), &mut engine).unwrap();
        let removed = editor
            .outlines
            .iter()
            .find(|outline| {
                let [node] = outline.document.nodes() else {
                    return false;
                };
                !outline.title
                    && node.lists.is_empty()
                    && node.tags.is_empty()
                    && node.text().is_some_and(|text| text.tags.is_empty())
            })
            .unwrap()
            .id;
        let bodies = editor
            .outlines
            .iter()
            .filter(|outline| !outline.title && outline.id != removed)
            .map(|outline| outline.id)
            .collect::<Vec<_>>();
        editor.focus_outline(bodies[0]).unwrap();
        editor.insert(&mut engine, "typed").unwrap();
        editor.move_outline(bodies[1], [123.0, 456.0]).unwrap();
        editor.focus_outline(bodies[2]).unwrap();
        editor.resize(&mut engine, 200.0).unwrap();
        editor.focus_outline(removed).unwrap();
        editor.select_all().unwrap();
        assert!(editor.delete(&mut engine, false).unwrap());
        let added = editor
            .create_outline(&mut engine, [24.0, 600.0], 300.0)
            .unwrap();
        editor.insert(&mut engine, "added").unwrap();

        let page = editor.page().unwrap();
        assert_eq!(page.title, source.title);
        assert_eq!(page.created, source.created);
        assert_eq!(page.margin_origin, source.margin_origin);
        assert!(body_text(&page, bodies[0]).unwrap().starts_with("typed"));
        let moved = page
            .objects
            .iter()
            .find_map(|object| match object {
                PageObject::Outline(outline) if outline.id == bodies[1] => Some(&outline.layout),
                _ => None,
            })
            .unwrap();
        assert_eq!([moved.x, moved.y], [Some(123.0), Some(456.0)]);
        let resized = page
            .objects
            .iter()
            .find_map(|object| match object {
                PageObject::Outline(outline) if outline.id == bodies[2] => Some(&outline.layout),
                _ => None,
            })
            .unwrap();
        assert_eq!(resized.max_width, Some(200.0));
        assert_eq!(resized.width_set_by_user, Some(true));
        assert!(body_text(&page, removed).is_none());
        assert_eq!(body_text(&page, added).as_deref(), Some("added"));
        assert_eq!(page.objects.last().unwrap().id(), added);
        assert_eq!(
            page.objects.iter().map(PageObject::id).collect::<Vec<_>>(),
            source
                .objects
                .iter()
                .map(PageObject::id)
                .filter(|id| *id != removed)
                .chain([added])
                .collect::<Vec<_>>()
        );
        assert_eq!(
            page.objects
                .iter()
                .find(|object| matches!(object, PageObject::Title(_))),
            source
                .objects
                .iter()
                .find(|object| matches!(object, PageObject::Title(_)))
        );

        let mut engine = TextEngine::default();
        let reimported = CanvasEditor::from_page(page.clone(), &mut engine).unwrap();
        assert_eq!(reimported.page().unwrap(), page);
    }

    #[test]
    fn an_edited_page_writes_back_through_the_page_writer() {
        let mut engine = TextEngine::default();
        let (space, source) = corpus_page(CORPUS[2], "Split middle");
        let mut editor = CanvasEditor::from_page(source.clone(), &mut engine).unwrap();
        let body = editor
            .outlines
            .iter()
            .find(|outline| !outline.title)
            .unwrap()
            .id;
        editor.focus_outline(body).unwrap();
        editor.insert(&mut engine, "Edited ").unwrap();
        let page = editor.page().unwrap();
        let written = onestore::PreparedEdit::page(CORPUS[2], space, &page, "Author").unwrap();
        let reread = corpus_pages(written.as_bytes())
            .into_iter()
            .find_map(|(candidate, page)| (candidate == space).then_some(page))
            .unwrap();
        let edited = body_text(&page, body).unwrap();
        assert!(edited.starts_with("Edited "));
        assert_eq!(body_text(&reread, body).as_deref(), Some(edited.as_str()));
        assert_ne!(body_text(&source, body).as_deref(), Some(edited.as_str()));
    }

    /// A page changed elsewhere shows in place: the caret keeps its paragraph and moves
    /// past text inserted before it; history goes, unless nothing changed.
    #[test]
    fn a_refresh_keeps_the_caret_by_identity() {
        let mut engine = TextEngine::default();
        let lines =
            ["Hello world", "Second"].map(|line| Paragraph::new(line.into(), Format::default()));
        let document = TextDocument::new(lines.to_vec()).unwrap();
        let mut editor = CanvasEditor::new(&mut engine, document, 400.0).unwrap();
        editor
            .select(
                [TextPosition {
                    paragraph: 1,
                    offset: 3,
                }; 2]
                    .into(),
            )
            .unwrap();
        editor.insert(&mut engine, "x").unwrap();
        editor
            .select(
                [TextPosition {
                    paragraph: 0,
                    offset: 8,
                }; 2]
                    .into(),
            )
            .unwrap();
        let _ = editor.take_ops();
        let page = editor.page().unwrap();
        assert!(!editor.refresh(page.clone(), &mut engine).unwrap());
        assert!(!editor.undo.is_empty(), "an unchanged page keeps history");

        let mut remote = page;
        let PageObject::Outline(outline) = &mut remote.objects[0] else {
            unreachable!()
        };
        for (paragraph, text) in outline
            .paragraphs
            .iter_mut()
            .zip(["Hey, Hello world", "Other"])
        {
            paragraph.text_mut().unwrap().text = Paragraph::new(text.into(), Format::default());
        }
        let (shown, affinities) = (editor.active_outline().id, editor.selection().affinities);
        assert!(editor.refresh(remote, &mut engine).unwrap());
        assert_eq!(editor.active_outline().id, shown);
        assert_eq!(
            editor.selection(),
            Selection {
                positions: [TextPosition {
                    paragraph: 0,
                    offset: 13
                }; 2],
                affinities,
            }
        );
        assert!(editor.undo.is_empty());
        assert_eq!(editor.take_ops().unwrap(), []);
    }
}
