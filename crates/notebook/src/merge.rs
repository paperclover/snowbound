//! Rebasing queued edits onto a changed remote section. Each op replays on the remote
//! section when the objects it reads are as the base image had them; text ops on a text
//! the remote also changed shift past the remote's changes when they stay clear of them;
//! a move, deletion or outline setting the remote already made is dropped as done.
//! Anything else is a conflict: the queue stays on its base until it is resolved.
//! Objects the queue itself created are not in the base, so nothing remote can have
//! changed them.

use crate::{ConflictKind, PendingEdit, Result};
use onestore::{
    ExGuid, OutlineEdit, PageCreation, PageEdit, PagePosition, Section,
    document::{Format, Layout, Tag},
    op::{Edit, Op, OpError, PageOp, SectionOp, TableEdit},
    page::{
        Attachment, Image, Ink, MediaIndex, Outline, Page, PageObject, PageParagraph, Paragraph,
        ParagraphContent, TableColumn, TextObject, Unsupported,
    },
};
use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Range,
};

/// A conflict being resolved: from edit `first` on, ops on `space` are dropped. Keeping
/// mine replaces the first of them by the ops rewriting the remote page to `target` (or
/// putting `target` back as a page where the remote removed it), or, for the section's
/// page list, keeps what of each section op still applies.
pub(crate) struct Resolving<'p> {
    pub first: u64,
    pub space: ExGuid,
    pub mine: bool,
    pub target: Option<&'p Page>,
}

pub(crate) enum Outcome {
    /// Every edit applied; these edits' ops changed. An edit whose ops were all dropped
    /// stays, empty, to be published and acknowledged with its batch.
    Applied(Vec<(u64, Edit)>),
    Conflict {
        id: u64,
        space: ExGuid,
        kind: ConflictKind,
    },
}

/// Replays `edits` on `new`, the remote section; `old` is the base they were made on.
pub(crate) fn rebase(
    old: &mut Section<'_>,
    new: &mut Section<'_>,
    edits: &[PendingEdit],
    converged: &BTreeSet<ExGuid>,
    resolving: Option<Resolving<'_>>,
) -> Result<Outcome> {
    let root = new.root();
    let before: BTreeMap<ExGuid, ExGuid> = old.revisions().collect();
    let after: BTreeMap<ExGuid, ExGuid> = new.revisions().collect();
    let orders = (order(old)?, order(new)?);
    let mut state = Replay {
        before,
        after,
        orders,
        created: BTreeSet::new(),
        diffs: BTreeMap::new(),
    };
    let mut rewritten = Vec::new();
    let mut resolved = false;
    for queued in edits {
        let branch = resolving
            .as_ref()
            .filter(|resolving| queued.id >= resolving.first);
        let mut ops = Vec::new();
        let mut changed = false;
        for op in &queued.edit.ops {
            let conflict = |kind, space| Outcome::Conflict {
                id: queued.id,
                space,
                kind,
            };
            if let Some(resolving) = branch {
                let dropped = match op {
                    Op::Page { space, .. } => *space == resolving.space,
                    Op::Section(_) => resolving.space == root,
                };
                if dropped {
                    changed = true;
                    let rewrite = match op {
                        Op::Page { space, .. } if !resolved && resolving.mine => {
                            resolving.target.map(|target| (*space, target))
                        }
                        _ => None,
                    };
                    resolved |= matches!(op, Op::Page { .. });
                    if let Some((space, target)) = rewrite {
                        if !state.listed(space) {
                            // Kept while the remote removed it: the page comes back.
                            match state.restore(
                                new,
                                space,
                                target,
                                &queued.author,
                                queued.edit.at,
                            )? {
                                Ok(restored) => ops.extend(restored),
                                Err(kind) => return Ok(conflict(kind, space)),
                            }
                            continue;
                        }
                        let current = new.page(space)?;
                        for op in onestore::op::lower_page(&current, target)
                            .map_err(|error| OpError::Unsupported(error.message))?
                        {
                            let op = Op::Page { space, op };
                            if let Some(kind) =
                                state.apply(new, &queued.author, queued.edit.at, &op)?
                            {
                                return Ok(conflict(kind, space));
                            }
                            ops.push(op);
                        }
                    }
                    if let (Op::Section(op), true) = (op, resolving.mine) {
                        // Keeping mine for the section list: the first form that applies.
                        for op in state.salvage(new, op) {
                            let op = Op::Section(op);
                            if state
                                .apply(new, &queued.author, queued.edit.at, &op)?
                                .is_none()
                            {
                                ops.push(op);
                                break;
                            }
                        }
                    }
                    continue;
                }
            }
            let space = match op {
                Op::Page { space, .. } => *space,
                Op::Section(_) => root,
            };
            if matches!(op, Op::Page { .. }) && converged.contains(&space) {
                changed = true;
                continue;
            }
            let mapped = match state.check(old, new, op)? {
                Ok(Some(mapped)) => mapped,
                Ok(None) => {
                    changed = true;
                    continue;
                }
                Err(kind) => return Ok(conflict(kind, space)),
            };
            changed |= mapped != *op;
            if let Some(kind) = state.apply(new, &queued.author, queued.edit.at, &mapped)? {
                return Ok(conflict(kind, space));
            }
            ops.push(mapped);
        }
        if changed {
            rewritten.push((
                queued.id,
                Edit {
                    at: queued.edit.at,
                    ops,
                },
            ));
        }
    }
    Ok(Outcome::Applied(rewritten))
}

fn order(section: &mut Section<'_>) -> Result<Order> {
    Ok(section
        .pages()?
        .into_iter()
        .map(|(space, _, level)| (space, level))
        .collect())
}

/// Page spaces in section order with their levels.
type Order = Vec<(ExGuid, u32)>;

struct Replay {
    /// Active revisions of the base and the remote.
    before: BTreeMap<ExGuid, ExGuid>,
    after: BTreeMap<ExGuid, ExGuid>,
    /// Page order of the base and the remote.
    orders: (Order, Order),
    /// Page spaces the replayed edits created.
    created: BTreeSet<ExGuid>,
    /// Per page the remote changed, what it changed; `None` when it left the page alone.
    diffs: BTreeMap<ExGuid, Option<Diff>>,
}

impl Replay {
    /// Applies one op to the remote section; `Some` names why it did not apply.
    fn apply(
        &mut self,
        new: &mut Section<'_>,
        author: &str,
        at: u64,
        op: &Op,
    ) -> Result<Option<ConflictKind>> {
        match new.apply(
            author,
            &Edit {
                at,
                ops: vec![op.clone()],
            },
        ) {
            Ok(()) => {
                if let Op::Section(
                    SectionOp::Create(creation) | SectionOp::Import { creation, .. },
                ) = op
                {
                    self.created.insert(creation.space());
                }
                Ok(None)
            }
            Err(OpError::Failed(error)) => Err(error.into()),
            Err(error) => Ok(Some(match op {
                // A page list the remote reordered refuses the op, however it says so.
                Op::Section(_) => ConflictKind::StructureChanged,
                Op::Page { .. } if matches!(error, OpError::Unsupported(_)) => {
                    ConflictKind::UnsupportedEdit
                }
                Op::Page { space, .. } if !self.listed(*space) => ConflictKind::TargetUnavailable,
                Op::Page { .. } => ConflictKind::ContentChanged,
            })),
        }
    }

    /// The op as it replays on the remote section, or why it cannot.
    fn check(
        &mut self,
        old: &mut Section<'_>,
        new: &mut Section<'_>,
        op: &Op,
    ) -> Result<std::result::Result<Option<Op>, ConflictKind>> {
        match op {
            Op::Page { space, op } => {
                if !self.listed(*space) {
                    return Ok(Err(ConflictKind::TargetUnavailable));
                }
                let diff = match self.diffs.get_mut(space) {
                    Some(diff) => diff,
                    None => {
                        let diff = match (self.before.get(space), self.after.get(space)) {
                            (Some(before), Some(after)) if before != after => Some(Diff {
                                old: Index::of(&old.page(*space)?),
                                new: Index::of(&new.page(*space)?),
                                regions: BTreeMap::new(),
                            }),
                            _ => None,
                        };
                        self.diffs.entry(*space).or_insert(diff)
                    }
                };
                Ok(match diff {
                    None => Ok(Some(Op::Page {
                        space: *space,
                        op: op.clone(),
                    })),
                    Some(diff) => diff
                        .check(op)
                        .map(|op| op.map(|op| Op::Page { space: *space, op })),
                })
            }
            Op::Section(section) => Ok(self.section(section).map(|()| Some(op.clone()))),
        }
    }

    /// Whether the remote section lists the page, or the replayed edits created it; a page
    /// removed from the list keeps its stored revisions.
    fn listed(&self, space: ExGuid) -> bool {
        self.created.contains(&space) || self.orders.1.iter().any(|(page, _)| *page == space)
    }

    /// Puts `page`, which the remote removed, back into the section and returns the ops
    /// that did: a copy under fresh identities at its level, before the first page after it
    /// that the remote still has and that can take a page before it, or last.
    fn restore(
        &mut self,
        new: &mut Section<'_>,
        space: ExGuid,
        page: &Page,
        author: &str,
        at: u64,
    ) -> Result<std::result::Result<Vec<Op>, ConflictKind>> {
        let old = self.orders.0.clone();
        let position = old.iter().position(|(listed, _)| *listed == space);
        let anchors = position
            .map_or(&[][..], |position| &old[position + 1..])
            .iter()
            .map(|(next, _)| Some(*next))
            .filter(|next| next.is_some_and(|next| self.listed(next)))
            .chain([None])
            .collect::<Vec<_>>();
        let copy = page.copy()?;
        let mut refusal = ConflictKind::StructureChanged;
        for before in anchors {
            let creation = PageCreation::new(before, Some(&page.title), author)?;
            let import = Op::Section(SectionOp::Import {
                creation: creation.clone(),
                page: copy.clone(),
            });
            match self.apply(new, author, at, &import)? {
                Some(kind) => refusal = kind,
                None => {
                    let mut ops = vec![import];
                    if let Some(level) = position.map(|at| old[at].1).filter(|level| *level > 1) {
                        let indent = Op::Section(SectionOp::Pages(vec![PageEdit::set_level(
                            creation.space(),
                            level,
                        )?]));
                        if let Some(kind) = self.apply(new, author, at, &indent)? {
                            return Ok(Err(kind));
                        }
                        ops.push(indent);
                    }
                    return Ok(Ok(ops));
                }
            }
        }
        Ok(Err(refusal))
    }

    fn section(&self, op: &SectionOp) -> std::result::Result<(), ConflictKind> {
        let (old, new) = &self.orders;
        let find =
            |list: &[(ExGuid, u32)], space: ExGuid| list.iter().position(|(s, _)| *s == space);
        match op {
            SectionOp::Create(_) | SectionOp::Import { .. } => Ok(()),
            SectionOp::Delete(pages) => {
                for page in pages {
                    if find(new, *page).is_none() && !self.created.contains(page) {
                        return Err(ConflictKind::TargetUnavailable);
                    }
                    if self.before.get(page) != self.after.get(page) {
                        return Err(ConflictKind::ContentChanged);
                    }
                }
                Ok(())
            }
            SectionOp::Pages(edits) => {
                // Where the batch puts each page, from the base order.
                let mut wanted: Vec<ExGuid> = old.iter().map(|(space, _)| *space).collect();
                for edit in edits {
                    if let PagePosition::Before(before) = edit.position()
                        && let Some(at) = wanted.iter().position(|space| *space == edit.space())
                    {
                        let space = wanted.remove(at);
                        let at = before
                            .and_then(|before| wanted.iter().position(|space| *space == before))
                            .unwrap_or(wanted.len());
                        wanted.insert(at, space);
                    }
                }
                let wanted: Vec<(ExGuid, u32)> =
                    wanted.into_iter().map(|space| (space, 0)).collect();
                for edit in edits {
                    let space = edit.space();
                    let Some(at) = find(old, space) else {
                        continue;
                    };
                    let Some(now) = find(new, space) else {
                        return Err(ConflictKind::TargetUnavailable);
                    };
                    if old[at].1 != new[now].1 && new[now].1 != edit.level() {
                        return Err(ConflictKind::StructureChanged);
                    }
                    if edit.position() != PagePosition::Keep {
                        // Each page the remote also has must keep its side of the moved page,
                        // or already be on the side the batch wants.
                        let side = |list: &[(ExGuid, u32)], peer: ExGuid| {
                            Some(find(list, peer)? < find(list, space)?)
                        };
                        let conflicting = old.iter().any(|(peer, _)| {
                            *peer != space
                                && find(new, *peer).is_some()
                                && side(new, *peer) != side(old, *peer)
                                && side(new, *peer) != side(&wanted, *peer)
                        });
                        if conflicting {
                            return Err(ConflictKind::StructureChanged);
                        }
                    }
                }
                Ok(())
            }
        }
    }

    /// Forms of a section op to try on the remote section list, in order: as it is, a
    /// creation appended instead, moves and removals of the pages still there.
    fn salvage(&self, new: &mut Section<'_>, op: &SectionOp) -> Vec<SectionOp> {
        let pages: BTreeSet<ExGuid> = new
            .pages()
            .map(|pages| pages.into_iter().map(|(space, ..)| space).collect())
            .unwrap_or_default();
        let present = |space: &ExGuid| pages.contains(space) || self.created.contains(space);
        match op {
            SectionOp::Create(creation) => std::iter::once(creation.clone())
                .chain(creation.reposition(None).ok())
                .map(SectionOp::Create)
                .collect(),
            SectionOp::Import { .. } => vec![op.clone()],
            SectionOp::Pages(edits) => {
                let edits: Vec<PageEdit> = edits
                    .iter()
                    .filter(|edit| present(&edit.space()))
                    .cloned()
                    .collect();
                if edits.is_empty() {
                    Vec::new()
                } else {
                    vec![SectionOp::Pages(edits)]
                }
            }
            SectionOp::Delete(deleted) => {
                let deleted: Vec<ExGuid> = deleted
                    .iter()
                    .filter(|space| present(space))
                    .copied()
                    .collect();
                if deleted.is_empty() {
                    Vec::new()
                } else {
                    vec![SectionOp::Delete(deleted)]
                }
            }
        }
    }
}

/// One page as the base stored it and as the remote does, with the remote's text changes
/// in the coordinates of the replayed edits.
struct Diff {
    old: Index,
    new: Index,
    regions: BTreeMap<ExGuid, Changes>,
}

impl Diff {
    fn ours(&self, id: ExGuid) -> bool {
        !self.old.nodes.contains_key(&id)
    }

    /// Whether the remote left the object's own data as the base had it.
    fn kept(&self, id: ExGuid) -> bool {
        self.ours(id)
            || matches!((self.old.nodes.get(&id), self.new.nodes.get(&id)), (Some(a), Some(b)) if a.own == b.own)
    }

    /// Whether the remote left the object and everything under it as the base had it.
    fn untouched(&self, id: ExGuid) -> bool {
        if self.ours(id) {
            return true;
        }
        let (Some(a), Some(b)) = (self.old.nodes.get(&id), self.new.nodes.get(&id)) else {
            return false;
        };
        a.own == b.own
            && a.parent == b.parent
            && a.children == b.children
            && a.below().all(|child| self.untouched(child))
    }

    /// Whether the remote kept the object under the same parent, in the same order among
    /// the siblings both sides have.
    fn placed(&self, id: ExGuid) -> bool {
        if self.ours(id) {
            return true;
        }
        let (Some(a), Some(b)) = (self.old.nodes.get(&id), self.new.nodes.get(&id)) else {
            return false;
        };
        if a.parent != b.parent {
            return false;
        }
        let siblings = |index: &Index| index.children(a.parent).to_vec();
        let (old, new) = (siblings(&self.old), siblings(&self.new));
        let at = |list: &[ExGuid], id: ExGuid| list.iter().position(|x| *x == id);
        let (Some(i), Some(j)) = (at(&old, id), at(&new, id)) else {
            return false;
        };
        old.iter()
            .enumerate()
            .all(|(k, peer)| *peer == id || at(&new, *peer).is_none_or(|l| (k < i) == (l < j)))
    }

    /// Where the remote changed a text's characters and formats; its tags and date field
    /// are not positions an op names.
    fn region(&mut self, text: ExGuid) -> std::result::Result<Option<&mut Changes>, ConflictKind> {
        if !self.regions.contains_key(&text) {
            if self.ours(text) {
                return Ok(None);
            }
            let (Some(Own::Text(a)), Some(Own::Text(b))) = (
                self.old.nodes.get(&text).map(|node| &node.own),
                self.new.nodes.get(&text).map(|node| &node.own),
            ) else {
                return Err(ConflictKind::ContentChanged);
            };
            if a.text == b.text {
                return Ok(None);
            }
            let region = Changes::between(&a.text, &b.text);
            self.regions.insert(text, region);
        }
        Ok(self.regions.get_mut(&text))
    }

    /// The op as it replays on the remote page; `None` when the remote already made it.
    fn check(&mut self, op: &PageOp) -> std::result::Result<Option<PageOp>, ConflictKind> {
        use ConflictKind::ContentChanged as Changed;
        let require = |ok: bool| if ok { Ok(()) } else { Err(Changed) };
        let mut op = op.clone();
        match &mut op {
            PageOp::Text { text, range, with } => {
                if let Some(region) = self.region(*text)? {
                    let mapped = region.map(range).ok_or(Changed)?;
                    region.replaced(range, with.encode_utf16().count() as u32);
                    *range = mapped;
                }
            }
            PageOp::Format { text, range, .. } => {
                if let Some(region) = self.region(*text)? {
                    *range = region.map(range).ok_or(Changed)?;
                }
            }
            PageOp::Link { text, .. } | PageOp::Equation { text, .. } => {
                require(self.region(*text)?.is_none())?;
            }
            PageOp::Date { fields, .. } => {
                for (text, _) in fields.iter() {
                    require(self.region(*text)?.is_none())?;
                }
            }
            PageOp::Split {
                text, at, right, ..
            } => {
                // The new paragraph copies the holder's placement and formats; lists and tags
                // stay with the holder unless the split names copies of them.
                let holder = self.holder(*text);
                require(holder.is_none_or(|holder| self.splits_alike(holder)))?;
                if let Some(region) = self.region(*text)? {
                    // The cut moves with the character after it: text the remote typed at
                    // the cut stays left of it.
                    let cut = region.map(&(*at..*at + 1)).ok_or(Changed)?;
                    let right_changes = region.split(*at);
                    if !right_changes.0.is_empty() {
                        self.regions.insert(*right, right_changes);
                    }
                    *at = cut.start;
                }
            }
            PageOp::Join { left, right } => {
                require(self.untouched(*right))?;
                require(
                    self.holder_text(*right)
                        .is_none_or(|text| !self.regions.contains_key(&text)),
                )?;
                if let Some(text) = self.holder_text(*left) {
                    self.region(text)?;
                }
            }
            PageOp::Insert { .. } | PageOp::Add { .. } => {}
            PageOp::Move {
                object,
                parent,
                before,
            } => require(self.placed(*object) || self.placed_at(*object, *parent, *before))?,
            PageOp::Delete { object } => {
                if !self.ours(*object) && !self.new.nodes.contains_key(object) {
                    return Ok(None);
                }
                require(self.untouched(*object))?
            }
            PageOp::Level { paragraph, .. } => require(
                self.ours(*paragraph) || {
                    let level = |index: &Index| {
                        index.nodes.get(paragraph).map(|node| {
                            (
                                node.parent,
                                match &node.own {
                                    Own::Paragraph { level, .. } => *level,
                                    _ => 0,
                                },
                            )
                        })
                    };
                    level(&self.old).is_some() && level(&self.old) == level(&self.new)
                },
            )?,
            PageOp::Outline { object, edit } => {
                if self.ours(*object) {
                    return Ok(Some(op));
                }
                let edit: &OutlineEdit = edit;
                // What the edit sets, as each side has it.
                let part = |index: &Index| {
                    index.nodes.get(object).map(|node| match (&node.own, edit) {
                        (Own::Outline { layout, .. }, OutlineEdit::Position { .. }) => {
                            (layout.x, layout.y, None, None)
                        }
                        (Own::Outline { layout, .. }, OutlineEdit::Width { .. }) => (
                            layout.max_width,
                            layout.reserved_width,
                            layout.width_set_by_user,
                            None,
                        ),
                        (Own::Paragraph { collapsed, .. }, OutlineEdit::Collapsed(_)) => {
                            (None, None, None, Some(*collapsed))
                        }
                        _ => (None, None, None, None),
                    })
                };
                let (before, after) = (part(&self.old), part(&self.new));
                if before.is_some() && before != after {
                    // The remote set the same value: the edit is done.
                    let near = |a: Option<f32>, b: f32| a.is_some_and(|a| (a - b).abs() < 1e-3);
                    let done = match (after, edit) {
                        (Some((x, y, ..)), OutlineEdit::Position { x: px, y: py }) => {
                            near(x, *px) && near(y, *py)
                        }
                        (
                            Some((width, reserved, set, _)),
                            OutlineEdit::Width { points, user_set },
                        ) => near(width, *points) && reserved.is_none() && set == Some(*user_set),
                        (Some((.., collapsed)), OutlineEdit::Collapsed(value)) => {
                            collapsed == Some(*value)
                        }
                        _ => false,
                    };
                    return if done { Ok(None) } else { Err(Changed) };
                }
                require(before.is_some())?
            }
            PageOp::Paragraph { paragraph, .. }
            | PageOp::Style { paragraph, .. }
            | PageOp::List { paragraph, .. } => {
                require(self.kept(*paragraph) && self.new_has(*paragraph))?
            }
            PageOp::Tags { target, .. } => require(self.same_tags(*target))?,
            PageOp::Picture {
                picture: object, ..
            }
            | PageOp::Attachment {
                attachment: object, ..
            }
            | PageOp::Strokes { ink: object, .. } => {
                require(self.kept(*object) && self.new_has(*object))?
            }
            PageOp::Table { table, edit } => match edit {
                TableEdit::Cell { cell, .. } => require(self.kept(*cell) && self.new_has(*cell))?,
                TableEdit::DeleteRow(row) => require(self.kept(*table) && self.untouched(*row))?,
                TableEdit::DeleteColumn(_) => require(self.untouched(*table))?,
                TableEdit::Rows { .. }
                | TableEdit::Column { .. }
                | TableEdit::Columns(_)
                | TableEdit::Borders(_) => require(self.kept(*table) && self.new_has(*table))?,
            },
        }
        Ok(Some(op))
    }

    /// Whether the remote put the object where a move places it: under `parent` (the page
    /// when `None`), before `before` or last.
    fn placed_at(&self, id: ExGuid, parent: Option<ExGuid>, before: Option<ExGuid>) -> bool {
        let Some(node) = self.new.nodes.get(&id) else {
            return false;
        };
        let container = parent.unwrap_or(PAGE);
        let siblings = self.new.children(container);
        node.parent == container
            && siblings
                .iter()
                .position(|sibling| *sibling == id)
                .is_some_and(|at| siblings.get(at + 1).copied() == before)
    }

    /// Whether the remote kept a paragraph's, text's or table's tags.
    fn same_tags(&self, id: ExGuid) -> bool {
        if self.ours(id) {
            return true;
        }
        let tags = |index: &Index| {
            index.nodes.get(&id).map(|node| match &node.own {
                Own::Paragraph { tags, .. } | Own::Table { tags, .. } => Some(tags.clone()),
                Own::Text(text) => Some(text.tags.clone()),
                _ => None,
            })
        };
        let before = tags(&self.old);
        before.is_some() && before == tags(&self.new)
    }

    fn new_has(&self, id: ExGuid) -> bool {
        self.ours(id) || self.new.nodes.contains_key(&id)
    }

    /// The paragraph holding a text, as the base stored it.
    fn holder(&self, text: ExGuid) -> Option<ExGuid> {
        self.old.nodes.get(&text).map(|node| node.parent)
    }

    fn holder_text(&self, paragraph: ExGuid) -> Option<ExGuid> {
        match self.old.nodes.get(&paragraph).map(|node| &node.own) {
            Some(Own::Paragraph { content, .. }) => Some(*content),
            _ => None,
        }
    }

    /// Whether a split of the paragraph makes the same new paragraph on both sides: its
    /// container, style, format and children are as the base had them.
    fn splits_alike(&self, id: ExGuid) -> bool {
        if self.ours(id) {
            return true;
        }
        let shape = |index: &Index| {
            index.nodes.get(&id).map(|node| match &node.own {
                Own::Paragraph { style, format, .. } => {
                    Some((node.parent, *style, format.clone(), node.children.clone()))
                }
                _ => None,
            })
        };
        let before = shape(&self.old);
        before.is_some() && before == shape(&self.new)
    }
}

/// Where the remote replaced parts of a text, in order: each `start..end` of the text as
/// the replayed edits see it holds what the remote replaced by `end - start + delta` UTF-16
/// units. Outside them both texts hold the same characters in the same formats.
#[derive(Debug, Clone, Default, PartialEq)]
struct Changes(Vec<Region>);

#[derive(Debug, Clone, Copy, PartialEq)]
struct Region {
    start: u32,
    end: u32,
    delta: i64,
}

/// The characters of a text with their formats and UTF-16 widths.
fn units(paragraph: &Paragraph) -> Vec<(char, Format)> {
    let mut out = Vec::new();
    let mut spans = paragraph.spans().iter();
    let mut span = spans.next();
    for (byte, character) in paragraph.text().char_indices() {
        while span.is_some_and(|span| span.end <= byte) {
            span = spans.next();
        }
        out.push((
            character,
            span.map(|span| span.format.clone()).unwrap_or_default(),
        ));
    }
    out
}

/// Beyond this many cells of the alignment table the changed middle counts as one change.
const CELLS: usize = 4_000_000;

impl Changes {
    /// The runs where `new` differs from `old` in characters or formats, from a longest
    /// common subsequence of the two: where several align equally, one is chosen, which
    /// places an op among repeated characters one way of the equally valid ones.
    fn between(old: &Paragraph, new: &Paragraph) -> Self {
        let (a, b) = (units(old), units(new));
        let prefix = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
        let suffix = a[prefix..]
            .iter()
            .rev()
            .zip(b[prefix..].iter().rev())
            .take_while(|(x, y)| x == y)
            .count();
        let (x, y) = (&a[prefix..a.len() - suffix], &b[prefix..b.len() - suffix]);
        // Matched pairs of the middles, in order.
        let mut matched = Vec::new();
        if x.len().saturating_mul(y.len()) <= CELLS {
            let width = y.len() + 1;
            let mut table = vec![0_u32; (x.len() + 1) * width];
            for i in (0..x.len()).rev() {
                for j in (0..y.len()).rev() {
                    table[i * width + j] = if x[i] == y[j] {
                        table[(i + 1) * width + j + 1] + 1
                    } else {
                        table[(i + 1) * width + j].max(table[i * width + j + 1])
                    };
                }
            }
            let (mut i, mut j) = (0, 0);
            while i < x.len() && j < y.len() {
                if x[i] == y[j] {
                    matched.push((i, j));
                    i += 1;
                    j += 1;
                } else if table[(i + 1) * width + j] >= table[i * width + j + 1] {
                    i += 1;
                } else {
                    j += 1;
                }
            }
        }
        matched.push((x.len(), y.len()));
        let width = |units: &[(char, Format)]| -> u32 {
            units
                .iter()
                .map(|(character, _)| character.len_utf16() as u32)
                .sum()
        };
        let mut regions = Vec::new();
        let (mut at, mut i, mut j) = (width(&a[..prefix]), 0, 0);
        for (k, l) in matched {
            if (k, l) != (i, j) {
                let (removed, inserted) = (width(&x[i..k]), width(&y[j..l]));
                regions.push(Region {
                    start: at,
                    end: at + removed,
                    delta: i64::from(inserted) - i64::from(removed),
                });
                at += removed;
            }
            if k < x.len() {
                at += width(&x[k..k + 1]);
            }
            (i, j) = (k + 1, l + 1);
        }
        Self(regions)
    }

    /// The range on the remote text, when it lies clear of every change: a range may end
    /// where a change starts or start where one ends, an insertion point may not.
    fn map(&self, range: &Range<u32>) -> Option<Range<u32>> {
        let empty = range.start == range.end;
        let mut shift = 0;
        for region in &self.0 {
            if range.end < region.start || (!empty && range.end == region.start) {
                break;
            }
            if range.start > region.end || (!empty && range.start == region.end) {
                shift += region.delta;
            } else {
                return None;
            }
        }
        let shifted = |at: u32| u32::try_from(i64::from(at) + shift).ok();
        Some(shifted(range.start)?..shifted(range.end)?)
    }

    /// Follows a replacement of `range`, clear of every change, by `length` units.
    fn replaced(&mut self, range: &Range<u32>, length: u32) {
        let moved = i64::from(length) - i64::from(range.end - range.start);
        for region in &mut self.0 {
            if region.start >= range.end {
                region.start = (i64::from(region.start) + moved) as u32;
                region.end = (i64::from(region.end) + moved) as u32;
            }
        }
    }

    /// Cuts the text at `at`, clear of every change, keeping the changes left of it (text
    /// typed at the cut included) and returning those right of it, measured from the cut.
    fn split(&mut self, at: u32) -> Self {
        let right = self
            .0
            .iter()
            .filter(|region| region.end > at)
            .map(|region| Region {
                start: region.start - at,
                end: region.end - at,
                delta: region.delta,
            })
            .collect();
        self.0.retain(|region| region.end <= at);
        Self(right)
    }
}

/// What an object holds apart from the objects under it.
#[derive(Debug, Clone, PartialEq)]
enum Own {
    Outline {
        title: bool,
        min_width: Option<f32>,
        layout: Layout,
        indents: Vec<f32>,
        unsupported: Vec<Unsupported>,
    },
    Title {
        date: Option<ExGuid>,
        layout: Layout,
    },
    Paragraph {
        level: u32,
        style: Option<ExGuid>,
        format: Format,
        lists: Vec<ExGuid>,
        tags: Vec<Tag>,
        media: MediaIndex,
        collapsed: bool,
        content: ExGuid,
    },
    Text(TextObject),
    Table {
        columns: Vec<TableColumn>,
        borders: Option<bool>,
        layout: Layout,
        tags: Vec<Tag>,
    },
    Row,
    Cell {
        layout: Layout,
        indents: Vec<f32>,
        shading: Option<u32>,
        unsupported: Vec<Unsupported>,
    },
    Image(Image),
    Attachment(Attachment),
    Ink(Ink),
    Unsupported(Unsupported),
}

#[derive(Debug)]
struct Node {
    own: Own,
    /// The containing object; the page is the default identity.
    parent: ExGuid,
    children: Vec<ExGuid>,
}

impl Node {
    /// Everything directly under the object, a paragraph's content included.
    fn below(&self) -> impl Iterator<Item = ExGuid> + '_ {
        let content = match &self.own {
            Own::Paragraph { content, .. } => Some(*content),
            _ => None,
        };
        content.into_iter().chain(self.children.iter().copied())
    }
}

#[derive(Default)]
struct Index {
    nodes: BTreeMap<ExGuid, Node>,
    page: Vec<ExGuid>,
}

const PAGE: ExGuid = ExGuid {
    guid: [0; 16],
    n: 0,
};

impl Index {
    fn of(page: &Page) -> Self {
        let mut index = Self::default();
        for object in &page.objects {
            index.page.push(object.id());
            match object {
                PageObject::Outline(outline) => index.outline(outline, PAGE),
                PageObject::Title(title) => {
                    index.add(
                        title.id,
                        PAGE,
                        Own::Title {
                            date: title.date,
                            layout: title.layout.clone(),
                        },
                        title.outlines.iter().map(|outline| outline.id).collect(),
                    );
                    for outline in &title.outlines {
                        index.outline(outline, title.id);
                    }
                }
                PageObject::Image(image) => {
                    index.add(image.id, PAGE, Own::Image(image.clone()), Vec::new())
                }
                PageObject::Ink(ink) => index.add(ink.id, PAGE, Own::Ink(ink.clone()), Vec::new()),
                PageObject::Unsupported(object) => index.add(
                    object.id,
                    PAGE,
                    Own::Unsupported(object.clone()),
                    Vec::new(),
                ),
            }
        }
        index
    }

    fn children(&self, id: ExGuid) -> &[ExGuid] {
        if id == PAGE {
            &self.page
        } else {
            self.nodes.get(&id).map_or(&[], |node| &node.children)
        }
    }

    fn add(&mut self, id: ExGuid, parent: ExGuid, own: Own, children: Vec<ExGuid>) {
        self.nodes.insert(
            id,
            Node {
                own,
                parent,
                children,
            },
        );
    }

    fn outline(&mut self, outline: &Outline, parent: ExGuid) {
        let children = self.paragraphs(&outline.paragraphs, outline.id);
        self.add(
            outline.id,
            parent,
            Own::Outline {
                title: outline.title,
                min_width: outline.min_width,
                layout: outline.layout.clone(),
                indents: outline.indents.clone(),
                unsupported: outline.unsupported.clone(),
            },
            children,
        );
    }

    /// Indexes a flat paragraph list; returns the container's direct children.
    fn paragraphs(&mut self, list: &[PageParagraph], container: ExGuid) -> Vec<ExGuid> {
        let mut children: BTreeMap<ExGuid, Vec<ExGuid>> = BTreeMap::new();
        for paragraph in list {
            children
                .entry(paragraph.parent.unwrap_or(container))
                .or_default()
                .push(paragraph.id);
        }
        for paragraph in list {
            let content = match &paragraph.content {
                ParagraphContent::Text(text) => {
                    self.add(text.id, paragraph.id, Own::Text(text.clone()), Vec::new());
                    text.id
                }
                ParagraphContent::Table(table) => {
                    let rows = table.rows.iter().map(|row| row.id).collect();
                    for row in &table.rows {
                        let cells = row.cells.iter().map(|cell| cell.id).collect();
                        for cell in &row.cells {
                            let children = self.paragraphs(&cell.paragraphs, cell.id);
                            self.add(
                                cell.id,
                                row.id,
                                Own::Cell {
                                    layout: cell.layout.clone(),
                                    indents: cell.indents.clone(),
                                    shading: cell.shading,
                                    unsupported: cell.unsupported.clone(),
                                },
                                children,
                            );
                        }
                        self.add(row.id, table.id, Own::Row, cells);
                    }
                    self.add(
                        table.id,
                        paragraph.id,
                        Own::Table {
                            columns: table.columns.clone(),
                            borders: table.borders,
                            layout: table.layout.clone(),
                            tags: table.tags.clone(),
                        },
                        rows,
                    );
                    table.id
                }
                ParagraphContent::Image(image) => {
                    self.add(
                        image.id,
                        paragraph.id,
                        Own::Image(image.clone()),
                        Vec::new(),
                    );
                    image.id
                }
                ParagraphContent::Attachment(attachment) => {
                    self.add(
                        attachment.id,
                        paragraph.id,
                        Own::Attachment(attachment.clone()),
                        Vec::new(),
                    );
                    attachment.id
                }
                ParagraphContent::Ink(ink) => {
                    self.add(ink.id, paragraph.id, Own::Ink(ink.clone()), Vec::new());
                    ink.id
                }
                ParagraphContent::Unsupported(object) => {
                    self.add(
                        object.id,
                        paragraph.id,
                        Own::Unsupported(object.clone()),
                        Vec::new(),
                    );
                    object.id
                }
            };
            self.add(
                paragraph.id,
                paragraph.parent.unwrap_or(container),
                Own::Paragraph {
                    level: paragraph.level,
                    style: paragraph.style,
                    format: paragraph.format.clone(),
                    lists: paragraph.lists.clone(),
                    tags: paragraph.tags.clone(),
                    media: paragraph.media.clone(),
                    collapsed: paragraph.collapsed,
                    content,
                },
                children.remove(&paragraph.id).unwrap_or_default(),
            );
        }
        children.remove(&container).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> Paragraph {
        Paragraph::new(s.into(), Format::default())
    }

    #[test]
    fn ranges_clear_of_the_remote_changes_shift_past_them() {
        let region = |start, end, delta| Region { start, end, delta };
        let changes = Changes::between(&text("one two three"), &text("one 2 three"));
        assert_eq!(changes.0, [region(4, 7, -2)]);
        assert_eq!(changes.map(&(0..3)), Some(0..3));
        assert_eq!(changes.map(&(0..4)), Some(0..4));
        assert_eq!(changes.map(&(8..13)), Some(6..11));
        assert_eq!(changes.map(&(7..8)), Some(5..6));
        assert_eq!(changes.map(&(13..13)), Some(11..11));
        assert_eq!(changes.map(&(4..4)), None);
        assert_eq!(changes.map(&(7..7)), None);
        assert_eq!(changes.map(&(3..5)), None);
        let mut moved = changes.clone();
        moved.replaced(&(0..0), 5);
        assert_eq!(moved.0, [region(9, 12, -2)]);
        let both = Changes::between(&text("ab🦀cd"), &text("Xab🦀cYd"));
        assert_eq!(both.0, [region(0, 0, 1), region(5, 5, 1)]);
        assert_eq!(both.map(&(2..4)), Some(3..5));
        assert_eq!(both.map(&(1..1)), Some(2..2));
        assert_eq!(both.map(&(0..0)), None);
        assert_eq!(both.map(&(3..6)), None);
        let bold = Format {
            bold: Some(true),
            ..Default::default()
        };
        let formatted = Changes::between(
            &text("abc"),
            &Paragraph::from_runs([
                ("a".to_owned(), Format::default()),
                ("b".to_owned(), bold),
                ("c".to_owned(), Format::default()),
            ]),
        );
        assert_eq!(formatted.0, [region(1, 2, 0)]);
        let emoji = Changes::between(&text("🦀a"), &text("🦀ab"));
        assert_eq!(emoji.0, [region(3, 3, 1)]);
        let mut cut = Changes::between(&text("abcdef"), &text("aXbcdeYf"));
        assert_eq!(cut.map(&(3..4)), Some(4..5));
        let right = cut.split(3);
        assert_eq!(
            (cut.0, right.0),
            (vec![region(1, 1, 1)], vec![region(2, 2, 1)])
        );
        let mut typed = Changes::between(&text("ab🦀cd"), &text("abX🦀cd"));
        assert_eq!(typed.map(&(2..3)), Some(3..4));
        assert!(typed.split(2).0.is_empty());
        assert_eq!(typed.0, [region(2, 2, 1)]);
    }
}
