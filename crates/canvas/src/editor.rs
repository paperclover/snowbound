use crate::{
    document::{DocumentEdit, TextDocument, TextPosition},
    layout::{LayoutError, TextEngine},
    outline::{OutlineLayout, ParagraphLayout, arrange, visible_paragraphs},
    page::{Definition, Outline},
    text::{EditError, Paragraph},
};
use onestore::ExGuid;
use parley::{
    Affinity, BoundingBox,
    editing::{Cursor, Selection as ParagraphSelection},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    ops::Range,
};

#[derive(Clone, Copy, Debug)]
pub enum Movement {
    Left,
    Right,
    Up,
    Down,
    WordLeft,
    WordRight,
    LineStart,
    LineEnd,
}

#[derive(Debug)]
pub enum EditorError {
    Edit(EditError),
    Layout(LayoutError),
}

impl fmt::Display for EditorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Edit(error) => error.fmt(f),
            Self::Layout(error) => error.fmt(f),
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
    outlines: Vec<TextOutline>,
    definitions: BTreeMap<ExGuid, Definition>,
    active: Focus,
    undo: Vec<History>,
    redo: Vec<History>,
    composition: Option<Composition>,
    preferred_x: Option<f32>,
}

enum Focus {
    Outline(usize),
    Caret {
        outline: Box<TextOutline>,
        index: usize,
    },
}

pub struct TextOutline {
    pub id: onestore::ExGuid,
    layout: onestore::document::Layout,
    document: TextDocument,
    indents: Vec<f32>,
    shaped: OutlineLayout,
    selection: Selection,
}

impl TextOutline {
    /// A single plain paragraph containing only ASCII spaces or no text.
    pub fn is_empty(&self) -> bool {
        self.document.nodes().len() == 1
            && self.document.validate_flat().is_ok()
            && self
                .document
                .paragraphs()
                .next()
                .unwrap()
                .text()
                .bytes()
                .all(|byte| byte == b' ')
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
        let shaped = visible_paragraphs(document.nodes())
            .map(|paragraph| {
                ParagraphLayout::shape(engine, paragraph, width, &indents, &BTreeMap::new())
            })
            .collect::<Result<Vec<_>, _>>()?;
        let shaped = OutlineLayout::new(shaped, width, true)?;
        Ok(Self {
            id: crate::text::new_id()?,
            layout: onestore::document::Layout {
                x: Some(position[0]),
                y: Some(position[1]),
                max_width: Some(width),
                width_set_by_user: Some(true),
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
        let shaped = outline.layout(engine, definitions)?;
        let document = TextDocument::from_nodes(outline.paragraphs.clone())?;
        Ok(Self {
            id: outline.id,
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

    fn snapshot(&self) -> Outline {
        Outline {
            id: self.id,
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

    pub fn origin(&self) -> [f32; 2] {
        [self.layout.x.unwrap_or(0.0), self.layout.y.unwrap_or(0.0)]
    }

    pub fn bounds(&self) -> BoundingBox {
        let [x, y] = self.origin().map(f64::from);
        BoundingBox {
            x0: x,
            y0: y,
            x1: x + f64::from(self.shaped.size[0]),
            y1: y + f64::from(self.shaped.size[1]),
        }
    }

    pub fn layouts(&self) -> impl Iterator<Item = (usize, &ParagraphLayout)> {
        let mut shaped = self.shaped.paragraphs.iter().peekable();
        self.document
            .nodes()
            .iter()
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

    fn visible_index(&self, source: usize) -> Result<usize, EditError> {
        let id = self
            .document
            .nodes()
            .get(source)
            .ok_or(EditError::InvalidRange)?
            .id;
        self.shaped
            .paragraphs
            .iter()
            .position(|paragraph| paragraph.id == id)
            .ok_or(EditError::InvalidRange)
    }

    fn source_index(&self, visible: usize) -> usize {
        let id = self.shaped.paragraphs[visible].id;
        self.document
            .nodes()
            .iter()
            .position(|paragraph| paragraph.id == id)
            .unwrap()
    }

    pub fn paragraph_layout(&self, source: usize) -> Result<&ParagraphLayout, EditError> {
        Ok(&self.shaped.paragraphs[self.visible_index(source)?])
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
    Text {
        outline: onestore::ExGuid,
        change: TextChange,
    },
    Position {
        outline: onestore::ExGuid,
        position: [Option<f32>; 2],
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
            active,
            undo: Vec::new(),
            redo: Vec::new(),
            composition: None,
            preferred_x: None,
        })
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
        Self::from_text_outlines(outlines, definitions)
    }

    pub fn from_text_outlines(
        outlines: Vec<TextOutline>,
        definitions: BTreeMap<ExGuid, Definition>,
    ) -> Result<Self, EditorError> {
        if outlines.is_empty() {
            return Err(EditError::InvalidRange.into());
        }
        let mut ids = BTreeSet::new();
        for outline in &outlines {
            for id in
                std::iter::once(outline.id).chain(
                    outline.document.nodes().iter().flat_map(|p| {
                        std::iter::once(p.id).chain(p.text.iter().map(|text| text.id))
                    }),
                )
            {
                if !ids.insert(id) {
                    return Err(EditError::InvalidStructure.into());
                }
            }
        }
        Ok(Self {
            outlines,
            definitions,
            active: Focus::Outline(0),
            undo: Vec::new(),
            redo: Vec::new(),
            composition: None,
            preferred_x: None,
        })
    }

    pub fn outlines(&self) -> &[TextOutline] {
        &self.outlines
    }
    pub fn active_outline(&self) -> &TextOutline {
        match &self.active {
            Focus::Outline(index) => &self.outlines[*index],
            Focus::Caret { outline, .. } => outline,
        }
    }

    fn active_outline_mut(&mut self) -> &mut TextOutline {
        match &mut self.active {
            Focus::Outline(index) => &mut self.outlines[*index],
            Focus::Caret { outline, .. } => outline,
        }
    }

    /// Provisional input geometry outside the document's outline collection.
    pub fn caret_outline(&self) -> Option<&TextOutline> {
        match &self.active {
            Focus::Caret { outline, .. } => Some(outline),
            Focus::Outline(_) => None,
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

    pub fn focus_outline(&mut self, id: onestore::ExGuid) -> Result<(), EditError> {
        if self.active_outline().id != id && !self.outlines.iter().any(|outline| outline.id == id) {
            return Err(EditError::InvalidRange);
        }
        self.finish_composition();
        if let Some(index) = self.outlines.iter().position(|outline| outline.id == id) {
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
                Focus::Outline(index) => RestoreFocus::Outline(self.outlines[*index].id),
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
        self.undo.push(History::Position {
            outline: id,
            position: [layout.x, layout.y],
        });
        self.redo.clear();
        layout.x = Some(position[0]);
        layout.y = Some(position[1]);
        self.active = Focus::Outline(index);
        self.preferred_x = None;
        Ok(())
    }

    pub fn selection(&self) -> Selection {
        self.active_outline().selection
    }

    pub fn resize(&mut self, engine: &mut TextEngine, width: f32) -> Result<(), EditorError> {
        let outline = self.active_outline();
        if outline.layout.reserved_width.or(outline.layout.max_width) == Some(width)
            && outline.layout.width_set_by_user == Some(true)
        {
            return Ok(());
        }
        let paragraphs = visible_paragraphs(outline.document.nodes())
            .map(|paragraph| {
                ParagraphLayout::shape(
                    engine,
                    paragraph,
                    width,
                    &outline.indents,
                    &self.definitions,
                )
            })
            .collect::<Result<_, _>>()?;
        let shaped = OutlineLayout::new(paragraphs, width, true)?;
        let outline = self.active_outline_mut();
        outline.layout.max_width = Some(width);
        outline.layout.reserved_width = None;
        outline.layout.width_set_by_user = Some(true);
        outline.shaped = shaped;
        self.preferred_x = None;
        Ok(())
    }

    pub fn select(&mut self, selection: Selection) -> Result<(), EditError> {
        for position in selection.positions {
            self.active_outline().visible_index(position.paragraph)?;
            self.active_outline()
                .document
                .paragraphs()
                .nth(position.paragraph)
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
        let last = self
            .active_outline()
            .document
            .paragraphs()
            .nth(paragraph)
            .unwrap();
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
        if !x.is_finite() || !y.is_finite() {
            return Err(EditError::InvalidRange);
        }
        let index = self
            .active_outline()
            .shaped
            .paragraphs
            .partition_point(|paragraph| paragraph.origin[1] <= y)
            .saturating_sub(1);
        let paragraph = &self.active_outline().shaped.paragraphs[index];
        let cursor = paragraph
            .text
            .hit_test(x - paragraph.origin[0], y - paragraph.origin[1]);
        let visible = paragraph.projection.text().utf16_offset(cursor.index())?;
        let position = TextPosition {
            paragraph: self.active_outline().source_index(index),
            offset: paragraph
                .projection
                .source_offset(visible, cursor.affinity())?,
        };
        self.finish_composition();
        self.preferred_x = None;
        if !extend {
            self.active_outline_mut().selection.positions[0] = position;
            self.active_outline_mut().selection.affinities[0] = cursor.affinity();
        }
        self.active_outline_mut().selection.positions[1] = position;
        self.active_outline_mut().selection.affinities[1] = cursor.affinity();
        Ok(())
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

    pub fn move_selection(&mut self, movement: Movement, extend: bool) -> Result<(), EditError> {
        self.finish_composition();
        let [anchor, focus] = self.active_outline().selection.positions;
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
            let target = if matches!(movement, Movement::Up) {
                if line > 0 {
                    paragraph.text.lines().nth(line - 1).map(|(_, line)| line)
                } else if index > 0 {
                    index -= 1;
                    self.active_outline().shaped.paragraphs[index]
                        .text
                        .lines()
                        .last()
                        .map(|(_, line)| line)
                } else {
                    None
                }
            } else if let Some((_, line)) = paragraph.text.lines().nth(line + 1) {
                Some(line)
            } else if index + 1 < self.active_outline().shaped.paragraphs.len() {
                index += 1;
                self.active_outline().shaped.paragraphs[index]
                    .text
                    .lines()
                    .next()
                    .map(|(_, line)| line)
            } else {
                None
            };
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
            let layout = &paragraph.text.shaped;
            next = match movement {
                Movement::Left => local.previous_visual(layout, extend),
                Movement::Right => local.next_visual(layout, extend),
                Movement::WordLeft => local.previous_visual_word(layout, extend),
                Movement::WordRight => local.next_visual_word(layout, extend),
                Movement::LineStart => local.line_start(layout, extend),
                Movement::LineEnd => local.line_end(layout, extend),
                Movement::Up | Movement::Down => unreachable!(),
            }
            .focus();
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
                        next = if left {
                            next.previous_visual_word(&paragraph.text.shaped)
                        } else {
                            next.next_visual_word(&paragraph.text.shaped)
                        };
                    }
                }
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
                .source_offset(visible, next.affinity())?,
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
        for (index, paragraph) in self
            .active_outline()
            .layouts()
            .filter(|(index, _)| *index >= start.paragraph && *index <= end.paragraph)
        {
            let source = self
                .active_outline()
                .document
                .paragraphs()
                .nth(index)
                .unwrap();
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
        let format = self
            .active_outline()
            .document
            .paragraphs()
            .nth(start.paragraph)
            .unwrap()
            .format_at(start.offset)?;
        let replacement = text
            .split('\n')
            .map(|line| Paragraph::new(line.to_owned(), format.clone()))
            .collect();
        self.replace(engine, replacement)
    }

    pub fn delete(&mut self, engine: &mut TextEngine, backward: bool) -> Result<bool, EditorError> {
        let [anchor, focus] = self.active_outline().selection.positions;
        let mut range = anchor.min(focus)..anchor.max(focus);
        if range.is_empty() {
            let paragraph = self.active_outline().paragraph_layout(focus.paragraph)?;
            let cursor =
                paragraph.cursor(focus.offset, self.active_outline().selection.affinities[1])?;
            if let Some(cluster) =
                cursor.logical_clusters(&paragraph.text.shaped)[usize::from(!backward)]
            {
                let visible = paragraph.projection.text();
                let bytes = cluster.text_range();
                range.start.offset = paragraph
                    .projection
                    .source_offset(visible.utf16_offset(bytes.start)?, Affinity::Downstream)?;
                range.end.offset = paragraph
                    .projection
                    .source_offset(visible.utf16_offset(bytes.end)?, Affinity::Upstream)?;
            } else if backward && focus.paragraph > 0 {
                let previous = self
                    .active_outline()
                    .document
                    .paragraphs()
                    .nth(focus.paragraph - 1)
                    .unwrap();
                range.start = TextPosition {
                    paragraph: focus.paragraph - 1,
                    offset: previous.utf16_offset(previous.text().len())?,
                };
                range.end.offset = 0;
            } else if !backward
                && focus.paragraph + 1 < self.active_outline().document.nodes().len()
            {
                let current = self
                    .active_outline()
                    .document
                    .paragraphs()
                    .nth(focus.paragraph)
                    .unwrap();
                range.start.offset = current.utf16_offset(current.text().len())?;
                range.end = TextPosition {
                    paragraph: focus.paragraph + 1,
                    offset: 0,
                };
            } else {
                return Ok(false);
            }
        }
        let caret = range.start;
        let format = self
            .active_outline()
            .document
            .paragraphs()
            .nth(caret.paragraph)
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
        let valid = match &history {
            History::Text { outline, .. } | History::Position { outline, .. } => {
                self.outlines.iter().any(|item| item.id == *outline)
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
            History::Text { outline, change } => {
                let index = self
                    .outlines
                    .iter()
                    .position(|item| item.id == outline)
                    .unwrap();
                let previous = std::mem::replace(&mut self.active, Focus::Outline(index));
                let inverse = match self.apply(engine, change.edit.clone(), change.selection, false)
                {
                    Ok(change) => change,
                    Err(error) => {
                        self.active = previous;
                        return Err((History::Text { outline, change }, error));
                    }
                };
                History::Text {
                    outline,
                    change: inverse,
                }
            }
            History::Position { outline, position } => {
                let index = self
                    .outlines
                    .iter()
                    .position(|item| item.id == outline)
                    .unwrap();
                let layout = &mut self.outlines[index].layout;
                let inverse = History::Position {
                    outline,
                    position: [layout.x, layout.y],
                };
                [layout.x, layout.y] = position;
                self.active = Focus::Outline(index);
                inverse
            }
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
        let format = self
            .active_outline()
            .document
            .paragraphs()
            .nth(range.start.paragraph)
            .unwrap()
            .format_at(range.start.offset)?;
        let replacement = text
            .split('\n')
            .map(|part| Paragraph::new(part.to_owned(), format.clone()))
            .collect();
        let edit = self
            .active_outline()
            .document
            .replace(range.clone(), replacement)?;
        let inverse = self.apply(
            engine,
            edit,
            Selection {
                positions,
                affinities: [Affinity::Downstream; 2],
            },
            false,
        )?;
        let original = if let Some(composition) = self.composition.take() {
            let mut original = composition.original;
            original.edit.range = inverse.edit.range;
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
        self.apply(engine, original.edit, original.selection, false)?;
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
        edit: DocumentEdit,
        selection: Selection,
    ) -> Result<(), EditorError> {
        let inverse = self.apply(engine, edit, selection, true)?;
        self.record_change(inverse);
        Ok(())
    }

    fn record_change(&mut self, change: TextChange) {
        if self.caret_outline().is_some() {
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
            self.undo.push(History::Remove {
                outline: outline.id,
                focus: RestoreFocus::Caret {
                    source: Box::new(source),
                    selection: change.selection,
                    index,
                },
            });
            self.outlines.insert(index, *outline);
        } else if self.active_outline().is_empty() {
            let Focus::Outline(index) = self.active else {
                unreachable!()
            };
            let outline = self.outlines.remove(index);
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
            self.undo.push(History::Text {
                outline: self.active_outline().id,
                change,
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
    ) -> Result<TextChange, EditorError> {
        let outline = self.active_outline();
        outline.document.validate_edit(&edit)?;
        let width = outline
            .layout
            .reserved_width
            .or(outline.layout.max_width)
            .unwrap();
        let visible_start = outline
            .layouts()
            .take_while(|(i, _)| *i < edit.range.start)
            .count();
        let visible_end = outline
            .layouts()
            .take_while(|(i, _)| *i < edit.range.end)
            .count();
        if visible_end - visible_start != edit.range.len() {
            return Err(EditError::UnsupportedContent.into());
        }
        let shaped = edit
            .replacement
            .iter()
            .map(|paragraph| {
                ParagraphLayout::shape(
                    engine,
                    paragraph,
                    width,
                    &outline.indents,
                    &self.definitions,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        let proposed_layouts = || {
            outline.shaped.paragraphs[..visible_start]
                .iter()
                .chain(&shaped)
                .chain(&outline.shaped.paragraphs[visible_end..])
        };
        let (origins, size) = arrange(
            proposed_layouts(),
            width,
            outline.layout.width_set_by_user == Some(true),
        )?;
        for position in selection.positions {
            let node = outline.document.nodes()[..edit.range.start]
                .iter()
                .chain(&edit.replacement)
                .chain(&outline.document.nodes()[edit.range.end..])
                .nth(position.paragraph)
                .ok_or(EditError::InvalidRange)?;
            node.text[0].text.byte_offset(position.offset)?;
            if !proposed_layouts().any(|paragraph| paragraph.id == node.id) {
                return Err(EditError::InvalidRange.into());
            }
        }
        if finish_composition {
            self.finish_composition();
        }
        let outline = self.active_outline_mut();
        let inverse = outline.document.apply(edit)?;
        outline
            .shaped
            .paragraphs
            .splice(visible_start..visible_end, shaped);
        for (paragraph, y) in outline.shaped.paragraphs.iter_mut().zip(origins) {
            paragraph.origin[1] = y;
        }
        outline.shaped.size = size;
        self.preferred_x = None;
        let previous_selection =
            std::mem::replace(&mut self.active_outline_mut().selection, selection);
        Ok(TextChange {
            edit: inverse,
            selection: previous_selection,
        })
    }
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
        let mut editor = CanvasEditor::from_text_outlines(outlines, BTreeMap::new()).unwrap();
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
            let mut editor = CanvasEditor::from_text_outlines(outlines, BTreeMap::new()).unwrap();
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
        use crate::page::{Page, PageObject};
        use onestore::{RevisionIndex, Store, document::Document};
        let store = Store::parse(include_bytes!(
            "../../../resources/canvas/baseline-anchors.one"
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
                    if outline.paragraphs[0].combined_text().text() == "Spacing first" =>
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
            id: crate::text::new_id().unwrap(),
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
        let marker = crate::text::new_id().unwrap();
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
            id: crate::text::new_id().unwrap(),
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
        assert!(editor.insert(&mut engine, "\n").is_err());
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
        editor.move_selection(Movement::Right, false).unwrap();
        assert_eq!(editor.selection().positions[1].paragraph, 4);
        editor.move_selection(Movement::Left, false).unwrap();
        assert_eq!(editor.selection().positions[1].paragraph, 2);
        editor.move_selection(Movement::Down, false).unwrap();
        assert_eq!(editor.selection().positions[1].paragraph, 4);
        editor.move_selection(Movement::Up, false).unwrap();
        assert_eq!(editor.selection().positions[1].paragraph, 2);
        editor.select_all().unwrap();
        assert!(!editor.selection_rects().unwrap().is_empty());
    }

    #[test]
    #[ignore = "requires CANVAS_TEST_SECTION and CANVAS_TEST_PAGE private fixture inputs"]
    fn imported_editor_reflows_and_restores_native_outline_geometry() {
        use crate::page::{Page, PageObject};
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
                let text = &nodes[paragraph].text[0].text;
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
            editor.move_selection(Movement::Right, false).unwrap();
            if editor.selection().positions[1] == last {
                break;
            }
        }
        assert_eq!(editor.selection().positions[1], last);
        for _ in 0..32 {
            editor.move_selection(Movement::Left, false).unwrap();
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
        assert_eq!(editor.active_outline().document, original);
        assert_eq!(editor.selection().positions, [anchor, focus]);
        editor.redo(&mut engine).unwrap();
        assert_eq!(editor.active_outline().document, after);
        assert_eq!(editor.selection().positions[1].offset, 4);
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
            editor.move_selection(movement, false).unwrap();
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
        editor.resize(&mut engine, 25.0).unwrap();
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
        editor.move_selection(Movement::Down, false).unwrap();
        assert_eq!(
            editor.selection().positions[1],
            TextPosition {
                paragraph: 1,
                offset: 0
            }
        );
        editor.move_selection(Movement::Down, false).unwrap();
        assert_eq!(
            editor.selection().positions[1],
            TextPosition {
                paragraph: 2,
                offset: 8
            }
        );
        assert_eq!(editor.caret(1.0).unwrap().x0, x);
        editor.move_selection(Movement::Up, true).unwrap();
        editor.move_selection(Movement::Up, true).unwrap();
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
        editor.move_selection(Movement::Left, false).unwrap();
        assert_eq!(editor.selection().positions, [start; 2]);
        let boundary = TextPosition {
            paragraph: 1,
            offset: 0,
        };
        editor.select([boundary, boundary].into()).unwrap();
        editor.move_selection(Movement::Left, false).unwrap();
        assert_eq!(
            editor.selection().positions[1],
            TextPosition {
                paragraph: 0,
                offset: 10
            }
        );
        editor.move_selection(Movement::Right, false).unwrap();
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
        editor.resize(&mut engine, 30.0).unwrap();
        let rebuilt =
            CanvasEditor::new(&mut engine, editor.active_outline().document.clone(), 30.0).unwrap();
        assert_eq!(ys(editor.active_outline()), ys(rebuilt.active_outline()));
        assert_eq!(editor.selection().positions, selection);
        assert!(editor.resize(&mut engine, f32::NAN).is_err());
        assert_eq!(ys(editor.active_outline()), ys(rebuilt.active_outline()));
        editor.undo(&mut engine).unwrap();
        let rebuilt =
            CanvasEditor::new(&mut engine, editor.active_outline().document.clone(), 30.0).unwrap();
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
}
