//! Rebasing queued edits onto a changed remote section. Each op replays on the remote
//! section when the objects it reads are as the base image had them; text ops on a text
//! the remote also changed shift past the remote's changes when they stay clear of them;
//! a move, deletion or outline setting the remote already made is dropped as done.
//! An op on a page the remote changed where it did is dropped, with every later op naming
//! the objects it named, and the page conflicts: as OneNote 2010 does, the remote's version
//! stays the page and the local one becomes a conflict page under it (`conflict_page`).
//! Page-list edits merge as OneNote 2010 merges page series: a page the remote moved keeps
//! the remote's place, and a page the queue placed goes before the next page in the queue's
//! order the remote left in place (`corpus/conflict-page/native-pages`, `native-restore`).
//! Objects the queue itself created are not in the base, so nothing remote can have
//! changed them.

use crate::{PendingEdit, Result};
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

/// The remote changed what an op reads: the op does not replay.
struct Conflicting;

/// The queue replayed on the remote section.
pub(crate) struct Merged {
    /// Edits whose ops changed. An edit whose ops were all dropped stays, empty, to be
    /// published and acknowledged with its batch.
    pub rewritten: Vec<(u64, Edit)>,
    /// Pages whose local version the remote could not take, with who made the first edit
    /// that did not replay and the objects the dropped ops named.
    pub conflicts: BTreeMap<ExGuid, (String, BTreeSet<ExGuid>)>,
    /// Pages the base listed that the remote moved (`moved`).
    pub moved: BTreeSet<ExGuid>,
}

/// Replays `edits` on `new`, the remote section; `old` is the base they were made on.
/// Ops on the pages in `converged`, which the remote holds as the edits leave them, are done.
pub(crate) fn rebase(
    old: &mut Section<'_>,
    new: &mut Section<'_>,
    edits: &[PendingEdit],
    converged: &BTreeSet<ExGuid>,
) -> Result<Merged> {
    let before: BTreeMap<ExGuid, ExGuid> = old.revisions().collect();
    let after: BTreeMap<ExGuid, ExGuid> = new.revisions().collect();
    let orders = (order(old)?, order(new)?);
    let mut state = Replay {
        before,
        after,
        local: orders.0.clone(),
        moved: moved(old, new)?,
        orders,
        created: BTreeSet::new(),
        diffs: BTreeMap::new(),
    };
    let mut merged = Merged {
        rewritten: Vec::new(),
        conflicts: BTreeMap::new(),
        moved: state.moved.clone(),
    };
    for queued in edits {
        let mut ops = Vec::new();
        let mut changed = false;
        for op in &queued.edit.ops {
            let apply = |state: &mut Replay, new: &mut Section<'_>, op: &Op| {
                state.apply(new, &queued.author, queued.edit.at, op)
            };
            match op {
                Op::Page { space, op: page_op } => {
                    if converged.contains(space) {
                        changed = true;
                        continue;
                    }
                    // An op naming what a dropped op named would build on it.
                    let named = named(page_op);
                    if let Some((_, dropped)) = merged.conflicts.get_mut(space)
                        && named.iter().any(|id| dropped.contains(id))
                    {
                        dropped.extend(named);
                        changed = true;
                        continue;
                    }
                    let replayed = match state.check(old, new, *space, page_op)? {
                        Ok(None) => {
                            changed = true;
                            continue;
                        }
                        Ok(Some(mapped)) => {
                            let mapped = Op::Page {
                                space: *space,
                                op: mapped,
                            };
                            apply(&mut state, new, &mapped)?.then_some(mapped)
                        }
                        Err(Conflicting) => None,
                    };
                    match replayed {
                        Some(mapped) => {
                            changed |= mapped != *op;
                            ops.push(mapped);
                        }
                        None => {
                            changed = true;
                            merged
                                .conflicts
                                .entry(*space)
                                .or_insert_with(|| (queued.author.clone(), BTreeSet::new()))
                                .1
                                .extend(named);
                        }
                    }
                }
                Op::Section(section_op) => {
                    advance(&mut state.local, section_op);
                    let mut kept = state.form(new, section_op)?.map(Op::Section);
                    if let Some(form) = &kept
                        && !apply(&mut state, new, form)?
                    {
                        kept = None;
                    }
                    changed |= kept.as_ref() != Some(op);
                    ops.extend(kept);
                }
            }
        }
        if changed {
            merged.rewritten.push((
                queued.id,
                Edit {
                    at: queued.edit.at,
                    ops,
                },
            ));
        }
    }
    Ok(merged)
}

/// The stored objects a page op names; a conflict page marks those it holds.
fn named(op: &PageOp) -> Vec<ExGuid> {
    match op {
        PageOp::Text { text, .. }
        | PageOp::Format { text, .. }
        | PageOp::Link { text, .. }
        | PageOp::Equation { text, .. }
        | PageOp::Split { text, .. } => vec![*text],
        PageOp::Date { fields, .. } => fields.iter().map(|(text, _)| *text).collect(),
        PageOp::Color(_) | PageOp::RuleLines(_) => Vec::new(),
        PageOp::Join { left, right } => vec![*left, *right],
        PageOp::Insert { container, .. } => vec![*container],
        PageOp::Move { object, .. }
        | PageOp::Delete { object }
        | PageOp::Outline { object, .. } => {
            vec![*object]
        }
        PageOp::Level { paragraph, .. }
        | PageOp::Paragraph { paragraph, .. }
        | PageOp::Style { paragraph, .. }
        | PageOp::List { paragraph, .. } => vec![*paragraph],
        PageOp::Tags { target, .. } => vec![*target],
        PageOp::Add { object, .. } => vec![object.id()],
        PageOp::Picture {
            picture: object, ..
        }
        | PageOp::Attachment {
            attachment: object, ..
        }
        | PageOp::Strokes { ink: object, .. } => vec![*object],
        PageOp::Table { table, edit } => match edit {
            TableEdit::Cell { cell, .. } => vec![*cell],
            TableEdit::DeleteRow(row) => vec![*row],
            _ => vec![*table],
        },
    }
}

/// The edit keeping `page`, the version of page `space` the remote could not take, and
/// applies it to `new`, the merged section: a conflict page under the remote's page
/// marking `objects` where `page` holds them, or, where the remote removed the page, the
/// page put back as a copy before the next page after it in `local`, the section as the
/// queue leaves it, that the remote did not move (`moved`). Content outside the page model
/// stays on the remote's page only; `at` is now.
#[allow(clippy::too_many_arguments)]
pub(crate) fn conflict_page(
    new: &mut Section<'_>,
    local: &mut Section<'_>,
    moved: &BTreeSet<ExGuid>,
    space: ExGuid,
    page: &Page,
    author: &str,
    objects: &BTreeSet<ExGuid>,
    at: u64,
) -> Result<Edit> {
    let mut page = page.clone();
    page.objects
        .retain(|object| !matches!(object, PageObject::Unsupported(_)));
    let listed = order(new)?;
    let ops = if listed.iter().any(|(listed, _)| *listed == space) {
        let index = Index::of(&page);
        let mut marked: Vec<ExGuid> = objects
            .iter()
            .copied()
            .filter(|id| index.nodes.contains_key(id))
            .collect();
        let copy = page.copy_with(&mut marked)?;
        let titled = page
            .objects
            .iter()
            .any(|object| matches!(object, PageObject::Title(_)));
        vec![Op::Section(SectionOp::Conflict {
            of: space,
            creation: PageCreation::new(None, titled.then_some(page.title.as_str()), author)?,
            page: copy,
            objects: marked,
        })]
    } else {
        // A copy under fresh identities, as OneNote 2010 puts a removed page back.
        let queued = order(local)?;
        let at = queued.iter().position(|(queued, _)| *queued == space);
        let before = at
            .map_or(&[][..], |at| &queued[at + 1..])
            .iter()
            .map(|(next, _)| *next)
            .find(|next| !moved.contains(next) && listed.iter().any(|(listed, _)| listed == next));
        let creation = PageCreation::new(before, Some(&page.title), author)?;
        let mut ops = vec![Op::Section(SectionOp::Import {
            creation: creation.clone(),
            page: page.copy()?,
        })];
        if let Some(level) = at.map(|at| queued[at].1).filter(|level| *level > 1) {
            ops.push(Op::Section(SectionOp::Pages(vec![PageEdit::set_level(
                creation.space(),
                level,
            )?])));
        }
        ops
    };
    let edit = Edit { at, ops };
    new.apply(author, &edit)?;
    Ok(edit)
}

/// Pages the base and the remote both list that the remote moved: into another series or
/// level, or out of base order among the rest. OneNote 2010 and this writer give a moved
/// page a series of its own; older builds of this writer moved a page within its series.
fn moved(old: &mut Section<'_>, new: &mut Section<'_>) -> Result<BTreeSet<ExGuid>> {
    let series = (old.series()?, new.series()?);
    let base: BTreeMap<ExGuid, (usize, u32)> = order(old)?
        .into_iter()
        .enumerate()
        .map(|(at, (space, level))| (space, (at, level)))
        .collect();
    let listed = order(new)?;
    let kept: Vec<(ExGuid, usize)> = listed
        .iter()
        .filter_map(|&(space, level)| {
            let (at, before) = base.get(&space)?;
            (*before == level && series.0.get(&space) == series.1.get(&space))
                .then_some((space, *at))
        })
        .collect();
    // A longest subsequence of `kept` in base order (patience sorting).
    let mut tails: Vec<usize> = Vec::new();
    let mut previous = vec![None; kept.len()];
    for (i, (_, at)) in kept.iter().enumerate() {
        let k = tails.partition_point(|tail| kept[*tail].1 < *at);
        previous[i] = k.checked_sub(1).map(|k| tails[k]);
        if k == tails.len() {
            tails.push(i);
        } else {
            tails[k] = i;
        }
    }
    let mut unmoved = BTreeSet::new();
    let mut next = tails.last().copied();
    while let Some(i) = next {
        unmoved.insert(kept[i].0);
        next = previous[i];
    }
    Ok(listed
        .into_iter()
        .map(|(space, _)| space)
        .filter(|space| base.contains_key(space) && !unmoved.contains(space))
        .collect())
}

/// Applies a queued page-list edit to `local`, the page order as the queue leaves it.
fn advance(local: &mut Order, op: &SectionOp) {
    let insert = |local: &mut Order, page: (ExGuid, u32), before: Option<ExGuid>| {
        let at = before
            .and_then(|before| local.iter().position(|(space, _)| *space == before))
            .unwrap_or(local.len());
        local.insert(at, page);
    };
    match op {
        SectionOp::Create(creation) | SectionOp::Import { creation, .. } => {
            insert(local, (creation.space(), 1), creation.before());
        }
        SectionOp::Pages(edits) => {
            for edit in edits {
                let Some(at) = local.iter().position(|(space, _)| *space == edit.space()) else {
                    continue;
                };
                local[at].1 = edit.level();
                if let PagePosition::Before(before) = edit.position() {
                    let page = local.remove(at);
                    insert(local, page, before);
                }
            }
        }
        SectionOp::Delete(spaces) => local.retain(|(space, _)| !spaces.contains(space)),
        SectionOp::Conflict { .. }
        | SectionOp::Color(_)
        | SectionOp::RestoreVersion { .. }
        | SectionOp::DeleteVersions { .. } => {}
    }
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
    /// Page order as the queue leaves it, up to the op replaying.
    local: Order,
    /// Pages the base listed that the remote moved (`moved`).
    moved: BTreeSet<ExGuid>,
    /// Page spaces the replayed edits created.
    created: BTreeSet<ExGuid>,
    /// Per page the remote changed, what it changed; `None` when it left the page alone.
    diffs: BTreeMap<ExGuid, Option<Diff>>,
}

impl Replay {
    /// Applies one op to the remote section; false when the section refused it.
    fn apply(&mut self, new: &mut Section<'_>, author: &str, at: u64, op: &Op) -> Result<bool> {
        match new.apply(
            author,
            &Edit {
                at,
                ops: vec![op.clone()],
            },
        ) {
            Ok(()) => {
                if let Op::Section(
                    SectionOp::Create(creation)
                    | SectionOp::Import { creation, .. }
                    | SectionOp::Conflict { creation, .. },
                ) = op
                {
                    self.created.insert(creation.space());
                }
                Ok(true)
            }
            Err(OpError::Failed(error)) => Err(error.into()),
            Err(_) => Ok(false),
        }
    }

    /// The page op as it replays on the remote section; `None` when the remote made it.
    fn check(
        &mut self,
        old: &mut Section<'_>,
        new: &mut Section<'_>,
        space: ExGuid,
        op: &PageOp,
    ) -> Result<std::result::Result<Option<PageOp>, Conflicting>> {
        if !self.listed(space) {
            return Ok(Err(Conflicting));
        }
        let diff = match self.diffs.get_mut(&space) {
            Some(diff) => diff,
            None => {
                let diff = match (self.before.get(&space), self.after.get(&space)) {
                    (Some(before), Some(after)) if before != after => Some(Diff {
                        old: Index::of(&old.page(space)?),
                        new: Index::of(&new.page(space)?),
                        regions: BTreeMap::new(),
                    }),
                    _ => None,
                };
                self.diffs.entry(space).or_insert(diff)
            }
        };
        Ok(match diff {
            None => Ok(Some(op.clone())),
            Some(diff) => diff.check(op),
        })
    }

    /// Whether the remote section lists the page, or the replayed edits created it; a page
    /// removed from the list keeps its stored revisions.
    fn listed(&self, space: ExGuid) -> bool {
        self.created.contains(&space) || self.orders.1.iter().any(|(page, _)| *page == space)
    }

    /// Where a page the queue put before `before` goes on the remote: before the first page
    /// from `before` on, in the queue's order, that the remote lists where the base had it,
    /// or last, as OneNote 2010 places a page series only one side changed.
    fn anchor(&self, listed: &Order, page: ExGuid, before: Option<ExGuid>) -> Option<ExGuid> {
        let from = self
            .local
            .iter()
            .position(|(space, _)| Some(*space) == before)?;
        self.local[from..]
            .iter()
            .map(|(space, _)| *space)
            .find(|space| {
                *space != page
                    && !self.moved.contains(space)
                    && listed.iter().any(|(listed, _)| listed == space)
            })
    }

    /// A section op as it replays on the remote section, or `None`: a page placed by
    /// `anchor`, the page edits of pages the remote still has and did not move, removals of
    /// pages the remote still has (a page it changed is not removed), a conflict page's
    /// content as a page of its own where the remote removed its page.
    fn form(&self, new: &mut Section<'_>, op: &SectionOp) -> Result<Option<SectionOp>> {
        let conflicts: BTreeSet<ExGuid> = new
            .conflicts()?
            .into_iter()
            .flat_map(|(_, pages)| pages.into_iter().map(|page| page.space))
            .collect();
        let listed = order(new)?;
        let anchored = |creation: &PageCreation| {
            creation.reposition(self.anchor(&listed, creation.space(), creation.before()))
        };
        Ok(match op {
            SectionOp::Create(creation) => Some(SectionOp::Create(anchored(creation)?)),
            SectionOp::Import { creation, page } => Some(SectionOp::Import {
                creation: anchored(creation)?,
                page: page.clone(),
            }),
            SectionOp::Color(_) => Some(op.clone()),
            SectionOp::Conflict {
                of, creation, page, ..
            } => Some(if self.listed(*of) {
                op.clone()
            } else {
                SectionOp::Import {
                    creation: creation.clone(),
                    page: page.clone(),
                }
            }),
            // A page the remote moved or re-leveled keeps the remote's placement.
            SectionOp::Pages(edits) => {
                let mut kept = Vec::new();
                for edit in edits {
                    if self.moved.contains(&edit.space())
                        || listed.iter().all(|(listed, _)| *listed != edit.space())
                    {
                        continue;
                    }
                    kept.push(match edit.position() {
                        PagePosition::Keep => edit.clone(),
                        PagePosition::Before(before) => edit.reposition(
                            PagePosition::Before(self.anchor(&listed, edit.space(), before)),
                            edit.level(),
                        )?,
                    });
                }
                (!kept.is_empty()).then_some(SectionOp::Pages(kept))
            }
            // A version the remote deleted is gone; restoring one keeps the remote's page as
            // the newest version.
            SectionOp::RestoreVersion { page, version, .. } => (self.listed(*page)
                && listed_versions(new, *page)?.contains(version))
            .then(|| op.clone()),
            SectionOp::DeleteVersions { page, versions } => {
                let listed = listed_versions(new, *page)?;
                let versions: Vec<ExGuid> = versions
                    .iter()
                    .filter(|version| listed.contains(version))
                    .copied()
                    .collect();
                (!versions.is_empty()).then_some(SectionOp::DeleteVersions {
                    page: *page,
                    versions,
                })
            }
            SectionOp::Delete(deleted) => {
                let deleted: Vec<ExGuid> = deleted
                    .iter()
                    .filter(|space| {
                        self.listed(**space) && self.before.get(space) == self.after.get(space)
                            || conflicts.contains(space)
                    })
                    .copied()
                    .collect();
                (!deleted.is_empty()).then_some(SectionOp::Delete(deleted))
            }
        })
    }
}

/// The versions `section` lists for `page`.
fn listed_versions(section: &mut Section<'_>, page: ExGuid) -> Result<Vec<ExGuid>> {
    Ok(section
        .versions()?
        .into_iter()
        .filter(|(listed, _)| *listed == page)
        .flat_map(|(_, versions)| versions.into_iter().map(|version| version.context))
        .collect())
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
    fn region(&mut self, text: ExGuid) -> std::result::Result<Option<&mut Changes>, Conflicting> {
        if !self.regions.contains_key(&text) {
            if self.ours(text) {
                return Ok(None);
            }
            let (Some(Own::Text(a)), Some(Own::Text(b))) = (
                self.old.nodes.get(&text).map(|node| &node.own),
                self.new.nodes.get(&text).map(|node| &node.own),
            ) else {
                return Err(Conflicting);
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
    fn check(&mut self, op: &PageOp) -> std::result::Result<Option<PageOp>, Conflicting> {
        let require = |ok: bool| if ok { Ok(()) } else { Err(Conflicting) };
        let mut op = op.clone();
        match &mut op {
            PageOp::Text { text, range, with } => {
                if let Some(region) = self.region(*text)? {
                    let mapped = region.map(range).ok_or(Conflicting)?;
                    region.replaced(range, with.encode_utf16().count() as u32);
                    *range = mapped;
                }
            }
            PageOp::Format { text, range, .. } => {
                if let Some(region) = self.region(*text)? {
                    *range = region.map(range).ok_or(Conflicting)?;
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
                    let cut = region.map(&(*at..*at + 1)).ok_or(Conflicting)?;
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
            // A page's colour and rule lines are the latest set, whichever side set them.
            PageOp::Insert { .. }
            | PageOp::Add { .. }
            | PageOp::Color(_)
            | PageOp::RuleLines(_) => {}
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
                        (
                            Own::Outline { layout, .. } | Own::Ink(Ink { layout, .. }),
                            OutlineEdit::Position { .. },
                        ) => (layout.x, layout.y, None, None),
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
                    return if done { Ok(None) } else { Err(Conflicting) };
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
