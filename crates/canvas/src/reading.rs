//! Reading view: a page's content in reading order, reflowed into one column as wide as a
//! phone's screen. The reflowed page is only shown; nothing of it is stored.
//!
//! Blocks are the page's outlines, pictures, files and drawings, as laid out. Blocks whose
//! lines or pictures overlap, ink over text, and strokes within reach of each other join
//! into one piece moved whole. Reading order is a recursive XY cut: rows split by horizontal
//! gaps, then columns by vertical gaps, left column first. Notes beside one long outline are
//! asides: the outline splits after the paragraph each note sits beside, and the note follows
//! it. Text rewraps to the column, pictures shrink to it, and everything else keeps its size.

use crate::{
    editor::{CanvasEditor, EditorError, TextOutline, page::Content},
    layout::TextEngine,
};
use onestore::{
    ExGuid,
    page::{Outline, Page, PageObject, ParagraphContent},
};
use std::{collections::BTreeMap, ops::Range};

/// Points between blocks in the column: OneNote's grid row.
const GAP: f32 = 18.0;
/// Strokes this near one another are one drawing, as handwriting is many strokes.
const INK_REACH: f32 = 9.0;
/// The share of the content's area drawn, or kept as laid out, past which the page is a
/// canvas rather than a document.
const CANVAS_SHARE: f64 = 0.5;
/// How far below a paragraph's top a note beside it may start and still follow it.
const ASIDE_SLACK: f32 = 6.0;
/// How far an aside sits in from the column's edge, and the room above and below it.
const ASIDE_INDENT: f32 = 18.0;
const ASIDE_GAP: f32 = 6.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    Text,
    Picture,
    File,
    Drawing,
    /// Overlapping blocks or ink over text, moved as one piece.
    Group,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub kind: Kind,
    /// The page objects it moves.
    pub members: Vec<ExGuid>,
    /// `[x0, y0, x1, y1]` in page points, as laid out on the page.
    pub bounds: [f32; 4],
    /// The outline's top-level paragraphs it shows, where asides split the outline.
    pub part: Option<Range<usize>>,
    /// A note beside an outline, read after the paragraph it sits by.
    pub aside: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// No wider than the column already: shown as laid out.
    Fits,
    /// Every block flows into the column.
    Reflows,
    /// Flows, with what each `Kept` names read in an order or shape the page decides.
    Partial(Vec<Kept>),
    /// Not offered.
    Refused(Refusal),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kept {
    /// Notes beside an outline, each placed after the paragraph it sits beside.
    Asides,
    /// Blocks side by side, or interlocked, read left column first.
    SideBySide,
    /// Overlapping blocks or ink over text, moved whole at their size.
    Group,
    /// Something still wider than the column: a table with set column widths, a drawing, a
    /// group.
    Wide,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Refusal {
    /// Its positions read from the right; not yet reflowed.
    RightToLeft,
    /// Mostly ink.
    Drawn,
    /// Mostly overlapping blocks or ink over text.
    Arranged,
}

pub struct Reading {
    pub verdict: Verdict,
    /// In reading order where the page reflows, else as found.
    pub blocks: Vec<Block>,
    /// The page reflowed, where it reflows.
    pub page: Option<Page>,
}

impl Verdict {
    /// Whether the page has a reading view.
    pub fn offered(&self) -> bool {
        matches!(self, Self::Reflows | Self::Partial(_))
    }
}

/// `page` read into a column `column` points wide.
pub fn read(page: &Page, column: f32, engine: &mut TextEngine) -> Result<Reading, EditorError> {
    let editor = CanvasEditor::from_page(page.clone(), engine)?;
    let blocks = blocks(&editor);
    let refused = |refusal, blocks| Reading {
        verdict: Verdict::Refused(refusal),
        blocks,
        page: None,
    };
    if page.rtl {
        return Ok(refused(Refusal::RightToLeft, blocks));
    }
    let left = blocks
        .iter()
        .map(|b| b.bounds[0])
        .fold(f32::INFINITY, f32::min);
    let right = blocks
        .iter()
        .map(|b| b.bounds[2])
        .fold(f32::NEG_INFINITY, f32::max);
    if blocks.is_empty() || right - left <= column + 1.0 {
        return Ok(Reading {
            verdict: Verdict::Fits,
            blocks,
            page: None,
        });
    }
    let area = |kind: Option<Kind>| -> f64 {
        blocks
            .iter()
            .filter(|b| kind.is_none_or(|kind| b.kind == kind))
            .map(|b| f64::from(b.bounds[2] - b.bounds[0]) * f64::from(b.bounds[3] - b.bounds[1]))
            .sum()
    };
    let total = area(None).max(1.0);
    if area(Some(Kind::Drawing)) / total >= CANVAS_SHARE {
        return Ok(refused(Refusal::Drawn, blocks));
    }
    if area(Some(Kind::Group)) / total >= CANVAS_SHARE {
        return Ok(refused(Refusal::Arranged, blocks));
    }
    let mut kept = Vec::new();
    if blocks.iter().any(|b| b.kind == Kind::Group) {
        kept.push(Kept::Group);
    }
    let tops: BTreeMap<ExGuid, Vec<f32>> = editor
        .outlines()
        .iter()
        .map(|outline| (outline.id, root_tops(outline)))
        .collect();
    let blocks = order(blocks, &tops, &mut kept);
    let start = editor.body_start().unwrap_or([left, blocks[0].bounds[1]]);
    let mut reflowed = page.clone();
    let blocks = split(&mut reflowed, blocks);
    reflow(
        &mut reflowed,
        &blocks,
        [start[0].min(left), start[1]],
        column,
        engine,
        &mut kept,
    )?;
    kept.sort();
    kept.dedup();
    Ok(Reading {
        verdict: if kept.is_empty() {
            Verdict::Reflows
        } else {
            Verdict::Partial(kept)
        },
        blocks,
        page: Some(reflowed),
    })
}

/// Where `outline`'s lines, table cells and pictures lie, in page points.
fn marks(outline: &TextOutline) -> Vec<[f32; 4]> {
    let [x, y] = outline.origin();
    let shaped = outline.shaped();
    let lines = outline.layouts().flat_map(|(_, paragraph)| {
        let [px, py] = paragraph.origin;
        paragraph.text.lines().map(move |(line, bounds)| {
            let metrics = line.metrics();
            let left = x + px + metrics.offset;
            let top = y + py + bounds.top;
            [
                left,
                top,
                left + metrics.advance - metrics.trailing_whitespace,
                top + bounds.height,
            ]
        })
    });
    let cells = shaped
        .tables
        .iter()
        .flat_map(|table| &table.cells)
        .map(|cell| cell.rect);
    let objects = shaped.objects.iter().map(|object| object.rect);
    lines
        .chain(
            cells
                .chain(objects)
                .map(|rect| crate::translated(rect, [x, y])),
        )
        .filter(|[x0, y0, x1, y1]| x1 > x0 && y1 > y0)
        .collect()
}

/// The page top of each of `outline`'s top-level paragraphs: its own text's, or the first
/// shown below it; the one before's where nothing below it shows.
fn root_tops(outline: &TextOutline) -> Vec<f32> {
    let source = outline.snapshot();
    let mut root_of = BTreeMap::new();
    let mut roots = 0;
    for paragraph in &source.paragraphs {
        if paragraph.parent.is_none() {
            roots += 1;
        }
        root_of.insert(paragraph.id, roots - 1);
    }
    let y = outline.origin()[1];
    let mut tops = vec![f32::INFINITY; roots];
    for (_, paragraph) in outline.layouts() {
        if let Some(&root) = root_of.get(&paragraph.id) {
            tops[root] = tops[root].min(y + paragraph.origin[1]);
        }
    }
    let mut last = y;
    for top in &mut tops {
        if top.is_finite() {
            last = *top;
        } else {
            *top = last;
        }
    }
    tops
}

/// A page object as laid out: its identity, kind, bounds, and where its lines and pictures
/// lie within them.
type Item = (ExGuid, Kind, [f32; 4], Vec<[f32; 4]>);

/// The page's content as laid out, overlapping pieces and nearby strokes joined.
fn blocks(editor: &CanvasEditor) -> Vec<Block> {
    let mut items: Vec<Item> = editor
        .outlines()
        .iter()
        .filter(|outline| !outline.title && !outline.is_empty())
        .map(|outline| {
            let b = outline.bounds();
            let bounds = [b.x0, b.y0, b.x1, b.y1].map(|v| v as f32);
            (outline.id, Kind::Text, bounds, marks(outline))
        })
        .collect();
    for object in &editor.objects {
        let item = match object {
            Content::Outline {
                source,
                layout,
                below_title: None,
            } => {
                let [x, y] = crate::origin(&source.layout);
                Some((
                    source.id,
                    Kind::Text,
                    [x, y, x + layout.size[0], y + layout.size[1]],
                ))
            }
            Content::Image(image) if !image.background => {
                crate::outline::image_size(image).map(|[width, height]| {
                    let [x, y] = crate::origin(&image.layout);
                    (image.id, Kind::Picture, [x, y, x + width, y + height])
                })
            }
            Content::File { source, .. } => object.file().map(|(_, b)| (source.id, Kind::File, b)),
            Content::Ink(ink) => {
                crate::editor::page::ink_bounds(ink).map(|b| (ink.id, Kind::Drawing, b))
            }
            Content::ReadOnly(object) if !matches!(object.source, PageObject::Title(_)) => {
                Some((object.source.id(), Kind::Picture, object.rect()))
            }
            _ => None,
        };
        let item = item.filter(|(_, _, b)| b.iter().all(|v| v.is_finite()));
        items.extend(item.map(|(id, kind, bounds)| (id, kind, bounds, vec![bounds])));
    }
    let mut parent: Vec<usize> = (0..items.len()).collect();
    fn root(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    let meets = |a: &[f32; 4], b: &[f32; 4], reach: f32| {
        a[0] < b[2] + reach && b[0] < a[2] + reach && a[1] < b[3] + reach && b[1] < a[3] + reach
    };
    for i in 0..items.len() {
        for j in i + 1..items.len() {
            let (a, b) = (&items[i], &items[j]);
            // Touching edges are not overlap.
            let reach = if a.1 == Kind::Drawing && b.1 == Kind::Drawing {
                INK_REACH
            } else {
                -1.0
            };
            if meets(&a.2, &b.2, reach)
                && a.3.iter().any(|m| b.3.iter().any(|n| meets(m, n, reach)))
            {
                let (ra, rb) = (root(&mut parent, i), root(&mut parent, j));
                parent[ra] = rb;
            }
        }
    }
    let mut joined: BTreeMap<usize, Block> = BTreeMap::new();
    for (i, (id, kind, bounds, _)) in items.into_iter().enumerate() {
        let r = root(&mut parent, i);
        joined
            .entry(r)
            .and_modify(|block| {
                block.members.push(id);
                if !(block.kind == Kind::Drawing && kind == Kind::Drawing) {
                    block.kind = Kind::Group;
                }
                block.bounds = union(block.bounds, bounds);
            })
            .or_insert(Block {
                kind,
                members: vec![id],
                bounds,
                part: None,
                aside: false,
            });
    }
    joined.into_values().collect()
}

fn union(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [
        a[0].min(b[0]),
        a[1].min(b[1]),
        a[2].max(b[2]),
        a[3].max(b[3]),
    ]
}

/// `blocks` in reading order by recursive XY cut: rows first, then columns left to right,
/// notes beside a single outline as its asides.
fn order(
    blocks: Vec<Block>,
    tops: &BTreeMap<ExGuid, Vec<f32>>,
    kept: &mut Vec<Kept>,
) -> Vec<Block> {
    if blocks.len() <= 1 {
        return blocks;
    }
    let mut parts = cut(blocks, 1);
    if let [row] = &mut parts[..] {
        let row = std::mem::take(row);
        if let Some(asides) = asides(&row, tops) {
            kept.push(Kept::Asides);
            return asides;
        }
        parts = cut(row, 0);
        let texts = parts
            .iter()
            .filter(|part| part.iter().any(|b| b.kind == Kind::Text));
        if texts.count() > 1 {
            kept.push(Kept::SideBySide);
        }
    }
    if let [interlocked] = &mut parts[..] {
        // No gap runs across: top to bottom, then left to right.
        kept.push(Kept::SideBySide);
        let mut blocks = std::mem::take(interlocked);
        blocks.sort_by(|a, b| {
            a.bounds[1]
                .total_cmp(&b.bounds[1])
                .then(a.bounds[0].total_cmp(&b.bounds[0]))
        });
        return blocks;
    }
    parts
        .into_iter()
        .flat_map(|part| order(part, tops, kept))
        .collect()
}

/// `row` as one outline split after each paragraph a note beside it sits by, where every
/// other block is text no taller than the outline and clear of it across.
fn asides(row: &[Block], tops: &BTreeMap<ExGuid, Vec<f32>>) -> Option<Vec<Block>> {
    let height = |b: &Block| b.bounds[3] - b.bounds[1];
    let main = row
        .iter()
        .filter(|b| b.kind == Kind::Text)
        .max_by(|a, b| height(a).total_cmp(&height(b)))?;
    let tops = tops.get(&main.members[0])?;
    let mut notes: Vec<&Block> = row.iter().filter(|b| *b != main).collect();
    let beside = |b: &Block| {
        b.kind == Kind::Text
            && height(b) < height(main)
            && (b.bounds[0] >= main.bounds[0] + 72.0 || b.bounds[2] <= main.bounds[0])
    };
    if notes.is_empty() || tops.len() < 2 || !notes.iter().all(|b| beside(b)) {
        return None;
    }
    notes.sort_by(|a, b| a.bounds[1].total_cmp(&b.bounds[1]));
    let mut out = Vec::new();
    let mut from = 0;
    let slice = |range: Range<usize>| {
        let mut bounds = main.bounds;
        bounds[1] = tops[range.start];
        bounds[3] = tops.get(range.end).copied().unwrap_or(main.bounds[3]);
        Block {
            part: Some(range),
            bounds,
            ..main.clone()
        }
    };
    for note in notes {
        let after = tops
            .partition_point(|top| *top <= note.bounds[1] + ASIDE_SLACK)
            .max(1);
        if after > from {
            out.push(slice(from..after));
            from = after;
        }
        out.push(Block {
            aside: true,
            ..note.clone()
        });
    }
    if from < tops.len() {
        out.push(slice(from..tops.len()));
    }
    Some(out)
}

/// `blocks` split at each gap along `axis` (0 for x, 1 for y) that no block spans.
fn cut(mut blocks: Vec<Block>, axis: usize) -> Vec<Vec<Block>> {
    blocks.sort_by(|a, b| a.bounds[axis].total_cmp(&b.bounds[axis]));
    let mut parts: Vec<Vec<Block>> = Vec::new();
    let mut end = f32::NEG_INFINITY;
    for block in blocks {
        if parts.is_empty() || block.bounds[axis] >= end {
            parts.push(Vec::new());
        }
        end = end.max(block.bounds[axis + 2]);
        parts.last_mut().unwrap().push(block);
    }
    parts
}

/// Splits each outline asides cut into one outline per part, shown only: the first part
/// keeps the outline's identity, the others take ones derived from it.
fn split(page: &mut Page, mut blocks: Vec<Block>) -> Vec<Block> {
    let mut whole: BTreeMap<ExGuid, Outline> = BTreeMap::new();
    for block in &mut blocks {
        let Some(range) = block.part.clone() else {
            continue;
        };
        let id = block.members[0];
        let source = match whole.get(&id) {
            Some(source) => source.clone(),
            None => {
                let Some(PageObject::Outline(source)) = object_mut(page, id).cloned() else {
                    continue;
                };
                whole.insert(id, source.clone());
                source
            }
        };
        let mut root = 0;
        let paragraphs = source
            .paragraphs
            .iter()
            .filter(|paragraph| {
                if paragraph.parent.is_none() {
                    root += 1;
                }
                range.contains(&(root - 1))
            })
            .cloned()
            .collect();
        let part = Outline {
            id: if range.start == 0 {
                id
            } else {
                ExGuid {
                    guid: id.guid,
                    n: id.n ^ (0x8000_0000 | range.start as u32),
                }
            },
            paragraphs,
            ..source
        };
        block.members[0] = part.id;
        match object_mut(page, part.id) {
            Some(object) => *object = PageObject::Outline(part),
            None => page.objects.push(PageObject::Outline(part)),
        }
    }
    blocks
}

/// Stacks `blocks` in order from `start` down a column `column` wide.
fn reflow(
    page: &mut Page,
    blocks: &[Block],
    start: [f32; 2],
    column: f32,
    engine: &mut TextEngine,
    kept: &mut Vec<Kept>,
) -> Result<(), EditorError> {
    let [left, mut y] = start;
    // Widths first: text rewraps, pictures shrink, the rest moves across whole.
    let mut heights = Vec::with_capacity(blocks.len());
    for block in blocks {
        let [x0, y0, x1, y1] = block.bounds;
        let mut height = y1 - y0;
        let indent = if block.aside { ASIDE_INDENT } else { 0.0 };
        match (block.kind, object_mut(page, block.members[0])) {
            (Kind::Text, Some(PageObject::Outline(outline))) => {
                outline.layout.x = Some(left + indent);
                outline.layout.max_width = Some((x1 - x0).min(column - indent));
                outline.layout.reserved_width = None;
                outline.min_width = None;
                collapse_blanks(outline);
                for paragraph in &mut outline.paragraphs {
                    if let ParagraphContent::Image(image) = &mut paragraph.content {
                        shrink(image, column - 36.0);
                    }
                }
            }
            (Kind::Picture, Some(PageObject::Image(image))) => {
                image.layout.x = Some(left);
                height = shrink(image, column).unwrap_or(height);
            }
            _ => {
                for &id in &block.members {
                    if let Some(object) = object_mut(page, id) {
                        let layout = object.layout_mut();
                        layout.x = Some(layout.x.unwrap_or(0.0) + left - x0);
                    }
                }
                if x1 - x0 > column + 1.0 {
                    kept.push(Kept::Wide);
                }
            }
        }
        heights.push(height);
    }
    // Text heights at the new widths.
    let laid = CanvasEditor::from_page(page.clone(), engine)?;
    for (block, height) in blocks.iter().zip(&mut heights) {
        if block.kind != Kind::Text {
            continue;
        }
        if let Some(outline) = laid.outlines().iter().find(|o| o.id == block.members[0]) {
            let b = outline.bounds();
            *height = (b.y1 - b.y0) as f32;
            if (b.x1 - b.x0) as f32 > column + 1.0 {
                kept.push(Kept::Wide);
            }
        }
    }
    for (index, (block, height)) in blocks.iter().zip(&heights).enumerate() {
        // Asides and the parts of the outline they split sit close.
        let close = |block: &Block| block.aside || block.part.as_ref().is_some_and(|p| p.start > 0);
        if index > 0 {
            let previous = &blocks[index - 1];
            y += if close(block) || previous.aside {
                ASIDE_GAP
            } else {
                GAP
            };
        }
        let dy = y - block.bounds[1];
        for (index, &id) in block.members.iter().enumerate() {
            if let Some(object) = object_mut(page, id) {
                let layout = object.layout_mut();
                layout.y = Some(
                    if index == 0 && matches!(block.kind, Kind::Text | Kind::Picture) {
                        y
                    } else {
                        layout.y.unwrap_or(0.0) + dy
                    },
                );
            }
        }
        y += height;
    }
    Ok(())
}

/// Drops each blank top-level paragraph that follows another, as spacing kept a side
/// column level with the text beside it.
fn collapse_blanks(outline: &mut Outline) {
    let blank = |p: &onestore::page::PageParagraph| {
        p.lists.is_empty()
            && p.tags.is_empty()
            && p.text()
                .is_some_and(|t| t.tags.is_empty() && t.text.text().trim().is_empty())
    };
    let parents: std::collections::BTreeSet<ExGuid> =
        outline.paragraphs.iter().filter_map(|p| p.parent).collect();
    let mut previous_blank = false;
    let mut root_kept = true;
    outline.paragraphs.retain(|p| {
        if p.parent.is_some() {
            return root_kept;
        }
        let is_blank = blank(p) && !parents.contains(&p.id);
        root_kept = !(is_blank && previous_blank);
        previous_blank = is_blank;
        root_kept
    });
}

fn object_mut(page: &mut Page, id: ExGuid) -> Option<&mut PageObject> {
    page.objects.iter_mut().find(|object| object.id() == id)
}

/// Scales `image` down to `width` points across, keeping its shape; its new height.
fn shrink(image: &mut onestore::page::Image, width: f32) -> Option<f32> {
    let [w, h] = crate::outline::image_size(image)?;
    let scale = (width / w).min(1.0);
    image.layout.max_width = Some(w * scale);
    image.layout.max_height = Some(h * scale);
    Some(h * scale)
}
