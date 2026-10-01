//! The ops each stored change lowers to, recorded as the change applies so a page is never
//! rebuilt to be saved.

use super::*;
use onestore::op;
use onestore::{OutlineEdit, document::Layout};

type Lowered = Result<Vec<PageOp>, onestore::Error>;

fn refused(message: &'static str) -> onestore::Error {
    onestore::Error { offset: 0, message }
}

fn stale(_: EditError) -> onestore::Error {
    refused("The editor lost track of the stored page")
}

/// Whether `op` needs paragraph `blank`, last in `outline` and not stored yet: it edits the
/// paragraph or its text, or adds to or moves within the outline's top level.
fn reaches(op: &PageOp, outline: ExGuid, blank: &PageParagraph) -> bool {
    let text = blank.text().map(|text| text.id);
    let own = |id: &ExGuid| *id == blank.id || Some(*id) == text;
    match op {
        PageOp::Text { text, .. }
        | PageOp::Format { text, .. }
        | PageOp::Link { text, .. }
        | PageOp::Equation { text, .. }
        | PageOp::Split { text, .. } => own(text),
        PageOp::Join { left, right } => own(left) || own(right),
        PageOp::Insert {
            container, before, ..
        } => *container == outline || before.as_ref().is_some_and(own),
        PageOp::Move {
            object,
            parent,
            before,
        } => {
            own(object)
                || parent
                    .as_ref()
                    .is_some_and(|parent| *parent == outline || own(parent))
                || before.as_ref().is_some_and(own)
        }
        PageOp::Delete { object: id }
        | PageOp::Level { paragraph: id, .. }
        | PageOp::Outline { object: id, .. }
        | PageOp::Paragraph { paragraph: id, .. }
        | PageOp::Style { paragraph: id, .. }
        | PageOp::Unstyle { paragraph: id }
        | PageOp::Media { paragraph: id, .. }
        | PageOp::List { paragraph: id, .. }
        | PageOp::Tags { target: id, .. } => own(id),
        _ => false,
    }
}

/// Definitions a paragraph names: its lists, style and note tags.
pub(super) fn references(node: &PageParagraph) -> impl Iterator<Item = ExGuid> + '_ {
    let content_tags = match &node.content {
        ParagraphContent::Text(text) => text.tags.as_slice(),
        ParagraphContent::Table(table) => table.tags.as_slice(),
        ParagraphContent::Image(image) => image.tags.as_slice(),
        ParagraphContent::Attachment(file) => file.tags.as_slice(),
        _ => &[],
    };
    node.lists.iter().copied().chain(node.style).chain(
        node.tags
            .iter()
            .chain(content_tags)
            .filter_map(|tag| tag.definition),
    )
}

/// The definitions `paragraphs` and their cells name.
fn named(
    definitions: &BTreeMap<ExGuid, Definition>,
    paragraphs: &[&[PageParagraph]],
) -> BTreeMap<ExGuid, Definition> {
    paragraphs
        .iter()
        .flat_map(|list| descendants(list, None))
        .flat_map(|(_, _, node)| references(node))
        .filter_map(|id| Some((id, definitions.get(&id)?.clone())))
        .collect()
}

/// `op::lower` of paragraphs `before` of `container` replaced by `nodes[range]` and the
/// definitions they name, so it stays O(edit). When the structure changes, it also sees the
/// unchanged paragraphs an op may split, join, move or anchor to: the ones either side and
/// their ancestors, the range's descendants past it, and after them the next child of each
/// ancestor.
fn lower(
    container: ExGuid,
    nodes: &[PageParagraph],
    range: Range<usize>,
    before: &[PageParagraph],
    definitions: &BTreeMap<ExGuid, Definition>,
) -> Lowered {
    let after = &nodes[range.clone()];
    if before.is_empty() && after.is_empty() {
        return Ok(Vec::new());
    }
    let mut context = BTreeSet::new();
    let shape = |node: &PageParagraph| (node.id, node.parent, node.level);
    if before.iter().map(shape).ne(after.iter().map(shape)) {
        context.extend(range.start.checked_sub(1));
        context.extend((range.end < nodes.len()).then_some(range.end));
        let held: BTreeSet<ExGuid> = before.iter().chain(after).map(|node| node.id).collect();
        let mut wanted: Vec<ExGuid> = before
            .iter()
            .chain(after)
            .chain(context.iter().map(|&at| &nodes[at]))
            .filter_map(|node| node.parent)
            .collect();
        let mut open = BTreeMap::from([(None, 0)]);
        while let Some(id) = wanted.pop() {
            if held.contains(&id) || context.iter().any(|&at| nodes[at].id == id) {
                continue;
            }
            if let Some(at) = nodes[..range.start].iter().rposition(|node| node.id == id) {
                context.insert(at);
                open.insert(Some(id), nodes[at].level);
                wanted.extend(nodes[at].parent);
            }
        }
        // Descendants of the range move with the paragraphs it splits, joins and moves.
        let mut members = held;
        let mut end = range.end;
        while let Some(node) = nodes.get(end)
            && node.parent.is_some_and(|parent| members.contains(&parent))
        {
            members.insert(node.id);
            context.insert(end);
            end += 1;
        }
        for (at, node) in nodes.iter().enumerate().skip(end) {
            if open.remove(&node.parent).is_some() {
                context.insert(at);
            }
            // A container's children end where its subtree does.
            open.retain(|_, level| *level < node.level);
            if open.is_empty() {
                break;
            }
        }
    }
    let around = |middle: &[PageParagraph]| {
        let (head, tail): (Vec<usize>, Vec<usize>) =
            context.iter().partition(|&&at| at < range.start);
        head.iter()
            .map(|&at| &nodes[at])
            .chain(middle)
            .chain(tail.iter().map(|&at| &nodes[at]))
            .cloned()
            .collect::<Vec<_>>()
    };
    let (before, after) = (around(before), around(after));
    let named = named(definitions, &[&before, &after]);
    // What `op::lower` cannot move or change in place is removed and inserted again under
    // the identities it had: stored paragraphs a new table cell holds, and, where lowering
    // fails, a paragraph whose text object a join replaced outside what `Join` expresses
    // (between empty paragraphs, or back on undo) with the paragraph whose text it takes.
    let text = |node: &PageParagraph| node.text().map(|text| text.id);
    let stored: BTreeMap<ExGuid, Option<ExGuid>> = descendants(&before, None)
        .map(|(_, _, node)| (node.id, text(node)))
        .collect();
    let cells: BTreeSet<ExGuid> = descendants(&before, None)
        .filter_map(|(cell, _, _)| cell)
        .collect();
    let mut removed: BTreeSet<ExGuid> = descendants(&after, None)
        .filter(|(cell, _, node)| {
            stored.contains_key(&node.id) && cell.is_some_and(|cell| !cells.contains(&cell))
        })
        .map(|(_, _, node)| node.id)
        .collect();
    let lowered = |removed: &BTreeSet<ExGuid>| {
        let mut ops = Vec::new();
        let mut deleted = Vec::new();
        let mut kept = without(&before, removed, &mut ops, &mut deleted).map_err(stale)?;
        if kept.is_empty() {
            let anchor = anchor().map_err(stale)?;
            ops.push(PageOp::Insert {
                container,
                before: None,
                paragraphs: vec![anchor.clone()],
            });
            kept.push(anchor);
        }
        ops.extend(deleted.into_iter().map(|object| PageOp::Delete { object }));
        ops.extend(op::lower(container, &kept, &after, None, &named)?);
        Ok(ops)
    };
    let error = match lowered(&removed) {
        Ok(ops) => return Ok(ops),
        Err(error) => error,
    };
    let taken: BTreeSet<ExGuid> = descendants(&after, None)
        .filter(|(_, _, node)| stored.get(&node.id).is_some_and(|old| *old != text(node)))
        .filter_map(|(_, _, node)| {
            removed.insert(node.id);
            text(node)
        })
        .collect();
    if taken.is_empty() {
        return Err(error);
    }
    removed.extend(
        descendants(&before, None)
            .filter(|(_, _, node)| text(node).is_some_and(|id| taken.contains(&id)))
            .map(|(_, _, node)| node.id),
    );
    lowered(&removed)
}

/// An empty paragraph keeping a container from emptying while its paragraphs are removed
/// and inserted again; the lowering removes it after.
pub(super) fn anchor() -> Result<PageParagraph, EditError> {
    crate::document::node(
        Paragraph::new(String::new(), Format::default()),
        Format::default(),
    )
}

/// `nodes` without the subtrees of `removed`, whose roots `deleted` receives; a table cell
/// they would empty first receives an anchor, which `ops` inserts.
fn without(
    nodes: &[PageParagraph],
    removed: &BTreeSet<ExGuid>,
    ops: &mut Vec<PageOp>,
    deleted: &mut Vec<ExGuid>,
) -> Result<Vec<PageParagraph>, EditError> {
    let mut gone = BTreeSet::new();
    let mut kept = Vec::new();
    for node in nodes {
        if node.parent.is_some_and(|parent| gone.contains(&parent)) {
            gone.insert(node.id);
        } else if removed.contains(&node.id) {
            gone.insert(node.id);
            deleted.push(node.id);
        } else {
            let mut node = node.clone();
            if let ParagraphContent::Table(table) = &mut node.content {
                for cell in table.rows.iter_mut().flat_map(|row| &mut row.cells) {
                    cell.paragraphs = without(&cell.paragraphs, removed, ops, deleted)?;
                    if cell.paragraphs.is_empty() {
                        let anchor = anchor()?;
                        ops.push(PageOp::Insert {
                            container: cell.id,
                            before: None,
                            paragraphs: vec![anchor.clone()],
                        });
                        cell.paragraphs.push(anchor);
                    }
                }
            }
            kept.push(node);
        }
    }
    Ok(kept)
}

/// A page-level outline's position and width, as `op::lower_page` compares them.
pub(super) fn layout_ops(id: ExGuid, old: &Layout, new: &Layout) -> Lowered {
    let mut ops = Vec::new();
    if (new.x, new.y) != (old.x, old.y) {
        let (Some(x), Some(y)) = (new.x, new.y) else {
            return Err(refused("An outline position needs both coordinates"));
        };
        ops.push(PageOp::Outline {
            object: id,
            edit: OutlineEdit::Position { x, y },
        });
    }
    if (new.max_width, new.width_set_by_user) != (old.max_width, old.width_set_by_user) {
        ops.push(PageOp::Outline {
            object: id,
            edit: OutlineEdit::Width {
                points: new
                    .max_width
                    .ok_or_else(|| refused("An outline width cannot be removed"))?,
                user_set: new.width_set_by_user == Some(true),
            },
        });
    }
    Ok(ops)
}

/// The op giving `target` `tags`, with the definitions they name.
pub(super) fn tags(
    target: ExGuid,
    tags: Vec<onestore::document::Tag>,
    definitions: &BTreeMap<ExGuid, onestore::page::Definition>,
) -> Vec<PageOp> {
    let definitions = tags
        .iter()
        .filter_map(|tag| tag.definition)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter_map(|id| Some((id, definitions.get(&id)?.clone())))
        .collect();
    vec![PageOp::Tags {
        target,
        tags,
        definitions,
    }]
}

/// A page picture's stored position, size and description.
pub(super) fn picture_layout(image: &onestore::page::Image) -> Vec<PageOp> {
    vec![PageOp::Picture {
        picture: image.id,
        layout: image.layout.clone(),
        alt: image.alt.clone(),
    }]
}

impl CanvasEditor {
    /// The ops the stored page took since the last call, in order; an error means an edit
    /// since then cannot be stored, and the page should be reopened from storage.
    pub fn take_ops(&mut self) -> Result<Vec<PageOp>, onestore::Error> {
        let taken = std::mem::replace(&mut self.ops, Ok(Vec::new()));
        match (&taken, &mut self.stored) {
            (Ok(ops), Some((_, sent))) => sent.extend(ops.iter().cloned()),
            (Err(_), stored) => *stored = None,
            (Ok(_), None) => {}
        }
        taken
    }

    /// Queues `lowered`, storing first the provisional paragraph of an outline an op reaches.
    pub(super) fn record(&mut self, lowered: Lowered) {
        let Ok(ops) = &mut self.ops else {
            return;
        };
        let lowered = match lowered {
            Ok(lowered) => lowered,
            Err(error) => {
                self.ops = Err(error);
                return;
            }
        };
        for op in lowered {
            for (outline, (blank, stored)) in &mut self.provisional {
                if !*stored && reaches(&op, *outline, blank) {
                    *stored = true;
                    ops.push(PageOp::Insert {
                        container: *outline,
                        before: None,
                        paragraphs: vec![blank.clone()],
                    });
                }
            }
            ops.push(op);
        }
    }

    /// Ops for `change`, the inverse of an edit just applied to stored outline `outline`.
    pub(super) fn change_ops(&self, outline: &TextOutline, change: &TextChange) -> Lowered {
        let edit = &change.edit;
        let nodes = outline.document.container(edit.container).map_err(stale)?;
        let mut ops = lower(
            edit.container.unwrap_or(outline.id),
            nodes,
            edit.range.clone(),
            &edit.replacement,
            &self.definitions,
        )?;
        for (_, _, node) in descendants(outline.document.nodes(), None) {
            if let ParagraphContent::Table(table) = &node.content
                && edit.columns.contains_key(&table.id)
            {
                ops.push(PageOp::Table {
                    table: table.id,
                    edit: op::TableEdit::Columns(table.columns.clone()),
                });
            }
        }
        for placement in &change.positions {
            ops.extend(self.placement_ops(placement)?);
        }
        Ok(ops)
    }

    /// A page object's move from `placement`, as title flow makes it.
    pub(super) fn placement_ops(&self, placement: &Placement) -> Lowered {
        let old = Layout {
            x: placement.position[0],
            y: placement.position[1],
            ..Default::default()
        };
        let moved = |layout: &Layout| Layout {
            x: layout.x,
            y: layout.y,
            ..Default::default()
        };
        let picture = |image: &onestore::page::Image| {
            Ok(if [image.layout.x, image.layout.y] == placement.position {
                Vec::new()
            } else {
                picture_layout(image)
            })
        };
        if let Some(outline) = self.outlines.iter().find(|item| item.id == placement.id) {
            return layout_ops(outline.id, &old, &moved(&outline.layout));
        }
        for object in &self.objects {
            if let Some(image) = object.picture().filter(|image| image.id == placement.id) {
                return picture(image);
            }
            match (object, object.layout()) {
                (page::Content::ReadOnly(object), Some((id, _)))
                    if id == placement.id && !matches!(object.source, PageObject::Outline(_)) =>
                {
                    return Err(refused("Unsupported objects cannot be moved"));
                }
                (_, Some((id, layout))) if id == placement.id => {
                    return layout_ops(id, &old, &moved(layout));
                }
                _ => {}
            }
        }
        Err(refused("The editor lost track of the stored page"))
    }

    /// Ops turning stored outline `old` into what `outlines` holds under `id` now: added,
    /// removed, or its changed paragraphs and geometry.
    pub(super) fn outline_ops(&self, id: ExGuid, old: Option<&TextOutline>) -> Lowered {
        let new = self.outlines.iter().find(|outline| outline.id == id);
        match (old, new) {
            (None, None) => Ok(Vec::new()),
            (Some(_), None) => Ok(vec![PageOp::Delete { object: id }]),
            (None, Some(new)) => self.added(new.snapshot()),
            (Some(old), Some(new)) => {
                let (a, b) = (old.document.nodes(), new.document.nodes());
                let prefix = a.iter().zip(b).take_while(|(x, y)| x == y).count();
                let suffix = a[prefix..]
                    .iter()
                    .rev()
                    .zip(b[prefix..].iter().rev())
                    .take_while(|(x, y)| x == y)
                    .count();
                let mut ops = lower(
                    id,
                    b,
                    prefix..b.len() - suffix,
                    &a[prefix..a.len() - suffix],
                    &self.definitions,
                )?;
                ops.extend(layout_ops(id, &old.layout, &new.layout)?);
                Ok(ops)
            }
        }
    }

    /// Ops adding `outline`, new to the stored page, in its place in paint order.
    pub(super) fn added(&self, outline: Outline) -> Lowered {
        let page = |objects| Page {
            title: String::new(),
            identity: None,
            created: None,
            margin_origin: [0.0; 2],
            rtl: false,
            color: None,
            rule_lines: None,
            objects,
            definitions: named(&self.definitions, &[&outline.paragraphs]),
        };
        let mut ops = op::lower_page(
            &page(Vec::new()),
            &page(vec![PageObject::Outline(outline.clone())]),
        )?;
        let successor = self.successor(outline.id);
        for op in &mut ops {
            if let PageOp::Add { before, .. } = op {
                *before = successor;
            }
        }
        Ok(ops)
    }

    /// The page's date as the editor shows it.
    pub(super) fn date_ops(&self) -> Vec<PageOp> {
        let Some(date) = &self.date else {
            return Vec::new();
        };
        vec![PageOp::Date {
            created: date.timestamp(),
            fields: date
                .fields()
                .filter_map(|(_, paragraph)| {
                    let text = paragraph.text()?;
                    Some((text.id, text.text.text().to_owned()))
                })
                .collect(),
        }]
    }

    /// The page child `page()` places after `id`, titles aside.
    pub(super) fn successor(&self, id: ExGuid) -> Option<ExGuid> {
        let mut children: Vec<ExGuid> = Vec::new();
        for content in &self.objects {
            let child = match content {
                page::Content::Editable(outline)
                    if self.outlines.iter().any(|o| o.id == *outline) =>
                {
                    *outline
                }
                page::Content::Editable(_) => continue,
                page::Content::Outline { source, .. } => source.id,
                page::Content::Date { .. } => continue,
                page::Content::Image(image) => image.id,
                page::Content::File { source, .. } => source.id,
                page::Content::Ink(ink) => ink.id,
                page::Content::ReadOnly(object)
                    if matches!(object.source, PageObject::Title(_)) =>
                {
                    continue;
                }
                page::Content::ReadOnly(object) => object.source.id(),
            };
            // Titles, the date's among them, are listed apart from what an op places.
            if !self
                .header
                .areas
                .iter()
                .any(|area| area.origins.contains_key(&child))
            {
                children.push(child);
            }
        }
        children.extend(
            self.outlines
                .iter()
                .filter(|outline| !self.has_page_outline(outline.id))
                .map(|outline| outline.id),
        );
        let at = children.iter().position(|child| *child == id)?;
        children.get(at + 1).copied()
    }
}

/// `image` with the ops `editor` recorded since they were last taken applied to the page in
/// `space` and sealed, as the application saves an edit.
#[cfg(test)]
pub(crate) fn saved(image: &[u8], space: ExGuid, editor: &mut CanvasEditor) -> Vec<u8> {
    let arena = onestore::Arena::default();
    let mut section = onestore::Section::open(&arena, image.to_vec()).unwrap();
    let ops = editor.take_ops().unwrap();
    let ops = ops
        .into_iter()
        .map(|op| op::Op::Page { space, op })
        .collect();
    let edit = op::Edit {
        at: 134_000_000_000_000_000,
        ops,
    };
    section.apply("Author", &edit).unwrap();
    section.seal().unwrap();
    section.image()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::TextEngine;
    use std::time::{Duration, Instant};

    fn median(mut samples: Vec<Duration>) -> Duration {
        samples.sort();
        samples[samples.len() / 2]
    }

    /// The rows of the data-layer plan's table: each edit and its undo as ops, the undo
    /// of a join splitting the paragraph back under its own identities.
    #[test]
    fn edits_and_their_undo_record_the_plan_s_ops() {
        let mut engine = TextEngine::default();
        let lines = ["Hello", "World"].map(|line| Paragraph::new(line.into(), Format::default()));
        let document = crate::document::TextDocument::new(lines.to_vec()).unwrap();
        let mut editor = CanvasEditor::new(&mut engine, document, 400.0).unwrap();
        let nodes = editor.outlines()[0].document().nodes().to_vec();
        let text = |index: usize| nodes[index].text().unwrap().id;
        let at = |paragraph, offset| [TextPosition { paragraph, offset }; 2].into();
        let undone = |editor: &mut CanvasEditor, engine: &mut TextEngine| {
            editor.undo(engine).unwrap();
            editor.take_ops().unwrap()
        };

        editor.select(at(0, 3)).unwrap();
        editor.insert(&mut engine, "ab").unwrap();
        let typed = PageOp::Text {
            text: text(0),
            range: 3..3,
            with: "ab".into(),
        };
        assert_eq!(editor.take_ops().unwrap(), [typed]);
        let erased = PageOp::Text {
            text: text(0),
            range: 3..5,
            with: String::new(),
        };
        assert_eq!(undone(&mut editor, &mut engine), [erased]);

        editor.select(at(1, 0)).unwrap();
        editor.delete(&mut engine, true).unwrap();
        let joined = PageOp::Join {
            left: text(0),
            right: text(1),
        };
        assert_eq!(editor.take_ops().unwrap(), [joined]);
        let split = PageOp::Split {
            text: text(0),
            at: 5,
            paragraph: nodes[1].id,
            right: text(1),
            lists: Vec::new(),
        };
        assert_eq!(undone(&mut editor, &mut engine), [split]);

        editor.select(at(1, 0)).unwrap();
        editor.tab(&mut engine, false).unwrap();
        let [PageOp::Move { object, parent, .. }] = &editor.take_ops().unwrap()[..] else {
            panic!("Tab moves the paragraph under its sibling")
        };
        assert_eq!((*object, *parent), (nodes[1].id, Some(nodes[0].id)));
        // A moved paragraph keeps its level where it lies deeper than its new parent.
        let ops = undone(&mut editor, &mut engine);
        let [
            PageOp::Move { object, parent, .. },
            PageOp::Level {
                paragraph,
                level: 1,
            },
        ] = &ops[..]
        else {
            panic!("undoing Tab moves it back and outdents it: {ops:?}")
        };
        assert_eq!(
            (*object, *parent, *paragraph),
            (nodes[1].id, Some(editor.outlines()[0].id), nodes[1].id)
        );

        editor
            .select(
                [
                    TextPosition {
                        paragraph: 0,
                        offset: 1,
                    },
                    TextPosition {
                        paragraph: 0,
                        offset: 4,
                    },
                ]
                .into(),
            )
            .unwrap();
        editor
            .format(&mut engine, Formatting::Toggle(format::Toggle::Bold))
            .unwrap();
        let bold = PageOp::Format {
            text: text(0),
            range: 1..4,
            set: vec![onestore::TextAttribute::Bold(true)],
            clear: Vec::new(),
        };
        assert_eq!(editor.take_ops().unwrap(), [bold]);
        let [
            PageOp::Format {
                text: target,
                range,
                ..
            },
        ] = &undone(&mut editor, &mut engine)[..]
        else {
            panic!("undoing bold formats the range back")
        };
        assert_eq!((*target, range.clone()), (text(0), 1..4));
    }

    /// Alt+Shift+Up and Down swap a paragraph, children and all, with its sibling as moves,
    /// keeping the caret on the moved text, and stop at the first and last sibling.
    #[test]
    fn moving_paragraphs_swaps_sibling_subtrees_as_move_ops() {
        let mut engine = TextEngine::default();
        let lines = ["A", "a", "B"].map(|line| Paragraph::new(line.into(), Format::default()));
        let document = crate::document::TextDocument::new(lines.to_vec()).unwrap();
        let mut editor = CanvasEditor::new(&mut engine, document, 400.0).unwrap();
        editor
            .select(
                [TextPosition {
                    paragraph: 1,
                    offset: 0,
                }; 2]
                    .into(),
            )
            .unwrap();
        editor.indent(&mut engine, false).unwrap();
        editor.take_ops().unwrap();
        let texts = |editor: &CanvasEditor| {
            editor.outlines()[0]
                .document()
                .nodes()
                .iter()
                .map(|node| (node.text().unwrap().text.text().to_owned(), node.level))
                .collect::<Vec<_>>()
        };
        let caret = TextPosition {
            paragraph: 2,
            offset: 1,
        };
        editor.select([caret; 2].into()).unwrap();
        assert!(!editor.move_paragraphs(&mut engine, false).unwrap());
        assert!(editor.move_paragraphs(&mut engine, true).unwrap());
        let moved = [("B".into(), 1), ("A".into(), 1), ("a".into(), 2)];
        assert_eq!(texts(&editor), moved);
        assert_eq!(
            editor.selection().positions,
            [TextPosition {
                paragraph: 0,
                offset: 1
            }; 2]
        );
        let ops = editor.take_ops().unwrap();
        assert!(
            !ops.is_empty() && ops.iter().all(|op| matches!(op, PageOp::Move { .. })),
            "{ops:?}"
        );
        assert!(!editor.move_paragraphs(&mut engine, true).unwrap());
        assert!(editor.move_paragraphs(&mut engine, false).unwrap());
        assert_eq!(texts(&editor)[2], ("B".into(), 1));
        assert_eq!(editor.selection().positions[0].paragraph, 2);
        editor.undo(&mut engine).unwrap();
        assert_eq!(texts(&editor), moved);
    }

    /// A change from elsewhere reaches only what it changed: a refresh whose stored page is
    /// what the editor's ops left (though storage normalized it) changes nothing, history
    /// included; one that changes an outline drops that outline's history and keeps the
    /// rest, which still undoes into the section.
    #[test]
    fn a_refresh_keeps_what_the_change_did_not_reach() {
        use onestore::op::{Edit, Op};
        let image =
            include_bytes!("../../../../corpus/paragraph-edit/before/notebook/synthetic.one");
        let arena = onestore::Arena::default();
        let mut section = onestore::Section::open(&arena, image.to_vec()).unwrap();
        let space = section.pages().unwrap()[0].0;
        let mut engine = TextEngine::default();
        let mut editor =
            CanvasEditor::from_page(section.page(space).unwrap(), &mut engine).unwrap();
        let mut at = 133_000_000_000_000_000;
        let mut store = |section: &mut onestore::Section<'_>, ops: Vec<PageOp>| {
            at += 10_000_000;
            let ops = ops.into_iter().map(|op| Op::Page { space, op }).collect();
            section.apply("Test", &Edit { at, ops }).unwrap();
        };
        let local = editor.outlines().iter().find(|o| !o.title).unwrap().id;
        editor.focus_outline(local).unwrap();
        editor
            .select(
                [TextPosition {
                    paragraph: 0,
                    offset: 0,
                }; 2]
                    .into(),
            )
            .unwrap();
        editor.insert(&mut engine, "Local ").unwrap();
        let created = editor
            .create_outline(&mut engine, [400.0, 600.0], 300.0)
            .unwrap();
        editor.insert(&mut engine, "Mine").unwrap();
        store(&mut section, editor.take_ops().unwrap());

        let (shown, history) = (editor.page().unwrap(), editor.undo.len());
        let stored = section.page(space).unwrap();
        assert_ne!(
            Page {
                title: shown.title.clone(),
                ..stored.clone()
            },
            shown,
            "storage normalizes"
        );
        assert!(!editor.refresh(stored, &mut engine).unwrap());
        assert_eq!(
            (
                editor.page().unwrap(),
                editor.undo.len(),
                editor.active_outline().id
            ),
            (shown, history, created)
        );

        let text = section
            .page(space)
            .unwrap()
            .objects
            .iter()
            .find_map(|object| match object {
                onestore::page::PageObject::Outline(outline) if outline.id == local => outline
                    .paragraphs
                    .last()?
                    .text()
                    .map(|text| (text.id, text.text.text().encode_utf16().count() as u32)),
                _ => None,
            });
        let (text, length) = text.unwrap();
        store(
            &mut section,
            vec![PageOp::Text {
                text,
                range: length..length,
                with: " remote".into(),
            }],
        );
        assert!(
            editor
                .refresh(section.page(space).unwrap(), &mut engine)
                .unwrap()
        );
        assert_eq!(
            editor.undo.len(),
            history - 1,
            "the typing in the changed outline goes"
        );
        assert_eq!(editor.active_outline().id, created);
        assert!(editor.undo(&mut engine).unwrap());
        assert!(editor.undo(&mut engine).unwrap());
        store(&mut section, editor.take_ops().unwrap());
        assert!(!editor.undo(&mut engine).unwrap());
        let page = section.page(space).unwrap();
        assert!(!page.objects.iter().any(|object| object.id() == created));
        let texts: Vec<String> = editor
            .outlines()
            .iter()
            .flat_map(|o| o.document().paragraphs().map(|p| p.text().to_owned()))
            .collect();
        assert!(texts.iter().any(|text| text.starts_with("Local ")));
        assert!(texts.iter().any(|text| text.ends_with(" remote")));
    }

    /// One keystroke on the 3000-paragraph probe page, on the frame thread:
    /// `SECTION_PROBE=path cargo test --release -p canvas --lib keystroke_recording -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn keystroke_recording() {
        let path = std::env::var("SECTION_PROBE").unwrap_or("/tmp/probe3000.one".into());
        let image = std::fs::read(&path).unwrap();
        let arena = onestore::Arena::default();
        let mut section = onestore::Section::open(&arena, image).unwrap();
        let page = section
            .pages()
            .unwrap()
            .into_iter()
            .map(|(space, ..)| section.page(space).unwrap())
            .max_by_key(|page| format!("{page:?}").len())
            .unwrap();
        let mut engine = TextEngine::default();
        let mut editor = CanvasEditor::from_page(page, &mut engine).unwrap();
        let outline = editor
            .outlines()
            .iter()
            .max_by_key(|outline| outline.document().nodes().len())
            .unwrap();
        let (id, count) = (outline.id, outline.document().nodes().len());
        editor.focus_outline(id).unwrap();
        let caret = TextPosition {
            paragraph: count / 2,
            offset: 0,
        };
        editor.select([caret; 2].into()).unwrap();
        let (mut typing, mut recording, mut rebuilding) = (Vec::new(), Vec::new(), Vec::new());
        let mut previous = editor.page().unwrap();
        for _ in 0..200 {
            let start = Instant::now();
            editor.insert(&mut engine, "x").unwrap();
            typing.push(start.elapsed());
            let Some(History::Text { change, .. }) = editor.undo.last() else {
                unreachable!()
            };
            let start = Instant::now();
            let ops = editor.change_ops(editor.active_outline(), change).unwrap();
            recording.push(start.elapsed());
            assert_eq!(ops.len(), 1, "{ops:?}");
            assert_eq!(editor.take_ops().unwrap().len(), 1);
            let start = Instant::now();
            let page = editor.page().unwrap();
            assert!(page != previous);
            rebuilding.push(start.elapsed());
            previous = page;
        }
        println!(
            "{path}: {count} paragraphs; keystroke {:?}, of which recording its op {:?}; \
             the page rebuilt and compared as persist did {:?}",
            median(typing),
            median(recording),
            median(rebuilding)
        );
    }
}
