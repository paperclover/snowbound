use onestore::page::text::{EditError, Paragraph, new_id};
use onestore::page::{PageParagraph, ParagraphContent, TableCell, TextObject};
use onestore::{ExGuid, document::Format};
use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Range,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct TextPosition {
    pub paragraph: usize,
    pub offset: u32,
}

/// Paragraph positions enumerate text leaves in document order, including table cells.
#[derive(Clone, Debug, PartialEq)]
pub struct TextDocument {
    nodes: Vec<PageParagraph>,
    /// The paragraph position of each root node's first text leaf, then the paragraph count, so
    /// a position finds its node by search instead of walking every leaf before it.
    starts: Vec<usize>,
    /// Root indices of the tables, so a cell is found without walking every node.
    tables: Vec<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DocumentEdit {
    /// Cell identity, or None for the document root.
    pub(crate) container: Option<ExGuid>,
    pub(crate) range: Range<usize>,
    pub(crate) replacement: Vec<PageParagraph>,
    /// Width patches address surviving tables outside the replacement subtree.
    pub(crate) columns: BTreeMap<ExGuid, Vec<f32>>,
}

pub(crate) fn node(text: Paragraph, format: Format) -> Result<PageParagraph, EditError> {
    Ok(PageParagraph {
        id: new_id()?,
        parent: None,
        level: 1,
        format,
        content: onestore::page::ParagraphContent::Text(TextObject {
            date_field: None,
            id: new_id()?,
            text,
            tags: Vec::new(),
        }),
        lists: Vec::new(),
        tags: Vec::new(),
        media: Default::default(),
        collapsed: false,
        style: None,
    })
}

pub(crate) fn edited_nodes<'a>(
    nodes: &'a [PageParagraph],
    container: Option<ExGuid>,
    edit: Option<&'a DocumentEdit>,
) -> impl Clone + Iterator<Item = &'a PageParagraph> {
    let (range, replacement) = match edit.filter(|edit| edit.container == container) {
        Some(edit) => (edit.range.clone(), edit.replacement.as_slice()),
        None => (0..0, &[][..]),
    };
    nodes[..range.start]
        .iter()
        .chain(replacement)
        .chain(&nodes[range.end..])
}

pub(crate) fn leaves<'a>(
    nodes: &'a [PageParagraph],
    edit: Option<&'a DocumentEdit>,
) -> impl Iterator<Item = (Option<ExGuid>, usize, &'a PageParagraph)> {
    descendants(nodes, edit).filter(|(_, _, node)| node.text().is_some())
}

pub(crate) fn descendants<'a>(
    nodes: &'a [PageParagraph],
    edit: Option<&'a DocumentEdit>,
) -> impl Iterator<Item = (Option<ExGuid>, usize, &'a PageParagraph)> {
    let mut current = (None, edited_nodes(nodes, None, edit).enumerate());
    let mut pending = Vec::new();
    std::iter::from_fn(move || {
        loop {
            let Some((index, node)) = current.1.next() else {
                current = pending.pop()?;
                continue;
            };
            let container = current.0;
            if let ParagraphContent::Table(table) = &node.content {
                pending.push(current.clone());
                pending.extend(
                    table
                        .rows
                        .iter()
                        .rev()
                        .flat_map(|row| row.cells.iter().rev())
                        .map(|cell| {
                            (
                                Some(cell.id),
                                edited_nodes(&cell.paragraphs, Some(cell.id), edit).enumerate(),
                            )
                        }),
                );
                current = pending.pop().unwrap();
            }
            return Some((container, index, node));
        }
    })
}

pub(crate) fn swap_columns(nodes: &mut [PageParagraph], widths: &mut BTreeMap<ExGuid, Vec<f32>>) {
    if widths.is_empty() {
        return;
    }
    for node in nodes {
        if let ParagraphContent::Table(table) = &mut node.content {
            if let Some(widths) = widths.get_mut(&table.id) {
                for (column, width) in table.columns.iter_mut().zip(widths) {
                    std::mem::swap(&mut column.width, width);
                }
            }
            for cell in table.rows.iter_mut().flat_map(|row| &mut row.cells) {
                swap_columns(&mut cell.paragraphs, widths);
            }
        }
    }
}

/// Identities a node holds itself; paragraphs in its cells hold their own.
fn owned_ids(node: &PageParagraph) -> impl Iterator<Item = ExGuid> + '_ {
    let (content, rows) = match &node.content {
        ParagraphContent::Text(text) => (text.id, &[][..]),
        ParagraphContent::Image(image) => (image.id, &[][..]),
        ParagraphContent::Attachment(file) => (file.id, &[][..]),
        ParagraphContent::Ink(ink) => (ink.id, &[][..]),
        ParagraphContent::Unsupported(unsupported) => (unsupported.id, &[][..]),
        ParagraphContent::Table(table) => (table.id, table.rows.as_slice()),
    };
    [node.id, content].into_iter().chain(
        rows.iter()
            .flat_map(|row| std::iter::once(row.id).chain(row.cells.iter().map(|cell| cell.id))),
    )
}

pub(crate) fn validate_nodes(
    nodes: &[PageParagraph],
    ids: &mut BTreeSet<ExGuid>,
) -> Result<(), EditError> {
    if nodes.is_empty() {
        return Err(EditError::InvalidRange);
    }
    validate_run(nodes, &[], 0, ids)
}

/// Validates `nodes`, which follow `earlier` in a container at `depth`, and every container
/// nested in them; `earlier` is searched only for parents the run lacks.
fn validate_run(
    nodes: &[PageParagraph],
    earlier: &[PageParagraph],
    depth: usize,
    ids: &mut BTreeSet<ExGuid>,
) -> Result<(), EditError> {
    let mut pending = vec![(nodes, earlier, depth)];
    while let Some((nodes, earlier, depth)) = pending.pop() {
        if depth > 64 {
            return Err(EditError::InvalidStructure);
        }
        let mut parents = BTreeMap::new();
        for node in nodes {
            let parent = |id| {
                parents.get(&id).copied().or_else(|| {
                    earlier
                        .iter()
                        .rev()
                        .find(|node| node.id == id)
                        .map(|node| node.level)
                })
            };
            if owned_ids(node).any(|id| !ids.insert(id))
                || node.level == 0
                || node
                    .parent
                    .is_some_and(|id| parent(id).is_none_or(|level| level >= node.level))
            {
                return Err(EditError::InvalidStructure);
            }
            parents.insert(node.id, node.level);
            match &node.content {
                ParagraphContent::Image(image) => {
                    crate::outline::image_size(image).ok_or(EditError::UnsupportedContent)?;
                }
                ParagraphContent::Table(table) => {
                    if table.rows.is_empty()
                        || table.columns.is_empty()
                        || table.columns.len() > 255
                        || table
                            .columns
                            .iter()
                            .any(|column| !column.width.is_finite() || column.width < 36.0)
                        || table
                            .rows
                            .iter()
                            .any(|row| row.cells.len() != table.columns.len())
                    {
                        return Err(EditError::InvalidStructure);
                    }
                    for cell in table.rows.iter().flat_map(|row| &row.cells) {
                        if !cell.unsupported.is_empty() {
                            return Err(EditError::UnsupportedContent);
                        }
                        if cell.paragraphs.is_empty() {
                            return Err(EditError::InvalidRange);
                        }
                        pending.push((cell.paragraphs.as_slice(), &[][..], depth + 1));
                    }
                }
                ParagraphContent::Text(_)
                | ParagraphContent::Attachment(_)
                | ParagraphContent::Ink(_)
                | ParagraphContent::Unsupported(_) => {}
            }
        }
    }
    Ok(())
}

fn validate_text(nodes: &[PageParagraph]) -> Result<(), EditError> {
    for (_, _, node) in leaves(nodes, None) {
        let text = &node.text().unwrap().text;
        text.utf16_offset(text.text().len())?;
    }
    Ok(())
}

/// Whether UTF-16 `offset` lies inside a hyperlink field, which OneNote has not been seen
/// splitting between paragraphs.
fn divides_link(text: &Paragraph, offset: u32) -> Result<bool, EditError> {
    let at = text.byte_offset(offset)?;
    let link = |byte: usize| {
        text.spans()
            .iter()
            .find(|span| byte < span.end)
            .is_some_and(|span| span.format.hyperlink == Some(true))
    };
    Ok(at > 0 && link(at - 1) && link(at) && !text.text()[at..].starts_with('\u{fddf}'))
}

/// What deleting `range`, which crosses a table's edge, leaves of `nodes`, whose
/// first text leaf is paragraph `*next`, and whether all of them lay inside it. As OneNote 2010
/// deletes such a selection, each container keeps what lies outside it and nothing joins
/// across a container's edge: the end paragraphs keep their outer text, a cell inside keeps
/// one empty paragraph, except that where the selection runs on past a table its rows with
/// every cell inside go, and the table when all of them do. A paragraph that goes leaves its
/// children to its parent.
fn cut(
    nodes: &[PageParagraph],
    next: &mut usize,
    range: &Range<TextPosition>,
) -> Result<(Vec<PageParagraph>, bool), EditError> {
    let (start, end) = (range.start, range.end);
    let mut kept = Vec::new();
    let mut gone = BTreeMap::new();
    for node in nodes {
        let inside = match &node.content {
            ParagraphContent::Text(text) => {
                let at = *next;
                *next += 1;
                let text = &text.text;
                let length = text.utf16_offset(text.text().len())?;
                let shown = match at {
                    at if at == start.paragraph => Some(text.slice(0..start.offset)?),
                    at if at == end.paragraph => Some(text.slice(end.offset..length)?),
                    _ => None,
                };
                if let Some(shown) = shown {
                    let mut node = node.clone();
                    node.text_mut().unwrap().text = shown;
                    kept.push(node);
                    continue;
                }
                start.paragraph < at && at < end.paragraph
            }
            ParagraphContent::Table(table) => {
                let mut table = table.clone();
                let mut whole_rows = Vec::new();
                for row in &mut table.rows {
                    let mut cells = Vec::new();
                    for cell in &row.cells {
                        let (paragraphs, inside) = cut(&cell.paragraphs, next, range)?;
                        cells.push((cell, paragraphs, inside));
                    }
                    let whole = cells.iter().all(|(_, _, inside)| *inside);
                    row.cells = cells
                        .into_iter()
                        .map(|(cell, paragraphs, inside)| {
                            let paragraphs = if inside {
                                let (_, _, leaf) = leaves(&cell.paragraphs, None)
                                    .next()
                                    .ok_or(EditError::InvalidStructure)?;
                                let mut leaf = leaf.clone();
                                let text = &mut leaf.text_mut().unwrap().text;
                                *text = text.slice(0..0)?;
                                leaf.parent = None;
                                leaf.level = 1;
                                vec![leaf]
                            } else {
                                paragraphs
                            };
                            Ok(TableCell {
                                paragraphs,
                                ..cell.clone()
                            })
                        })
                        .collect::<Result<_, EditError>>()?;
                    whole_rows.push(whole);
                }
                // Rows go only where the selection runs on past the table.
                if end.paragraph >= *next {
                    let mut whole = whole_rows.into_iter();
                    table.rows.retain(|_| !whole.next().unwrap_or(false));
                }
                if table.rows.is_empty() {
                    true
                } else {
                    let mut node = node.clone();
                    node.content = ParagraphContent::Table(table);
                    kept.push(node);
                    continue;
                }
            }
            _ => start.paragraph < *next && *next <= end.paragraph,
        };
        if inside {
            gone.insert(node.id, node.parent);
        } else {
            kept.push(node.clone());
        }
    }
    let whole = kept.is_empty();
    for node in &mut kept {
        while let Some(parent) = node.parent.and_then(|parent| gone.get(&parent)) {
            node.parent = *parent;
        }
    }
    Ok((kept, whole))
}

/// Whether a split or join keeping `first` up to UTF-16 `start` and `last` from `end` on meets
/// an embedded object at the seam or carries one to another paragraph, or joins inside an
/// equation's objects: the object's run data belongs to its paragraph, and an equation divides
/// only at its edges (Enter inside one breaks its line instead, `CanvasEditor::enter`).
fn moves_object(
    first: &Paragraph,
    start: u32,
    last: &Paragraph,
    end: u32,
) -> Result<bool, EditError> {
    let math = |text: &Paragraph, offset: u32| {
        text.format_at(offset)
            .is_ok_and(|format| format.math == Some(true))
    };
    let (before, after) = (first.byte_offset(start)?, last.byte_offset(end)?);
    let following = last.text()[after..]
        .chars()
        .next()
        .map(|c| end + c.len_utf16() as u32);
    // An equation joins where both seams lie outside its objects (fractions, scripts and the
    // like), as OneNote 2010 joins two equations a deletion meets.
    let nested = |text: &str| {
        text.chars().fold(0_i32, |depth, c| match c {
            '\u{fdd0}' => depth + 1,
            '\u{fdef}' => depth - 1,
            _ => depth,
        }) > 0
    };
    Ok(first.text()[..before].ends_with('\u{fffc}')
        || last.text()[after..].contains('\u{fffc}')
        || start > 0
            && math(first, start)
            && following.is_some_and(|next| math(last, next))
            && (nested(&first.text()[..before]) || nested(&last.text()[..after])))
}

/// Paragraph positions of each node's first text leaf, counting from `first`.
fn starts(nodes: &[PageParagraph], first: usize) -> impl Iterator<Item = usize> + '_ {
    nodes.iter().scan(first, |next, node| {
        let start = *next;
        *next += leaves(std::slice::from_ref(node), None).count();
        Some(start)
    })
}

fn enclosing(
    nodes: &[PageParagraph],
    first: usize,
    paragraph: usize,
    ranges: &mut Vec<Range<usize>>,
) {
    let count = |nodes: &[PageParagraph]| leaves(nodes, None).count();
    let Some((index, start)) = starts(nodes, first)
        .enumerate()
        .take_while(|(_, start)| *start <= paragraph)
        .last()
    else {
        return;
    };
    let ParagraphContent::Table(table) = &nodes[index].content else {
        let end = subtree_end(nodes, index);
        if end > index + 1 {
            ranges.push(start..start + count(&nodes[index..end]));
        }
        return;
    };
    let mut cell_start = start;
    for row in &table.rows {
        let row_start = cell_start;
        let mut holder = None;
        for cell in &row.cells {
            let cell_end = cell_start + count(&cell.paragraphs);
            if (cell_start..cell_end).contains(&paragraph) {
                holder = Some((cell, cell_start..cell_end));
            }
            cell_start = cell_end;
        }
        if let Some((cell, range)) = holder {
            enclosing(&cell.paragraphs, range.start, paragraph, ranges);
            ranges.extend([
                range,
                row_start..cell_start,
                start..start + count(&nodes[index..=index]),
            ]);
            return;
        }
    }
}

/// The index after `nodes[index]`'s last descendant. Descendants follow their ancestor
/// contiguously; a paragraph only an outline group indents has no parent to descend from.
pub(crate) fn subtree_end(nodes: &[PageParagraph], index: usize) -> usize {
    let mut members = BTreeSet::from([nodes[index].id]);
    index
        + 1
        + nodes[index + 1..]
            .iter()
            .take_while(|node| {
                node.parent.is_some_and(|parent| members.contains(&parent))
                    && members.insert(node.id)
            })
            .count()
}

/// The nodes from `from` on that descend from a paragraph in `moves` or `shifts`, rebuilt so
/// the children of each paragraph in `moves` belong to its new parent, one level below it, and
/// every subtree keeps its depth below its root; ends at the last node that changes.
fn adopt(
    nodes: &[PageParagraph],
    from: usize,
    moves: &BTreeMap<ExGuid, &PageParagraph>,
    mut shifts: BTreeMap<ExGuid, i64>,
) -> Result<Vec<PageParagraph>, EditError> {
    let mut adopted = Vec::new();
    let mut changed = 0;
    for node in &nodes[from..] {
        let Some(parent) = node.parent else { break };
        let mut node = node.clone();
        let shift = match (moves.get(&parent), shifts.get(&parent)) {
            (Some(holder), _) => {
                node.parent = Some(holder.id);
                i64::from(holder.level) + 1 - i64::from(node.level)
            }
            (None, Some(shift)) => *shift,
            (None, None) => break,
        };
        node.level = u32::try_from(i64::from(node.level) + shift)
            .map_err(|_| EditError::InvalidStructure)?;
        shifts.insert(node.id, shift);
        if shift != 0 || moves.contains_key(&parent) {
            changed = adopted.len() + 1;
        }
        adopted.push(node);
    }
    adopted.truncate(changed);
    Ok(adopted)
}

/// The sibling `nodes[index]` follows, passing over `skipped` siblings, when everything
/// between them descends from it or from a skipped paragraph.
pub(crate) fn previous_sibling(
    nodes: &[PageParagraph],
    index: usize,
    skipped: &BTreeSet<ExGuid>,
) -> Option<usize> {
    let node = &nodes[index];
    let sibling = (0..index).rev().find(|&at| {
        nodes[at].level <= node.level
            && !(nodes[at].level == node.level && skipped.contains(&nodes[at].id))
    })?;
    if nodes[sibling].level != node.level || nodes[sibling].parent != node.parent {
        return None;
    }
    let mut members = BTreeSet::from([nodes[sibling].id]);
    nodes[sibling + 1..index]
        .iter()
        .all(|node| {
            let inside = skipped.contains(&node.id)
                || node.parent.is_some_and(|parent| members.contains(&parent));
            members.insert(node.id);
            inside
        })
        .then_some(sibling)
}

/// Tab or Shift+Tab on `range` of `nodes` as OneNote does, moving each paragraph with its
/// subtree: indenting makes a paragraph the last child of its previous sibling, or without one
/// indents it within its group; outdenting a child makes it its parent's sibling, adopting the
/// siblings after it. None when nothing moves.
pub(crate) fn indent(
    nodes: &[PageParagraph],
    container: Option<ExGuid>,
    range: Range<usize>,
    outdent: bool,
) -> Option<DocumentEdit> {
    let selected = nodes[range.clone()]
        .iter()
        .map(|node| node.id)
        .collect::<BTreeSet<_>>();
    let mut end = range.end;
    let mut tops = BTreeMap::new();
    let mut adopters = BTreeMap::new();
    for index in range.clone() {
        let node = &nodes[index];
        if node.parent.is_some_and(|parent| selected.contains(&parent)) {
            continue;
        }
        end = end.max(subtree_end(nodes, index));
        if !outdent {
            let parent = previous_sibling(nodes, index, &selected).map(|at| nodes[at].id);
            tops.insert(node.id, (parent.or(node.parent), 1));
        } else if node.level > 1 {
            let parent = node
                .parent
                .and_then(|id| nodes[..index].iter().rposition(|node| node.id == id))
                .filter(|&at| nodes[at].level + 1 == node.level);
            let parent = match parent {
                Some(at) => {
                    end = end.max(subtree_end(nodes, at));
                    adopters.insert(nodes[at].id, node.id);
                    nodes[at].parent
                }
                None => node.parent,
            };
            tops.insert(node.id, (parent, -1));
        }
    }
    if tops.is_empty() {
        return None;
    }
    let mut shifts = BTreeMap::new();
    let replacement = nodes[range.start..end]
        .iter()
        .map(|node| {
            let mut node = node.clone();
            let shift = match (tops.get(&node.id), node.parent) {
                (Some(&(parent, shift)), _) => {
                    node.parent = parent;
                    shift
                }
                (None, Some(parent))
                    if !selected.contains(&node.id) && adopters.contains_key(&parent) =>
                {
                    node.parent = Some(adopters[&parent]);
                    0
                }
                (None, parent) => parent
                    .and_then(|parent| shifts.get(&parent).copied())
                    .unwrap_or(0),
            };
            node.level = node.level.saturating_add_signed(shift);
            shifts.insert(node.id, shift);
            node
        })
        .collect();
    Some(DocumentEdit {
        columns: BTreeMap::new(),
        container,
        range: range.start..end,
        replacement,
    })
}

pub(crate) fn container_mut(
    nodes: &mut Vec<PageParagraph>,
    id: Option<ExGuid>,
) -> Option<&mut Vec<PageParagraph>> {
    match id {
        None => Some(nodes),
        Some(id) => cell_mut(nodes, id),
    }
}

fn cell_mut(nodes: &mut [PageParagraph], id: ExGuid) -> Option<&mut Vec<PageParagraph>> {
    for node in nodes {
        if let ParagraphContent::Table(table) = &mut node.content {
            for cell in table.rows.iter_mut().flat_map(|row| &mut row.cells) {
                if cell.id == id {
                    return Some(&mut cell.paragraphs);
                }
                if let Some(nodes) = cell_mut(&mut cell.paragraphs, id) {
                    return Some(nodes);
                }
            }
        }
    }
    None
}

fn tables(nodes: &[PageParagraph], first: usize) -> impl Iterator<Item = usize> + '_ {
    nodes
        .iter()
        .enumerate()
        .filter(|(_, node)| matches!(node.content, ParagraphContent::Table(_)))
        .map(move |(index, _)| first + index)
}

impl TextDocument {
    pub fn new(paragraphs: Vec<Paragraph>) -> Result<Self, EditError> {
        Self::from_nodes(
            paragraphs
                .into_iter()
                .map(|text| node(text, Format::default()))
                .collect::<Result<_, _>>()?,
        )
    }

    pub fn from_nodes(nodes: Vec<PageParagraph>) -> Result<Self, EditError> {
        validate_nodes(&nodes, &mut BTreeSet::new())?;
        validate_text(&nodes)?;
        let mut starts = starts(&nodes, 0).collect::<Vec<_>>();
        starts.push(leaves(&nodes, None).count());
        Ok(Self {
            tables: tables(&nodes, 0).collect(),
            nodes,
            starts,
        })
    }

    pub fn nodes(&self) -> &[PageParagraph] {
        &self.nodes
    }

    pub fn text_nodes(&self) -> impl Iterator<Item = &PageParagraph> {
        leaves(&self.nodes, None).map(|(_, _, node)| node)
    }

    pub fn paragraphs(&self) -> impl Iterator<Item = &Paragraph> {
        self.text_nodes().map(|node| &node.text().unwrap().text)
    }

    /// The text leaf at a paragraph position, with its container and index there.
    pub(crate) fn leaf(&self, paragraph: usize) -> Option<(Option<ExGuid>, usize, &PageParagraph)> {
        let root = self
            .starts
            .partition_point(|start| *start <= paragraph)
            .checked_sub(1)?;
        let node = self.nodes.get(root)?;
        match node.text() {
            Some(_) => Some((None, root, node)),
            None => leaves(std::slice::from_ref(node), None).nth(paragraph - self.starts[root]),
        }
    }

    /// The text leaf at a paragraph position once a valid `edit` applies, with the position it
    /// has now unless the edit supplies it.
    pub(crate) fn edited_leaf<'a>(
        &'a self,
        edit: &'a DocumentEdit,
        paragraph: usize,
    ) -> Option<(&'a PageParagraph, Option<usize>)> {
        let nodes = self.container(edit.container).ok()?;
        let first = match edit.container {
            None => self.starts[edit.range.start],
            Some(cell) => {
                let root = self.root(cell).ok()?;
                self.starts[root]
                    + descendants(std::slice::from_ref(&self.nodes[root]), None)
                        .take_while(|(container, _, _)| *container != edit.container)
                        .filter(|(_, _, node)| node.text().is_some())
                        .count()
                    + leaves(&nodes[..edit.range.start], None).count()
            }
        };
        let added = leaves(&edit.replacement, None).count();
        match paragraph.checked_sub(first) {
            Some(offset) if offset < added => leaves(&edit.replacement, None)
                .nth(offset)
                .map(|(_, _, node)| (node, None)),
            Some(_) => {
                let before = paragraph - added + leaves(&nodes[edit.range.clone()], None).count();
                self.leaf(before).map(|(_, _, node)| (node, Some(before)))
            }
            None => self
                .leaf(paragraph)
                .map(|(_, _, node)| (node, Some(paragraph))),
        }
    }

    /// Text leaf ranges holding `paragraph`, innermost first: its subtree when it has
    /// descendants, then the cell, row and table of each table around it.
    pub(crate) fn enclosing(&self, paragraph: usize) -> Vec<Range<usize>> {
        let mut ranges = Vec::new();
        enclosing(&self.nodes, 0, paragraph, &mut ranges);
        ranges
    }

    /// The root node holding a table cell.
    pub(crate) fn root(&self, cell: ExGuid) -> Result<usize, EditError> {
        self.tables
            .iter()
            .copied()
            .find(|&root| {
                descendants(std::slice::from_ref(&self.nodes[root]), None)
                    .any(|(container, _, _)| container == Some(cell))
            })
            .ok_or(EditError::InvalidRange)
    }

    pub(crate) fn paragraph(&self, index: usize) -> Option<&Paragraph> {
        self.leaf(index)
            .map(|(_, _, node)| &node.text().unwrap().text)
    }

    pub(crate) fn container(&self, id: Option<ExGuid>) -> Result<&[PageParagraph], EditError> {
        self.nested_container(id).map(|(nodes, _)| nodes)
    }

    /// The container's paragraphs and how many tables enclose them.
    fn nested_container(&self, id: Option<ExGuid>) -> Result<(&[PageParagraph], usize), EditError> {
        let Some(id) = id else {
            return Ok((&self.nodes, 0));
        };
        let root = self.root(id)?;
        let mut pending = vec![(std::slice::from_ref(&self.nodes[root]), 0)];
        while let Some((nodes, depth)) = pending.pop() {
            for node in nodes {
                if let ParagraphContent::Table(table) = &node.content {
                    for cell in table.rows.iter().flat_map(|row| &row.cells) {
                        if cell.id == id {
                            return Ok((&cell.paragraphs, depth + 1));
                        }
                        pending.push((&cell.paragraphs, depth + 1));
                    }
                }
            }
        }
        Err(EditError::InvalidRange)
    }

    /// The nodes `range` covers, its end paragraphs cut to it: those of its container from
    /// one end to the other, or where it crosses a table's edge, the root nodes holding them.
    pub(crate) fn selected(
        &self,
        range: Range<TextPosition>,
    ) -> Result<Vec<PageParagraph>, EditError> {
        let (start_container, start, first) = self
            .leaf(range.start.paragraph)
            .ok_or(EditError::InvalidRange)?;
        let (end_container, end, last) = self
            .leaf(range.end.paragraph)
            .ok_or(EditError::InvalidRange)?;
        let (first, last) = (first.id, last.id);
        let (nodes, start, end) = if self.crosses_table(range.clone())? {
            let root = |container: Option<ExGuid>, index| match container {
                Some(cell) => self.root(cell),
                None => Ok(index),
            };
            (
                &self.nodes[..],
                root(start_container, start)?,
                root(end_container, end)?,
            )
        } else {
            (self.container(start_container)?, start, end)
        };
        let mut nodes = nodes[start..=end].to_vec();
        let tail = nodes.len() - 1;
        if nodes[tail].id == last {
            let from = if first == last { range.start.offset } else { 0 };
            let text = &mut nodes[tail].text_mut().unwrap().text;
            *text = text.slice(from..range.end.offset)?;
        }
        if nodes[0].id == first && first != last {
            let text = &mut nodes[0].text_mut().unwrap().text;
            let length = text.utf16_offset(text.text().len())?;
            *text = text.slice(range.start.offset..length)?;
        }
        Ok(nodes)
    }

    /// Replaces `range` as OneNote's typing, Enter and deletion do: the first paragraph keeps
    /// its identity and properties, and a paragraph the replacement adds takes its level,
    /// parent, style and lists but no note tags, except that a range's last paragraph stays
    /// itself when the replacement ends in one. The last paragraph holds the children of the
    /// paragraphs the edit removes or splits.
    pub(crate) fn replace(
        &self,
        range: Range<TextPosition>,
        replacement: Vec<Paragraph>,
    ) -> Result<DocumentEdit, EditError> {
        if range.start > range.end {
            return Err(EditError::InvalidRange);
        }
        if self.crosses_table(range.clone())? {
            return self.replace_across(range, replacement);
        }
        let (container, start, first) = self
            .leaf(range.start.paragraph)
            .ok_or(EditError::InvalidRange)?;
        let (_, end, last) = self
            .leaf(range.end.paragraph)
            .ok_or(EditError::InvalidRange)?;
        let nodes = self.container(container)?;
        let first_text = &first.text().unwrap().text;
        let last_text = &last.text().unwrap().text;
        let split = replacement.len() > 1;
        if (split || start != end)
            && (moves_object(first_text, range.start.offset, last_text, range.end.offset)?
                || split
                    && (divides_link(first_text, range.start.offset)?
                        || divides_link(last_text, range.end.offset)?))
        {
            return Err(EditError::UnsupportedContent);
        }
        let mut prefix = first_text.slice(0..range.start.offset)?;
        let suffix =
            last_text.slice(range.end.offset..last_text.utf16_offset(last_text.text().len())?)?;
        let mut replacement = replacement.into_iter();
        prefix.append(replacement.next().ok_or(EditError::InvalidRange)?)?;
        let mut head = first.clone();
        head.text_mut().unwrap().text = prefix;
        let following = replacement.len();
        let keeps_last = start != end && following > 0;
        let mut added = Vec::new();
        for (index, text) in replacement.enumerate() {
            let mut next = if keeps_last && index + 1 == following {
                let mut end = last.clone();
                end.text_mut().unwrap().text = text;
                end
            } else {
                let mut next = node(text, first.format.clone())?;
                next.style = first.style;
                next.lists.clone_from(&first.lists);
                next
            };
            next.parent = first.parent;
            next.level = first.level;
            added.push(next);
        }
        if let Some(tail) = added.last_mut() {
            tail.collapsed |= std::mem::take(&mut head.collapsed);
        }
        let tail = added.last_mut().unwrap_or(&mut head);
        if !suffix.text().is_empty() {
            tail.text_mut().unwrap().text.append(suffix)?;
        }
        let tail = added.last().unwrap_or(&head);
        let mut moves = nodes[start + 1..end + usize::from(!keeps_last)]
            .iter()
            .map(|node| (node.id, tail))
            .collect::<BTreeMap<_, _>>();
        if tail.id != head.id {
            moves.insert(head.id, tail);
        }
        let shifts = keeps_last
            .then(|| (last.id, i64::from(first.level) - i64::from(last.level)))
            .into_iter()
            .collect();
        let adopted = adopt(nodes, end + 1, &moves, shifts)?;
        Ok(DocumentEdit {
            columns: BTreeMap::new(),
            container,
            range: start..end + 1 + adopted.len(),
            replacement: [head].into_iter().chain(added).chain(adopted).collect(),
        })
    }

    /// Whether `range` crosses a table's edge: its ends lie in different containers, or a table
    /// lies between them.
    pub(crate) fn crosses_table(&self, range: Range<TextPosition>) -> Result<bool, EditError> {
        let (container, start, _) = self
            .leaf(range.start.paragraph)
            .ok_or(EditError::InvalidRange)?;
        let (end_container, end, _) = self
            .leaf(range.end.paragraph)
            .ok_or(EditError::InvalidRange)?;
        Ok(container != end_container
            || self.container(container)?[start..end]
                .iter()
                .any(|node| matches!(node.content, ParagraphContent::Table(_))))
    }

    /// `replace` of a range crossing a table's edge, as OneNote 2010 deletes such a selection
    /// before inserting at its start: see [`cut`].
    fn replace_across(
        &self,
        range: Range<TextPosition>,
        replacement: Vec<Paragraph>,
    ) -> Result<DocumentEdit, EditError> {
        // A caret never stands inside hidden text, such as a link's field code.
        for end in [range.start, range.end] {
            let text = self
                .paragraph(end.paragraph)
                .ok_or(EditError::InvalidRange)?;
            let at = text.byte_offset(end.offset)?;
            let hidden = |byte: usize| {
                text.spans()
                    .iter()
                    .find(|span| byte < span.end)
                    .is_some_and(|span| span.format.hidden == Some(true))
            };
            if at > 0 && hidden(at - 1) && hidden(at) {
                return Err(EditError::InvalidRange);
            }
        }
        let root = |paragraph: usize| self.starts.partition_point(|start| *start <= paragraph) - 1;
        let (first, last) = (root(range.start.paragraph), root(range.end.paragraph));
        let (kept, _) = cut(
            &self.nodes[first..=last],
            &mut self.starts[first].clone(),
            &range,
        )?;
        let mut nodes = self.nodes[..first].to_vec();
        nodes.extend(kept);
        nodes.extend_from_slice(&self.nodes[last + 1..]);
        let mut cut = Self::from_nodes(nodes)?;
        let insertion = cut.replace(range.start..range.start, replacement)?;
        cut.apply(insertion)?;
        let end = cut.nodes.len() + last + 1 - self.nodes.len();
        Ok(DocumentEdit {
            columns: BTreeMap::new(),
            container: None,
            range: first..last + 1,
            replacement: cut.nodes.drain(first..end).collect(),
        })
    }

    /// Appends text leaf `lower`'s text to `upper`'s, keeping the upper paragraph's properties
    /// and giving it the lower one's children; None unless nothing but `upper`'s hidden subtree
    /// lies between them in one container. The lower text keeps its look under `base`, the
    /// upper paragraph's style: as OneNote stores it, a flag it leaves unset becomes false and
    /// an unset colour automatic where the style sets them. An emptied upper paragraph takes
    /// the lower text whole.
    pub(crate) fn join(
        &self,
        upper: usize,
        lower: usize,
        base: &Format,
    ) -> Result<Option<DocumentEdit>, EditError> {
        let (container, first, top) = self.leaf(upper).ok_or(EditError::InvalidRange)?;
        let (end_container, last, bottom) = self.leaf(lower).ok_or(EditError::InvalidRange)?;
        let nodes = self.container(container)?;
        let (above, below) = (&top.text().unwrap().text, &bottom.text().unwrap().text);
        if container != end_container
            || last <= first
            || last > first + 1 && !(top.collapsed && subtree_end(nodes, first) == last)
            || moves_object(above, above.utf16_offset(above.text().len())?, below, 0)?
        {
            return Ok(None);
        }
        let mut head = top.clone();
        let text = head.text_mut().unwrap();
        if above.text().is_empty() {
            // OneNote moves the lower text object, with its style and any recording link, into
            // an emptied upper paragraph, which keeps its own note tags.
            let tags = std::mem::take(&mut text.tags);
            *text = bottom.text().unwrap().clone();
            text.tags = tags;
            head.style = bottom.style;
            head.media.clone_from(&bottom.media);
        } else {
            let automatic = |color: Option<u32>| color.map(|_| 0xff000000);
            let reset = Format {
                bold: base.bold.map(|_| false),
                italic: base.italic.map(|_| false),
                underline: base.underline.map(|_| false),
                strike: base.strike.map(|_| false),
                superscript: base.superscript.map(|_| false),
                subscript: base.subscript.map(|_| false),
                hidden: base.hidden.map(|_| false),
                hyperlink: base.hyperlink.map(|_| false),
                math: base.math.map(|_| false),
                color: automatic(base.color),
                highlight: automatic(base.highlight),
                ..Format::default()
            };
            // The joined text lies in the upper paragraph, whose spacing and alignment it takes.
            let paragraph = &above.spans()[0].format;
            let mut start = 0;
            text.text
                .append(Paragraph::from_runs(below.spans().iter().map(|span| {
                    let run = below.text()[start..span.end].to_owned();
                    start = span.end;
                    let format = Format {
                        alignment: paragraph.alignment,
                        space_before: paragraph.space_before,
                        space_after: paragraph.space_after,
                        line_spacing: paragraph.line_spacing,
                        list_spacing: paragraph.list_spacing,
                        ..span.format.inherit(&reset)
                    };
                    (run, format)
                })))?;
        }
        let moves = BTreeMap::from([(bottom.id, &head)]);
        let adopted = adopt(nodes, last + 1, &moves, BTreeMap::new())?;
        Ok(Some(DocumentEdit {
            columns: BTreeMap::new(),
            container,
            range: first..last + 1 + adopted.len(),
            replacement: [head]
                .into_iter()
                .chain(nodes[first + 1..last].iter().cloned())
                .chain(adopted)
                .collect(),
        }))
    }

    /// Rejects exactly the edits after which [`validate_nodes`] would reject the document, or
    /// whose column widths or text are invalid, looking only where the edit can conflict.
    pub(crate) fn validate_edit(&self, edit: &DocumentEdit) -> Result<(), EditError> {
        let (nodes, depth) = self.nested_container(edit.container)?;
        if edit.range.start > edit.range.end
            || edit.range.end > nodes.len()
            || nodes.len() - edit.range.len() + edit.replacement.len() == 0
        {
            return Err(EditError::InvalidRange);
        }
        let mut ids = BTreeSet::new();
        validate_run(
            &edit.replacement,
            &nodes[..edit.range.start],
            depth,
            &mut ids,
        )?;
        let removed = &nodes[edit.range.clone()];
        let levels = edit
            .replacement
            .iter()
            .map(|node| (node.id, node.level))
            .collect::<BTreeMap<_, _>>();
        // A later paragraph may name a removed or deepened paragraph as its parent.
        let moved = removed
            .iter()
            .filter_map(|node| {
                let level = levels.get(&node.id).copied();
                level
                    .is_none_or(|level| level > node.level)
                    .then_some((node.id, level))
            })
            .collect::<BTreeMap<_, _>>();
        if !moved.is_empty()
            && nodes[edit.range.end..].iter().any(|node| {
                node.parent
                    .and_then(|id| moved.get(&id))
                    .is_some_and(|level| level.is_none_or(|level| level >= node.level))
            })
        {
            return Err(EditError::InvalidStructure);
        }
        // Only identities the removed nodes did not hold can collide elsewhere.
        for (_, _, node) in descendants(removed, None) {
            for id in owned_ids(node) {
                ids.remove(&id);
            }
        }
        if !ids.is_empty()
            && descendants(&self.nodes, None)
                .any(|(_, _, node)| owned_ids(node).any(|id| ids.contains(&id)))
        {
            return Err(EditError::InvalidStructure);
        }
        if !edit.columns.is_empty() {
            let replaced = descendants(&edit.replacement, None)
                .filter_map(|(_, _, node)| match &node.content {
                    ParagraphContent::Table(table) => Some(table.id),
                    _ => None,
                })
                .collect::<BTreeSet<_>>();
            let tables = descendants(&self.nodes, Some(edit))
                .filter_map(|(_, _, node)| match &node.content {
                    ParagraphContent::Table(table) => Some((table.id, table)),
                    _ => None,
                })
                .collect::<BTreeMap<_, _>>();
            for (id, widths) in &edit.columns {
                if replaced.contains(id)
                    || tables
                        .get(id)
                        .is_none_or(|table| table.columns.len() != widths.len())
                    || widths
                        .iter()
                        .any(|width| !width.is_finite() || *width < 36.0)
                {
                    return Err(EditError::InvalidStructure);
                }
            }
        }
        validate_text(&edit.replacement)
    }

    pub(crate) fn apply(&mut self, edit: DocumentEdit) -> Result<DocumentEdit, EditError> {
        self.validate_edit(&edit)?;
        Ok(self.splice(edit))
    }

    /// Applies an edit [`Self::validate_edit`] accepted, returning its inverse.
    pub(crate) fn splice(&mut self, edit: DocumentEdit) -> DocumentEdit {
        let range = edit.range.start..edit.range.start + edit.replacement.len();
        let root = edit.container.map(|cell| {
            (
                cell,
                self.root(cell).expect("a validated edit's cell exists"),
            )
        });
        let nodes = match root {
            Some((cell, root)) => cell_mut(std::slice::from_mut(&mut self.nodes[root]), cell)
                .expect("a validated edit's cell exists"),
            None => &mut self.nodes,
        };
        let replacement: Vec<_> = nodes.splice(edit.range.clone(), edit.replacement).collect();
        let delta = leaves(&nodes[range.clone()], None).count() as isize
            - leaves(&replacement, None).count() as isize;
        let after = match root {
            Some((_, root)) => root + 1,
            None => {
                let first = self.starts[range.start];
                self.starts.splice(
                    edit.range.clone(),
                    starts(&self.nodes[range.clone()], first),
                );
                let [start, end] = [edit.range.start, edit.range.end]
                    .map(|index| self.tables.partition_point(|table| *table < index));
                let added = tables(&self.nodes[range.clone()], range.start).collect::<Vec<_>>();
                let later = start + added.len();
                self.tables.splice(start..end, added);
                for table in &mut self.tables[later..] {
                    *table =
                        table.wrapping_add_signed(range.len() as isize - edit.range.len() as isize);
                }
                range.end
            }
        };
        if delta != 0 {
            for start in &mut self.starts[after..] {
                *start = start.wrapping_add_signed(delta);
            }
        }
        let mut columns = edit.columns;
        swap_columns(&mut self.nodes, &mut columns);
        DocumentEdit {
            container: edit.container,
            range,
            replacement,
            columns,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use onestore::document::Format;
    use onestore::page::{Table, TableCell, TableColumn, TableRow};

    fn position(paragraph: usize, offset: u32) -> TextPosition {
        TextPosition { paragraph, offset }
    }

    fn table_document() -> TextDocument {
        let format = Format::default();
        let mut cells = Vec::new();
        for texts in [vec!["a🌲b", "second"], vec!["right"]] {
            cells.push(TableCell {
                id: new_id().unwrap(),
                layout: Default::default(),
                indents: vec![18.0, 0.0, 27.0, 27.0],
                shading: None,
                paragraphs: texts
                    .into_iter()
                    .map(|text| {
                        node(Paragraph::new(text.into(), format.clone()), format.clone()).unwrap()
                    })
                    .collect(),
                unsupported: Vec::new(),
            });
        }
        let table = PageParagraph {
            id: new_id().unwrap(),
            parent: None,
            level: 1,
            style: None,
            format: format.clone(),
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
                rows: vec![TableRow {
                    id: new_id().unwrap(),
                    cells,
                }],
                borders: Some(true),
                layout: Default::default(),
                tags: Vec::new(),
            }),
        };
        TextDocument::from_nodes(vec![
            node(
                Paragraph::new("before".into(), format.clone()),
                format.clone(),
            )
            .unwrap(),
            table,
            node(Paragraph::new("after".into(), format.clone()), format).unwrap(),
        ])
        .unwrap()
    }

    #[test]
    fn text_and_column_widths_share_an_exact_inverse() {
        let mut document = table_document();
        let source = document.clone();
        let ParagraphContent::Table(table) = &source.nodes()[1].content else {
            panic!()
        };
        let id = table.id;
        let neighbor = document.paragraphs().nth(3).unwrap().text().as_ptr();
        let mut edit = document
            .replace(
                position(1, 1)..position(1, 1),
                vec![Paragraph::new("X".into(), Format::default())],
            )
            .unwrap();
        edit.columns.insert(id, vec![81.24, 36.0]);
        let mut undo = document.apply(edit).unwrap();
        assert_eq!(undo.columns[&id], [72.0, 72.0]);
        let after = document.clone();
        for _ in 0..3 {
            let redo = document.apply(undo).unwrap();
            assert_eq!(document, source);
            assert_eq!(
                document.paragraphs().nth(3).unwrap().text().as_ptr(),
                neighbor
            );
            undo = document.apply(redo).unwrap();
            assert_eq!(document, after);
            assert_eq!(
                document.paragraphs().nth(3).unwrap().text().as_ptr(),
                neighbor
            );
        }
        let resize = DocumentEdit {
            container: None,
            range: 0..0,
            replacement: Vec::new(),
            columns: [(id, vec![37.11, 99.875])].into(),
        };
        let undo = document.apply(resize).unwrap();
        document.apply(undo).unwrap();
        assert_eq!(document, after);
    }

    #[test]
    fn invalid_column_patches_do_not_partially_edit_text() {
        let mut document = table_document();
        let source = document.clone();
        let ParagraphContent::Table(table) = &source.nodes()[1].content else {
            panic!()
        };
        let id = table.id;
        for widths in [
            vec![],
            vec![72.0],
            vec![72.0, 72.0, 72.0],
            vec![72.0, 35.99],
            vec![f32::NAN, 72.0],
            vec![f32::INFINITY, 72.0],
        ] {
            let mut edit = document
                .replace(
                    position(1, 1)..position(1, 1),
                    vec![Paragraph::new("X".into(), Format::default())],
                )
                .unwrap();
            edit.columns.insert(id, widths);
            assert!(document.apply(edit).is_err());
            assert_eq!(document, source);
        }
        for (range, replacement, id) in [
            (0..0, Vec::new(), new_id().unwrap()),
            (1..2, Vec::new(), id),
            (1..2, vec![source.nodes()[1].clone()], id),
        ] {
            let edit = DocumentEdit {
                container: None,
                range,
                replacement,
                columns: [(id, vec![81.0, 90.0])].into(),
            };
            assert!(document.apply(edit).is_err());
            assert_eq!(document, source);
        }
    }

    #[test]
    fn cell_text_edits_preserve_the_table_and_restore_paragraph_identity() {
        let mut document = table_document();
        let original = document.clone();
        let ParagraphContent::Table(table) = &document.nodes()[1].content else {
            panic!()
        };
        let cell = table.rows[0].cells[0].id;
        let other_text = table.rows[0].cells[1].paragraphs[0]
            .text()
            .unwrap()
            .text
            .text()
            .as_ptr();
        assert_eq!(
            document
                .paragraphs()
                .map(Paragraph::text)
                .collect::<Vec<_>>(),
            ["before", "a🌲b", "second", "right", "after"]
        );
        let shown = |nodes: Vec<PageParagraph>| {
            leaves(&nodes, None)
                .map(|(.., node)| node.text().unwrap().text.text().to_owned())
                .collect::<Vec<_>>()
        };
        let selected = |range| shown(document.selected(range).unwrap());
        assert_eq!(selected(position(1, 1)..position(2, 3)), ["🌲b", "sec"]);
        assert_eq!(
            selected(position(1, 1)..position(3, 2)),
            ["a🌲b", "second", "right"]
        );
        assert_eq!(
            selected(position(0, 2)..position(4, 1)),
            ["fore", "a🌲b", "second", "right", "a"]
        );
        let edit = document
            .replace(
                position(1, 1)..position(2, 3),
                vec![Paragraph::new("NEW".into(), Format::default())],
            )
            .unwrap();
        assert_eq!(edit.container, Some(cell));
        assert_eq!(edit.range, 0..2);
        let undo = document.apply(edit).unwrap();
        assert_eq!(
            document
                .paragraphs()
                .map(Paragraph::text)
                .collect::<Vec<_>>(),
            ["before", "aNEWond", "right", "after"]
        );
        let ParagraphContent::Table(table) = &document.nodes()[1].content else {
            panic!()
        };
        assert_eq!(
            table.rows[0].cells[1].paragraphs[0]
                .text()
                .unwrap()
                .text
                .text()
                .as_ptr(),
            other_text
        );
        assert_eq!(
            document.text_nodes().nth(1).unwrap().id,
            original.text_nodes().nth(1).unwrap().id
        );
        assert_eq!(undo.container, Some(cell));
        assert_eq!(undo.range, 0..1);
        let redo = document.apply(undo).unwrap();
        assert_eq!(document, original);
        let undo = document.apply(redo).unwrap();
        document.apply(undo).unwrap();
        assert_eq!(document, original);
    }

    #[test]
    fn surrounding_text_splits_without_flattening_the_table() {
        let mut document = table_document();
        let original = document.clone();
        let edit = document
            .replace(
                position(4, 2)..position(4, 2),
                vec![Paragraph::new(String::new(), Format::default()); 2],
            )
            .unwrap();
        assert_eq!(edit.container, None);
        assert_eq!(edit.range, 2..3);
        let undo = document.apply(edit).unwrap();
        assert_eq!(
            document
                .paragraphs()
                .map(Paragraph::text)
                .collect::<Vec<_>>(),
            ["before", "a🌲b", "second", "right", "af", "ter"]
        );
        assert_eq!(document.nodes()[1], original.nodes()[1]);
        document.apply(undo).unwrap();
        assert_eq!(document, original);
        let edit = document
            .replace(
                position(0, 0)..position(4, 0),
                vec![Paragraph::new(String::new(), Format::default())],
            )
            .unwrap();
        document.apply(edit).unwrap();
        assert_eq!(
            document
                .paragraphs()
                .map(Paragraph::text)
                .collect::<Vec<_>>(),
            ["", "after"]
        );
        assert_eq!(document.nodes().len(), 2);
    }

    #[test]
    fn cell_edits_validate_utf16_and_container_boundaries_before_mutation() {
        let mut document = table_document();
        let original = document.clone();
        let text = || vec![Paragraph::new("X".into(), Format::default())];
        assert_eq!(
            document.replace(position(1, 2)..position(1, 3), text()),
            Err(EditError::InvalidRange)
        );
        let across = document
            .replace(position(1, 1)..position(3, 1), text())
            .unwrap();
        let mut deleted = document.clone();
        deleted.apply(across).unwrap();
        assert_eq!(
            deleted
                .paragraphs()
                .map(Paragraph::text)
                .collect::<Vec<_>>(),
            ["before", "aX", "ight", "after"],
        );
        let ParagraphContent::Table(table) = &document.nodes()[1].content else {
            panic!()
        };
        let cell = table.rows[0].cells[1].id;
        for edit in [
            DocumentEdit {
                columns: BTreeMap::new(),
                container: Some(cell),
                range: 0..1,
                replacement: Vec::new(),
            },
            DocumentEdit {
                columns: BTreeMap::new(),
                container: Some(new_id().unwrap()),
                range: 0..1,
                replacement: vec![document.nodes()[0].clone()],
            },
            DocumentEdit {
                columns: BTreeMap::new(),
                container: Some(cell),
                range: 0..usize::MAX,
                replacement: Vec::new(),
            },
        ] {
            assert_eq!(document.apply(edit), Err(EditError::InvalidRange));
            assert_eq!(document, original);
        }
        let alias = DocumentEdit {
            columns: BTreeMap::new(),
            container: Some(cell),
            range: 0..1,
            replacement: vec![document.nodes()[0].clone()],
        };
        assert_eq!(document.apply(alias), Err(EditError::InvalidStructure));
        assert_eq!(document, original);
    }

    #[test]
    fn nested_cell_edits_keep_leaf_order_and_split_paragraphs_locally() {
        let mut document = table_document();
        let nested = table_document().nodes.remove(1);
        let ParagraphContent::Table(table) = &document.nodes()[1].content else {
            panic!()
        };
        let parent = table.rows[0].cells[0].id;
        document
            .apply(DocumentEdit {
                columns: BTreeMap::new(),
                container: Some(parent),
                range: 0..2,
                replacement: vec![nested],
            })
            .unwrap();
        let original = document.clone();
        let edit = document
            .replace(
                position(2, 3)..position(2, 3),
                vec![
                    Paragraph::new("X".into(), Format::default()),
                    Paragraph::new("Y".into(), Format::default()),
                ],
            )
            .unwrap();
        assert_ne!(edit.container, Some(parent));
        assert_eq!(edit.range, 1..2);
        let undo = document.apply(edit).unwrap();
        assert_eq!(
            document
                .paragraphs()
                .map(Paragraph::text)
                .collect::<Vec<_>>(),
            ["before", "a🌲b", "secX", "Yond", "right", "right", "after"]
        );
        document.apply(undo).unwrap();
        assert_eq!(document, original);
    }

    #[test]
    #[ignore = "requires CANVAS_TEST_SECTION and CANVAS_TEST_PAGE private fixture inputs"]
    fn imported_nodes_preserve_identity_through_edit_and_undo() {
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
        let mut outlines = 0;
        let mut paragraphs = 0;
        for outline in page.objects.iter().flat_map(|object| match object {
            PageObject::Outline(outline) => std::slice::from_ref(outline),
            PageObject::Title(title) => &title.outlines,
            _ => &[],
        }) {
            let original = TextDocument::from_nodes(outline.paragraphs.clone()).unwrap();
            for paragraph in 0..original.nodes().len() {
                let mut document = original.clone();
                let position = TextPosition {
                    paragraph,
                    offset: 0,
                };
                let format = document
                    .paragraphs()
                    .nth(paragraph)
                    .unwrap()
                    .format_at(0)
                    .unwrap()
                    .clone();
                let edit = document
                    .replace(
                        position..position,
                        vec![Paragraph::new("probe".into(), format)],
                    )
                    .unwrap();
                let undo = document.apply(edit).unwrap();
                let edited = document.clone();
                assert_eq!(edited.nodes()[paragraph].id, original.nodes()[paragraph].id);
                assert_eq!(
                    edited.nodes()[paragraph].text().unwrap().id,
                    original.nodes()[paragraph].text().unwrap().id
                );
                let mut restored_text = edited.nodes().to_vec();
                restored_text[paragraph].text_mut().unwrap().text =
                    original.nodes()[paragraph].text().unwrap().text.clone();
                assert_eq!(restored_text, original.nodes());
                let redo = document.apply(undo).unwrap();
                assert_eq!(document, original);
                document.apply(redo).unwrap();
                assert_eq!(document, edited);
                paragraphs += 1;
            }
            assert_eq!(outline.paragraphs, original.nodes());
            outlines += 1;
        }
        assert!(outlines > 0);
        eprintln!(
            "Edited and restored {outlines} imported outlines containing {paragraphs} paragraphs"
        );
    }

    #[test]
    fn structural_edits_preserve_surviving_node_identity_and_undo_all_metadata() {
        let mut source = ["A🌲B", "", "tail"]
            .into_iter()
            .enumerate()
            .map(|(index, text)| {
                node(
                    Paragraph::new(text.into(), Format::default()),
                    Format {
                        language: Some(1033 + index as u32),
                        ..Format::default()
                    },
                )
                .unwrap()
            })
            .collect::<Vec<_>>();
        source[2].text_mut().unwrap().text = Paragraph::new(
            "tail".into(),
            Format {
                bold: Some(true),
                ..Format::default()
            },
        );
        let original = TextDocument::from_nodes(source).unwrap();
        let mut document = original.clone();
        let edit = document
            .replace(
                position(0, 1)..position(2, 2),
                ["x", "y", "z"]
                    .into_iter()
                    .map(|s| Paragraph::new(s.into(), Format::default()))
                    .collect(),
            )
            .unwrap();
        let undo = document.apply(edit).unwrap();
        assert_eq!(
            document
                .paragraphs()
                .map(Paragraph::text)
                .collect::<Vec<_>>(),
            ["Ax", "y", "zil"]
        );
        for index in [0, 2] {
            assert_eq!(document.nodes()[index].id, original.nodes()[index].id);
            assert_eq!(
                document.nodes()[index].text().unwrap().id,
                original.nodes()[index].text().unwrap().id
            );
            assert_eq!(
                document.nodes()[index].format,
                original.nodes()[index].format
            );
        }
        let old_ids: BTreeSet<_> = original
            .nodes()
            .iter()
            .flat_map(|n| [n.id, n.text().unwrap().id])
            .collect();
        assert!(!old_ids.contains(&document.nodes()[1].id));
        assert!(!old_ids.contains(&document.nodes()[1].text().unwrap().id));
        let edited = document.clone();
        let redo = document.apply(undo).unwrap();
        assert_eq!(document, original);
        document.apply(redo).unwrap();
        assert_eq!(document, edited);
        let mut split = original.clone();
        let edit = split
            .replace(
                position(0, 1)..position(0, 1),
                vec![Paragraph::new(String::new(), Format::default()); 2],
            )
            .unwrap();
        split.apply(edit).unwrap();
        assert_eq!(split.nodes()[0].id, original.nodes()[0].id);
        assert!(!old_ids.contains(&split.nodes()[1].id));
        assert_eq!(&split.nodes()[2..], &original.nodes()[1..]);
    }

    #[test]
    fn replacement_cannot_alias_existing_objects() {
        let mut document =
            TextDocument::new(vec![Paragraph::new("text".into(), Format::default())]).unwrap();
        let original = document.clone();
        let duplicate = DocumentEdit {
            columns: BTreeMap::new(),
            container: None,
            range: 1..1,
            replacement: original.nodes().to_vec(),
        };
        assert_eq!(document.apply(duplicate), Err(EditError::InvalidStructure));
        let mut unsupported = original.nodes().to_vec();
        unsupported[0].content =
            onestore::page::ParagraphContent::Unsupported(onestore::page::Unsupported {
                id: new_id().unwrap(),
                jcid: 0x60012,
                layout: Default::default(),
            });
        // Content the canvas cannot draw is kept as a placeholder paragraph, once.
        assert!(TextDocument::from_nodes(unsupported.clone()).is_ok());
        let mut twice = unsupported.clone();
        twice[0].id = new_id().unwrap();
        twice.extend(unsupported);
        assert_eq!(
            TextDocument::from_nodes(twice),
            Err(EditError::InvalidStructure)
        );
        assert_eq!(document, original);
    }

    #[test]
    fn paragraph_links_require_an_earlier_parent_at_a_shallower_level() {
        let mut nodes = TextDocument::new(
            ["parent", "child", "grandchild"]
                .into_iter()
                .map(|text| Paragraph::new(text.into(), Format::default()))
                .collect(),
        )
        .unwrap()
        .nodes;
        nodes[1].parent = Some(nodes[0].id);
        nodes[1].level = 3;
        nodes[2].parent = Some(nodes[1].id);
        nodes[2].level = 5;
        let original = TextDocument::from_nodes(nodes.clone()).unwrap();
        for parent in [nodes[2].id, nodes[0].text().unwrap().id, new_id().unwrap()] {
            let mut invalid = nodes.clone();
            invalid[1].parent = Some(parent);
            assert_eq!(
                TextDocument::from_nodes(invalid),
                Err(EditError::InvalidStructure)
            );
        }
        for level in [0, 1] {
            let mut invalid = nodes.clone();
            invalid[1].level = level;
            assert_eq!(
                TextDocument::from_nodes(invalid),
                Err(EditError::InvalidStructure)
            );
        }
        let mut document = original.clone();
        assert_eq!(
            document.apply(DocumentEdit {
                columns: BTreeMap::new(),
                container: None,
                range: 0..1,
                replacement: Vec::new()
            }),
            Err(EditError::InvalidStructure)
        );
        assert_eq!(document, original);
    }

    #[test]
    fn editing_nested_tagged_text_preserves_metadata_through_structural_changes() {
        let mut nodes = TextDocument::new(
            ["parent", "a🌳e\u{301}z", "child"]
                .into_iter()
                .map(|text| Paragraph::new(text.into(), Format::default()))
                .collect(),
        )
        .unwrap()
        .nodes;
        nodes[1].parent = Some(nodes[0].id);
        nodes[1].level = 2;
        nodes[1].lists.push(new_id().unwrap());
        nodes[1].tags.push(onestore::document::Tag {
            definition: Some(new_id().unwrap()),
            action_type: Some(0),
            shape: None,
            property_status: None,
            status: 1,
            created: Some(123),
            completed: None,
            start: Some(124),
            due: Some(456),
            task_id: Some([7; 16]),
            extra_set: 0,
        });
        nodes[1].text_mut().unwrap().tags = nodes[1].tags.clone();
        nodes[1].collapsed = true;
        nodes[2].parent = Some(nodes[1].id);
        nodes[2].level = 3;
        let original = TextDocument::from_nodes(nodes).unwrap();
        let text = original.paragraphs().nth(1).unwrap();
        let offsets: Vec<_> = text
            .text()
            .char_indices()
            .map(|(byte, _)| text.utf16_offset(byte).unwrap())
            .chain(std::iter::once(
                text.utf16_offset(text.text().len()).unwrap(),
            ))
            .collect();
        for &start in &offsets {
            for &end in offsets.iter().filter(|&&end| end >= start) {
                let mut document = original.clone();
                let edit = document
                    .replace(
                        position(1, start)..position(1, end),
                        vec![Paragraph::new(
                            "👩🏽‍💻".into(),
                            Format {
                                bold: Some(true),
                                ..Format::default()
                            },
                        )],
                    )
                    .unwrap();
                let undo = document.apply(edit).unwrap();
                let edited = document.clone();
                let mut restored = edited.nodes().to_vec();
                restored[1].text_mut().unwrap().text = text.clone();
                assert_eq!(restored, original.nodes());
                let redo = document.apply(undo).unwrap();
                assert_eq!(document, original);
                document.apply(redo).unwrap();
                assert_eq!(document, edited);
            }
        }
        let structure = |range: Range<TextPosition>, count| {
            let mut document = original.clone();
            let edit = document
                .replace(
                    range,
                    vec![Paragraph::new(String::new(), Format::default()); count],
                )
                .unwrap();
            let undo = document.apply(edit).unwrap();
            let edited = document.nodes().to_vec();
            document.apply(undo).unwrap();
            assert_eq!(document, original);
            edited
        };
        let [parent, item, child] = original.nodes() else {
            unreachable!()
        };
        // The new half takes the list, the children and their collapsed state, not the tags.
        let split = structure(position(1, 0)..position(1, 0), 2);
        assert_eq!(split[1].id, item.id);
        assert!(split[1].text().unwrap().text.text().is_empty() && !split[1].collapsed);
        assert_eq!(split[1].tags, item.tags);
        let tail = &split[2];
        assert_eq!(tail.text().unwrap().text, item.text().unwrap().text);
        assert_eq!((tail.parent, tail.level), (Some(parent.id), 2));
        assert_eq!(tail.lists, item.lists);
        assert!(tail.tags.is_empty() && tail.text().unwrap().tags.is_empty() && tail.collapsed);
        assert_eq!((split[3].parent, split[3].level), (Some(tail.id), 3));
        // The upper paragraph wins a join and adopts the lower one's children.
        let joined = structure(position(0, 6)..position(1, 0), 1);
        assert_eq!(joined[0].text().unwrap().text.text(), "parenta🌳e\u{301}z");
        assert_eq!((joined[0].id, joined[0].lists.len()), (parent.id, 0));
        assert_eq!(
            (joined[1].id, joined[1].parent, joined[1].level),
            (child.id, Some(parent.id), 2)
        );
        let joined = structure(position(1, 6)..position(2, 0), 1);
        assert_eq!(joined.len(), 2);
        assert_eq!(
            PageParagraph {
                content: item.content.clone(),
                ..joined[1].clone()
            },
            *item
        );
    }

    #[test]
    fn split_join_and_undo_preserve_empty_paragraph_formats() {
        let regular = Format::default();
        let bold = Format {
            bold: Some(true),
            ..Format::default()
        };
        let original = TextDocument::new(vec![
            Paragraph::new("left".into(), regular.clone()),
            Paragraph::new(String::new(), bold.clone()),
            Paragraph::new("right".into(), bold.clone()),
        ])
        .unwrap();
        let mut document = original.clone();
        let split = document
            .replace(
                position(0, 2)..position(0, 2),
                vec![Paragraph::new(String::new(), regular.clone()); 2],
            )
            .unwrap();
        let undo = document.apply(split).unwrap();
        assert_eq!(
            document
                .paragraphs()
                .map(Paragraph::text)
                .collect::<Vec<_>>(),
            ["le", "ft", "", "right"]
        );
        let redo = document.apply(undo).unwrap();
        assert_eq!(document, original);
        document.apply(redo).unwrap();
        let join = document
            .replace(
                position(1, 2)..position(3, 0),
                vec![Paragraph::new(String::new(), regular)],
            )
            .unwrap();
        let before_join = document.clone();
        let undo = document.apply(join).unwrap();
        assert_eq!(document.paragraphs().nth(1).unwrap().text(), "ftright");
        assert_eq!(
            document.paragraphs().nth(1).unwrap().spans()[1].format,
            bold
        );
        document.apply(undo).unwrap();
        assert_eq!(document, before_join);
    }

    #[test]
    fn every_scalar_range_replaces_across_paragraphs_and_round_trips() {
        let regular = Format::default();
        let bold = Format {
            bold: Some(true),
            ..Format::default()
        };
        let original = TextDocument::new(vec![
            Paragraph::from_runs([
                ("a🌳".into(), regular.clone()),
                ("e\u{301}".into(), bold.clone()),
            ]),
            Paragraph::new(String::new(), bold.clone()),
            Paragraph::new("日本z".into(), regular.clone()),
        ])
        .unwrap();
        for (original, regions) in [
            (original, vec![0, 0, 0]),
            (table_document(), vec![0, 1, 1, 2, 0]),
        ] {
            let positions: Vec<_> = original
                .paragraphs()
                .enumerate()
                .flat_map(|(index, paragraph)| {
                    (0..=paragraph.text().len()).filter_map(move |byte| {
                        paragraph
                            .utf16_offset(byte)
                            .ok()
                            .map(|offset| position(index, offset))
                    })
                })
                .collect();
            let replacements = [
                vec![Paragraph::new(String::new(), regular.clone())],
                vec![Paragraph::new("x👩🏽‍💻".into(), bold.clone())],
                vec![
                    Paragraph::new(String::new(), bold.clone()),
                    Paragraph::new(String::new(), regular.clone()),
                ],
                vec![
                    Paragraph::new("one".into(), bold.clone()),
                    Paragraph::new(String::new(), regular.clone()),
                    Paragraph::new("two".into(), bold.clone()),
                ],
            ];
            let plain = original
                .paragraphs()
                .map(Paragraph::text)
                .collect::<Vec<_>>()
                .join("\n");
            for &start in &positions {
                for &end in positions.iter().filter(|&&end| end >= start) {
                    for replacement in &replacements {
                        let mut document = original.clone();
                        let planned = document.replace(start..end, replacement.clone());
                        if regions[start.paragraph..=end.paragraph]
                            .iter()
                            .any(|region| *region != regions[start.paragraph])
                        {
                            // Across a container's edge nothing joins: the start keeps what
                            // precedes it, followed by the replacement, and the end what
                            // follows it.
                            let planned = planned.unwrap();
                            assert_eq!(document, original);
                            let undo = document.apply(planned).unwrap();
                            let texts: Vec<_> =
                                document.paragraphs().map(Paragraph::text).collect();
                            let before: Vec<_> =
                                original.paragraphs().map(Paragraph::text).collect();
                            assert_eq!(texts[..start.paragraph], before[..start.paragraph]);
                            let text = |position: TextPosition| before[position.paragraph];
                            let byte = |position: TextPosition| {
                                original
                                    .paragraphs()
                                    .nth(position.paragraph)
                                    .unwrap()
                                    .byte_offset(position.offset)
                                    .unwrap()
                            };
                            let mut inserted: Vec<String> =
                                replacement.iter().map(|p| p.text().to_owned()).collect();
                            inserted[0].insert_str(0, &text(start)[..byte(start)]);
                            let at = start.paragraph;
                            assert_eq!(texts[at..at + inserted.len()], inserted);
                            assert!(
                                texts[at + inserted.len()..].contains(&&text(end)[byte(end)..])
                            );
                            let after = document.clone();
                            let redo = document.apply(undo).unwrap();
                            assert_eq!(document, original, "{start:?}..{end:?}");
                            document.apply(redo).unwrap();
                            assert_eq!(document, after);
                            continue;
                        }
                        let planned = planned.unwrap();
                        assert_eq!(document, original);
                        let undo = document.apply(planned).unwrap();
                        assert!(document.paragraphs().next().is_some());
                        let byte = |position: TextPosition| {
                            original
                                .paragraphs()
                                .take(position.paragraph)
                                .map(|p| p.text().len() + 1)
                                .sum::<usize>()
                                + original
                                    .paragraphs()
                                    .nth(position.paragraph)
                                    .unwrap()
                                    .byte_offset(position.offset)
                                    .unwrap()
                        };
                        let mut expected = plain.clone();
                        expected.replace_range(
                            byte(start)..byte(end),
                            &replacement
                                .iter()
                                .map(Paragraph::text)
                                .collect::<Vec<_>>()
                                .join("\n"),
                        );
                        assert_eq!(
                            document
                                .paragraphs()
                                .map(Paragraph::text)
                                .collect::<Vec<_>>()
                                .join("\n"),
                            expected
                        );
                        let after = document.clone();
                        let redo = document.apply(undo).unwrap();
                        assert_eq!(document, original, "{start:?}..{end:?}");
                        document.apply(redo).unwrap();
                        assert_eq!(document, after);
                    }
                }
            }
        }
    }

    /// Validates the materialized edited document from scratch.
    fn full_validation(document: &TextDocument, edit: &DocumentEdit) -> Result<(), EditError> {
        let mut nodes = document.nodes.clone();
        let container = container_mut(&mut nodes, edit.container).ok_or(EditError::InvalidRange)?;
        if edit.range.start > edit.range.end || edit.range.end > container.len() {
            return Err(EditError::InvalidRange);
        }
        container.splice(edit.range.clone(), edit.replacement.iter().cloned());
        validate_nodes(&nodes, &mut BTreeSet::new())?;
        let tables = |nodes| {
            descendants(nodes, None)
                .filter_map(|(_, _, node)| match &node.content {
                    ParagraphContent::Table(table) => Some((table.id, table.columns.len())),
                    _ => None,
                })
                .collect::<BTreeMap<_, _>>()
        };
        let replaced = tables(&edit.replacement);
        let tables = tables(&nodes);
        for (id, widths) in &edit.columns {
            if replaced.contains_key(id)
                || tables.get(id) != Some(&widths.len())
                || widths
                    .iter()
                    .any(|width| !width.is_finite() || *width < 36.0)
            {
                return Err(EditError::InvalidStructure);
            }
        }
        validate_text(&edit.replacement)
    }

    #[test]
    fn incremental_validation_matches_full_validation_of_random_edits() {
        let mut seed = 0x9e37_79b9_u64;
        let mut next = |n: usize| {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (seed >> 33) as usize % n.max(1)
        };
        let fresh = |text: &str| {
            node(
                Paragraph::new(text.into(), Format::default()),
                Format::default(),
            )
            .unwrap()
        };
        let mut document = table_document();
        let [mut accepted, mut rejected] = [0; 2];
        for step in 0..4000 {
            let all = descendants(&document.nodes, None)
                .map(|(_, _, node)| node.clone())
                .collect::<Vec<_>>();
            let cells = all
                .iter()
                .filter_map(|node| match &node.content {
                    ParagraphContent::Table(table) => Some(table),
                    _ => None,
                })
                .flat_map(|table| table.rows.iter().flat_map(|row| &row.cells))
                .map(|cell| Some(cell.id))
                .collect::<Vec<_>>();
            let tables = all
                .iter()
                .filter_map(|node| match &node.content {
                    ParagraphContent::Table(table) => Some(table.id),
                    _ => None,
                })
                .collect::<Vec<_>>();
            let container = match next(8) {
                0..4 => None,
                7 if step % 5 == 0 => Some(new_id().unwrap()),
                _ => cells.get(next(cells.len())).copied().flatten(),
            };
            let nodes = document.container(container).unwrap_or(&[]).to_vec();
            let start = next(nodes.len() + 2);
            let range = if next(16) == 0 {
                start..start.saturating_sub(1)
            } else {
                start..(start + next(3)).min(nodes.len() + usize::from(next(8) == 0))
            };
            let removed = nodes.get(range.clone()).unwrap_or_default();
            let prefix = &nodes[..range.start.min(nodes.len())];
            let suffix = nodes.get(range.end..).unwrap_or_default();
            let ids =
                |nodes: &[PageParagraph]| nodes.iter().map(|node| node.id).collect::<Vec<_>>();
            let parents = [ids(prefix), ids(removed), ids(suffix), ids(&all)].concat();
            let mut replacement = Vec::new();
            for _ in 0..next(4) {
                let mut node = match next(10) {
                    0..3 if !removed.is_empty() => removed[next(removed.len())].clone(),
                    3 if !all.is_empty() => all[next(all.len())].clone(),
                    4 => {
                        let mut table = table_document().nodes.remove(1);
                        let ParagraphContent::Table(inner) = &mut table.content else {
                            unreachable!()
                        };
                        match next(6) {
                            0 => inner.rows[0].cells[0].paragraphs.clear(),
                            1 => inner.columns[0].width = 35.0,
                            2 => {
                                inner.rows[0].cells.pop();
                            }
                            3 => {
                                inner.rows[0].cells[1].paragraphs[0].parent =
                                    Some(inner.rows[0].cells[0].paragraphs[0].id);
                            }
                            _ => {}
                        }
                        table
                    }
                    _ => fresh("new"),
                };
                match next(6) {
                    0 => node.level = next(4) as u32,
                    1 => node.level += 1,
                    2 => {
                        node.level = 1 + next(4) as u32;
                        node.parent = (!parents.is_empty()).then(|| parents[next(parents.len())]);
                    }
                    3 => node.parent = None,
                    _ => {}
                }
                replacement.push(node);
            }
            let mut columns = BTreeMap::new();
            if next(12) == 0 {
                let id = tables
                    .get(next(tables.len() + 1))
                    .copied()
                    .unwrap_or_else(|| new_id().unwrap());
                columns.insert(id, vec![[72.0, 30.0, f32::NAN][next(3)]; 1 + next(3)]);
            }
            let edit = if next(4) == 0 {
                let positions = document.paragraphs().count();
                let paragraph = next(positions);
                let offset = document.paragraphs().nth(paragraph).unwrap().text().len() as u32;
                let start = TextPosition {
                    paragraph,
                    offset: next(offset as usize + 1) as u32,
                };
                let end = TextPosition {
                    paragraph: paragraph + usize::from(paragraph + 1 < positions && next(2) == 0),
                    offset: 0,
                };
                let texts = vec![Paragraph::new("typed".into(), Format::default()); 1 + next(2)];
                match document.replace(start..end.max(start), texts) {
                    Ok(edit) => edit,
                    Err(_) => continue,
                }
            } else {
                DocumentEdit {
                    container,
                    range,
                    replacement,
                    columns,
                }
            };
            let expected = full_validation(&document, &edit);
            assert_eq!(
                document.validate_edit(&edit).is_ok(),
                expected.is_ok(),
                "step {step}: {expected:?} for {edit:#?}"
            );
            if expected.is_ok() {
                accepted += 1;
                let edited = leaves(&document.nodes, Some(&edit)).collect::<Vec<_>>();
                let supplied = descendants(&edit.replacement, None)
                    .map(|(_, _, node)| node.id)
                    .collect::<BTreeSet<_>>();
                for paragraph in 0..=edited.len() {
                    let leaf = document.edited_leaf(&edit, paragraph);
                    assert_eq!(
                        leaf.map(|(node, _)| node),
                        edited.get(paragraph).map(|(_, _, node)| *node)
                    );
                    if let Some((node, before)) = leaf {
                        assert_eq!(before.is_none(), supplied.contains(&node.id));
                        assert!(
                            before.is_none_or(|before| document.leaf(before).unwrap().2 == node)
                        );
                    }
                }
                document.apply(edit).unwrap();
                assert_eq!(
                    document,
                    TextDocument::from_nodes(document.nodes.clone()).unwrap()
                );
                let walked = leaves(&document.nodes, None).collect::<Vec<_>>();
                for paragraph in 0..=walked.len() {
                    assert_eq!(document.leaf(paragraph), walked.get(paragraph).copied());
                }
            } else {
                rejected += 1;
            }
        }
        assert!(accepted > 500 && rejected > 500, "{accepted} {rejected}");
        let nest = |mut node, depth| {
            for _ in 0..depth {
                let mut table = table_document().nodes.remove(1);
                let ParagraphContent::Table(inner) = &mut table.content else {
                    unreachable!()
                };
                inner.rows[0].cells[0].paragraphs = vec![node];
                node = table;
            }
            node
        };
        let document = TextDocument::from_nodes(vec![nest(fresh("leaf"), 64)]).unwrap();
        let (innermost, _, _) = leaves(&document.nodes, None)
            .find(|(_, _, node)| node.text().unwrap().text.text() == "leaf")
            .unwrap();
        for (container, depth, valid) in [
            (innermost, 0, true),
            (innermost, 1, false),
            (None, 64, true),
            (None, 65, false),
        ] {
            let edit = DocumentEdit {
                container,
                range: 0..0,
                replacement: vec![nest(fresh("deep"), depth)],
                columns: BTreeMap::new(),
            };
            assert_eq!(
                document.validate_edit(&edit).is_ok(),
                full_validation(&document, &edit).is_ok()
            );
            assert_eq!(document.validate_edit(&edit).is_ok(), valid);
        }
    }

    #[test]
    fn invalid_ranges_and_last_paragraph_removal_are_atomic() {
        let original =
            TextDocument::new(vec![Paragraph::new("🌳".into(), Format::default())]).unwrap();
        for range in [
            position(0, 1)..position(0, 2),
            position(0, 0)..position(1, 0),
            position(0, 2)..position(0, 0),
        ] {
            assert!(
                original
                    .replace(
                        range,
                        vec![Paragraph::new(String::new(), Format::default())]
                    )
                    .is_err()
            );
        }
        for range in [0..1, 0..2, Range { start: 2, end: 1 }] {
            let mut document = original.clone();
            assert!(
                document
                    .apply(DocumentEdit {
                        columns: BTreeMap::new(),
                        container: None,
                        range,
                        replacement: Vec::new()
                    })
                    .is_err()
            );
            assert_eq!(document, original);
        }
        assert!(TextDocument::new(Vec::new()).is_err());
    }

    #[test]
    #[ignore = "requires CANVAS_TEST_SECTION pointing to the native Tab table capture"]
    fn native_table_import() {
        use onestore::{
            RevisionIndex, Store,
            document::Document,
            page::{Page, PageObject, ParagraphContent},
        };
        let page = {
            let bytes = std::fs::read(std::env::var_os("CANVAS_TEST_SECTION").unwrap()).unwrap();
            let store = Store::parse(&bytes).unwrap();
            let index = RevisionIndex::parse(&store).unwrap();
            let document = Document::parse(&index).unwrap();
            for (space, page) in document.pages().unwrap() {
                let space = &document.spaces[&space];
                let revision = &space.revisions[&space.contexts[&ExGuid::default()]];
                Page::from_revision(revision, page).unwrap();
            }
            Page::from_document(&document, "rows").unwrap()
        };
        let table = page
            .objects
            .iter()
            .find_map(|object| match object {
                PageObject::Outline(outline) => {
                    outline.paragraphs.iter().find_map(|p| match &p.content {
                        ParagraphContent::Table(table) => Some(table),
                        _ => None,
                    })
                }
                _ => None,
            })
            .unwrap();
        assert_eq!(table.rows.len(), 3);
        assert_eq!(table.columns.len(), 2);
        assert!(table.columns.iter().all(|c| !c.locked));
        let cells: Vec<_> = table.rows.iter().flat_map(|row| &row.cells).collect();
        let text: Vec<_> = cells
            .iter()
            .map(|cell| cell.paragraphs[0].text().unwrap().text.text())
            .collect();
        assert_eq!(text, ["Alpha", "Beta", "Gamma", "Delta", "Epsilon", ""]);
        assert!(cells.iter().all(|c| c.indents == [18.0, 0.0, 27.0, 27.0]));
        let outline = page
            .objects
            .iter()
            .find_map(|object| match object {
                PageObject::Outline(outline)
                    if outline
                        .paragraphs
                        .iter()
                        .any(|p| matches!(p.content, ParagraphContent::Table(_))) =>
                {
                    Some(outline)
                }
                _ => None,
            })
            .unwrap();
        let mut document = TextDocument::from_nodes(outline.paragraphs.clone()).unwrap();
        let original = document.clone();
        for (index, cell) in cells.iter().enumerate() {
            let position = TextPosition {
                paragraph: index,
                offset: 0,
            };
            let edit = document
                .replace(
                    position..position,
                    vec![
                        Paragraph::new("🧊".into(), Format::default()),
                        Paragraph::new("text".into(), Format::default()),
                    ],
                )
                .unwrap();
            assert_eq!(edit.container, Some(cell.id));
            let undo = document.apply(edit).unwrap();
            assert_eq!(document.paragraphs().nth(index).unwrap().text(), "🧊");
            document.apply(undo).unwrap();
            assert_eq!(document, original);
        }
    }
}
