use crate::{
    Error, ExGuid, FileType, Object,
    document::{Element, Kind, Revision},
    write::{Commit, LiveRevision, PropertyObject, declared, differing},
};
#[cfg(test)]
use crate::{RevisionIndex, Store, document::Document, write::chain_depth};
use bumpalo::Bump;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

type Result<T> = std::result::Result<T, Error>;

/// Objects a writer stores in one revision of a page space.
pub(crate) type Changes = BTreeMap<ExGuid, PropertyObject>;

/// Payloads a store holds, by identity.
pub(crate) type Files<'a> = std::rc::Rc<dyn Fn([u8; 16]) -> Result<&'a [u8]> + 'a>;

/// A page space's active revision with the views its writers read, kept as appending
/// their revisions and parsing the result would leave it.
#[derive(Clone)]
pub(crate) struct ActivePage<'a> {
    files: Files<'a>,
    pub pages: Vec<ExGuid>,
    pub live: LiveRevision<'a>,
    /// Elements of the reachable objects.
    pub view: Revision<'a>,
    /// `view.parents(&pages)`.
    pub parents: BTreeMap<ExGuid, Vec<ExGuid>>,
    /// Elements of `view` carrying the page-title flag.
    titles: BTreeSet<ExGuid>,
    /// Payloads embedded by revisions `write` applied, in order.
    pub payloads: Vec<([u8; 16], &'a [u8])>,
}

impl<'a> ActivePage<'a> {
    /// The active page of `space`, for tests comparing writers.
    #[cfg(test)]
    pub(crate) fn parse(index: &'a RevisionIndex<'a>, space: ExGuid) -> Result<Self> {
        index.validate_current()?;
        let mut document = Document::parse(index)?;
        let pages = document.pages_in(space)?;
        let view = document
            .spaces
            .remove(&space)
            .and_then(|mut space| {
                let active = *space.contexts.get(&ExGuid::default())?;
                space.revisions.remove(&active)
            })
            .ok_or(Error {
                offset: 0,
                message: "The active page is unavailable",
            })?;
        let rid = index.active(space)?;
        let live = LiveRevision::new(index.resolve(space, rid)?, chain_depth(index, space, rid))?;
        let store: &'a Store<'a> = index.store;
        Self::viewed(
            std::rc::Rc::new(|guid| store.file_data(guid)),
            pages,
            live,
            view,
        )
    }

    /// The page space `live` holds, whose stored payloads `files` reads.
    pub(crate) fn open(
        space: ExGuid,
        live: LiveRevision<'a>,
        file_type: FileType,
        files: Files<'a>,
    ) -> Result<Self> {
        let (view, _) = Revision::parse(space, &live.revision, file_type, &mut |guid| files(guid))?;
        let pages = manifest_pages(&view);
        Self::viewed(files, pages, live, view)
    }

    fn viewed(
        files: Files<'a>,
        pages: Vec<ExGuid>,
        live: LiveRevision<'a>,
        view: Revision<'a>,
    ) -> Result<Self> {
        let parents = view.parents(&pages)?;
        let titles = view
            .nodes
            .iter()
            .filter(|(_, node)| is_title(node))
            .map(|(id, _)| *id)
            .collect();
        Ok(Self {
            files,
            pages,
            live,
            view,
            parents,
            titles,
            payloads: Vec::new(),
        })
    }

    /// The element of an object, reading payloads `write` embedded as well as stored ones.
    pub(crate) fn element<'o>(&self, object: &Object<'o>) -> Result<Element<'o>>
    where
        'a: 'o,
    {
        Element::parse_with(object, FileType::Section, &mut |guid| match self
            .payloads
            .iter()
            .find(|(id, _)| *id == guid)
        {
            Some((_, payload)) => Ok(*payload),
            None => (self.files)(guid),
        })
    }

    /// `parents`, after checking that `object` and its ancestors are editable.
    pub(crate) fn editable_parents(
        &self,
        object: ExGuid,
    ) -> Result<&BTreeMap<ExGuid, Vec<ExGuid>>> {
        crate::edit::check_editable(&self.view, &self.parents, &self.pages, object)?;
        Ok(&self.parents)
    }

    /// `edit::page_title` of the view with `overlay` replacing or adding elements that
    /// neither carry the title flag nor detach any element that does.
    pub(crate) fn title(
        &self,
        overlay: &BTreeMap<ExGuid, Element<'_>>,
        text_update: Option<(ExGuid, &str)>,
    ) -> Result<Option<(ExGuid, bool, String)>> {
        crate::edit::title_of(
            |id| overlay.get(&id).or_else(|| self.view.nodes.get(&id)),
            &self.parents,
            self.titles.iter().copied(),
            &self.pages,
            text_update,
        )
    }

    /// Stores a writer's objects and payloads as the next revision; false when it stores
    /// nothing, as appending it would leave the image unchanged.
    #[cfg(test)]
    pub(crate) fn write(
        &mut self,
        arena: &'a Bump,
        payloads: &[([u8; 16], &[u8])],
        changes: Changes,
    ) -> Result<bool> {
        Ok(self.store(arena, payloads, changes)?.is_some())
    }

    /// `write`, returning the objects whose content, reachability or reference count may
    /// have moved; none when it stores nothing.
    pub(crate) fn store(
        &mut self,
        arena: &'a Bump,
        payloads: &[([u8; 16], &[u8])],
        changes: Changes,
    ) -> Result<Option<BTreeSet<ExGuid>>> {
        let changes = self.live.prepare(changes, true)?;
        for (guid, payload) in payloads {
            self.payloads.push((*guid, arena.alloc_slice_copy(payload)));
        }
        if changes.is_empty() {
            return Ok((!payloads.is_empty()).then(BTreeSet::new));
        }
        let mut objects = Vec::new();
        for (id, change) in changes {
            let bytes = arena.alloc_slice_copy(&change.bytes);
            objects.push((id, declared(change.jcid, bytes, change.global_ids)?));
        }
        let replaced: BTreeSet<ExGuid> = objects.iter().map(|(id, _)| *id).collect();
        let Commit {
            checkpoint,
            changed,
            touched,
        } = self.live.commit(objects)?;
        if checkpoint {
            let all: Vec<ExGuid> = self.live.revision.objects.keys().copied().collect();
            self.live.settle(all)?;
        } else {
            self.live.settle(changed)?;
        }
        self.refresh(&replaced, touched.clone())?;
        Ok(Some(touched))
    }

    /// Takes the stored form of `ids` from `file`, the revision a seal appended, where the
    /// revisions written here since the previous seal left them otherwise: a seal groups
    /// tables and aliases read-only objects across all of them at once.
    pub(crate) fn adopt(
        &mut self,
        file: &LiveRevision<'a>,
        ids: impl IntoIterator<Item = ExGuid>,
    ) -> Result<()> {
        let mut replacements = Vec::new();
        for id in ids {
            let Some(stored) = file.revision.objects.get(&id) else {
                continue;
            };
            match self.live.revision.objects.get_mut(&id) {
                Some(object) if object.jcid == stored.jcid && object.data == stored.data => {
                    object.global_ids = Arc::clone(&stored.global_ids);
                    object.reference_count = stored.reference_count;
                }
                _ if file.is_reachable(id) => replacements.push((id, stored.clone())),
                // Nothing reads an object the file leaves unreachable.
                _ => {}
            }
        }
        if !replacements.is_empty() {
            let replaced = replacements.iter().map(|(id, _)| *id).collect();
            let Commit { touched, .. } = self.live.commit(replacements)?;
            for id in &touched {
                if let (Some(object), Some(stored)) = (
                    self.live.revision.objects.get_mut(id),
                    file.revision.objects.get(id),
                ) {
                    object.global_ids = Arc::clone(&stored.global_ids);
                    object.reference_count = stored.reference_count;
                }
            }
            self.refresh(&replaced, touched)?;
        }
        self.live.depth = file.depth;
        Ok(())
    }

    /// Brings the view, parents and titles up to date with the revision for the objects a
    /// commit touched, which include those it replaced.
    fn refresh(&mut self, replaced: &BTreeSet<ExGuid>, touched: BTreeSet<ExGuid>) -> Result<()> {
        let mut attach = Vec::new();
        let mut detach = Vec::new();
        let mut gone = BTreeMap::new();
        for id in touched {
            let reachable = self.live.is_reachable(id);
            let viewed = self.view.nodes.contains_key(&id);
            if reachable == viewed && !(reachable && replaced.contains(&id)) {
                continue;
            }
            let fresh = if reachable {
                Some(self.element(&self.live.revision.objects[&id])?)
            } else {
                None
            };
            let old = self.view.nodes.remove(&id);
            // An element leaving the view detaches with its last parent.
            if let Some(node) = &fresh
                && (self.parents.contains_key(&id) || self.pages.contains(&id))
            {
                let before: Vec<ExGuid> = old.iter().flat_map(edges).collect();
                let after: Vec<ExGuid> = edges(node).collect();
                let (removed, added) = differing(&before, &after);
                detach.extend(removed.iter().map(|child| (id, *child)));
                attach.extend(added.iter().map(|child| (id, *child)));
            }
            self.titles.remove(&id);
            if let Some(node) = fresh {
                if is_title(&node) {
                    self.titles.insert(id);
                }
                self.view.nodes.insert(id, node);
            } else if let Some(node) = old {
                gone.insert(id, node);
            }
        }
        let node = |nodes: &BTreeMap<ExGuid, Element<'a>>, id: ExGuid| {
            nodes
                .get(&id)
                .or_else(|| gone.get(&id))
                .map(|node| edges(node).collect::<Vec<_>>())
                .ok_or(Error {
                    offset: 0,
                    message: "Page content is unavailable",
                })
        };
        // Attaching before detaching keeps a moved subtree from being walked twice.
        while let Some((parent, child)) = attach.pop() {
            let parents = self.parents.entry(child).or_default();
            parents.push(parent);
            if parents.len() == 1 && !self.pages.contains(&child) {
                attach.extend(
                    node(&self.view.nodes, child)?
                        .into_iter()
                        .map(|grandchild| (child, grandchild)),
                );
            }
        }
        while let Some((parent, child)) = detach.pop() {
            let parents = self.parents.get_mut(&child).unwrap();
            let at = parents.iter().position(|id| *id == parent).unwrap();
            parents.remove(at);
            if parents.is_empty() {
                self.parents.remove(&child);
                if !self.pages.contains(&child) {
                    detach.extend(
                        node(&self.view.nodes, child)?
                            .into_iter()
                            .map(|grandchild| (child, grandchild)),
                    );
                }
            }
        }
        Ok(())
    }
}

/// The pages a page space's manifest lists.
pub(crate) fn manifest_pages(view: &Revision<'_>) -> Vec<ExGuid> {
    match view.roots.get(&1).and_then(|id| view.nodes.get(id)) {
        Some(manifest) if matches!(manifest.kind, Kind::Manifest { .. }) => manifest
            .content
            .iter()
            .filter(|id| {
                matches!(
                    view.nodes.get(id).map(|node| &node.kind),
                    Some(Kind::Page { .. })
                )
            })
            .copied()
            .collect(),
        _ => Vec::new(),
    }
}

fn edges<'n>(node: &'n Element<'_>) -> impl Iterator<Item = ExGuid> + 'n {
    node.children
        .iter()
        .chain(&node.content)
        .chain(&node.structure)
        .copied()
}

fn is_title(node: &Element<'_>) -> bool {
    node.extra
        .first()
        .is_some_and(|fields| fields.iter().any(|field| field.id == 0x88001cb4))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::{Insertion, ParagraphSplit, TextAttribute, TreeEdit, document::Kind};

    /// An identity from `fresh_guid`, which seeded tests draw deterministically per thread.
    fn new_id() -> Result<ExGuid> {
        Ok(ExGuid {
            guid: crate::write::fresh_guid()?,
            n: 1,
        })
    }

    /// A seeded source of the typed writers' changes: insertions, moves, deletions, splits
    /// and formatting of what a page holds.
    pub(crate) struct Writes(pub u64);

    impl Writes {
        fn pick(&mut self, count: usize) -> usize {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (self.0 >> 33) as usize % count.max(1)
        }

        pub(crate) fn next(&mut self, active: &ActivePage<'_>) -> Result<Changes> {
            let nodes = &active.view.nodes;
            let listed = |kind: fn(&Kind<'_>) -> bool| -> Vec<ExGuid> {
                nodes
                    .iter()
                    .filter(|(id, node)| kind(&node.kind) && active.parents.contains_key(id))
                    .map(|(id, _)| *id)
                    .collect()
            };
            let paragraphs = listed(|kind| matches!(kind, Kind::Paragraph { .. }));
            let containers = listed(|kind| {
                matches!(
                    kind,
                    Kind::Paragraph { .. } | Kind::Outline { .. } | Kind::Cell { .. }
                )
            });
            let texts = listed(|kind| matches!(kind, Kind::RichText { .. }));
            let unavailable = Error {
                offset: 0,
                message: "The page has nothing to edit",
            };
            let paragraph = *paragraphs
                .get(self.pick(paragraphs.len()))
                .ok_or(unavailable)?;
            let container = *containers
                .get(self.pick(containers.len()))
                .ok_or(unavailable)?;
            let anchor = nodes[&container].children.get(self.pick(4)).copied();
            let text = *texts.get(self.pick(texts.len())).ok_or(unavailable)?;
            let words = ["", "a", "Two words", "東京 🦀", "longer text here"];
            let word = words[self.pick(words.len())];
            // `Section::apply` refuses an edit ending with a table cell emptied.
            let fills_a_cell = active.parents[&paragraph].iter().any(|parent| {
                matches!(nodes[parent].kind, Kind::Cell { .. })
                    && nodes[parent].children == [paragraph]
            });
            let choice = match self.pick(8) {
                4 | 5 if fills_a_cell => 0,
                choice => choice,
            };
            match choice {
                0..=3 => {
                    Insertion::paragraph(container, anchor, word, "Author").and_then(|insertion| {
                        let paragraph = new_id()?;
                        insertion.changes_as(active, paragraph, paragraph, new_id()?)
                    })
                }
                4 => TreeEdit::move_to(paragraph, container, anchor, "Author")
                    .and_then(|edit| edit.changes(active)),
                5 => TreeEdit::delete(paragraph, "Author").and_then(|edit| edit.changes(active)),
                6 => {
                    let at = self.pick(3) as u32;
                    let lists = (0..8).map(|_| new_id()).collect::<Result<Vec<_>>>()?;
                    ParagraphSplit::new(text, at, "Author")
                        .and_then(|split| split.changes_as(active, new_id()?, new_id()?, &lists))
                }
                _ => {
                    let end = self.pick(2) as u32;
                    let bold = self.pick(2) == 0;
                    crate::formatting::format_changes(
                        active,
                        text,
                        0..end,
                        &[TextAttribute::Bold(bold)],
                        &[],
                    )
                }
            }
        }
    }

    /// `active` equals the page `image` stores, views included.
    fn assert_stores(active: &ActivePage<'_>, image: &[u8], space: ExGuid) {
        let store = Store::parse(image).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let read = ActivePage::parse(&index, space).unwrap();
        assert_eq!(read.live.revision.roots, active.live.revision.roots);
        let objects = |page: &ActivePage<'_>| -> Vec<String> {
            page.live
                .revision
                .objects
                .iter()
                .map(|(id, object)| format!("{id} {object:?}"))
                .collect()
        };
        assert_eq!(objects(&read), objects(active));
        assert_eq!(format!("{:?}", read.view), format!("{:?}", active.view));
        let parents = |page: &ActivePage<'_>| -> Vec<(ExGuid, Vec<ExGuid>)> {
            page.parents
                .iter()
                .map(|(child, parents)| {
                    let mut parents = parents.clone();
                    parents.sort();
                    (*child, parents)
                })
                .collect()
        };
        assert_eq!(parents(&read), parents(active));
        assert_eq!(read.titles, active.titles);
    }

    /// Applies `steps` random writes to the first page of `source` both in memory and by
    /// appending each revision, comparing the two as it goes.
    fn check_writes(source: &[u8], steps: usize) {
        let mut image = source.to_vec();
        let store = Store::parse(source).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let (space, _) = Document::parse(&index).unwrap().pages().unwrap()[0];
        let arena = Bump::new();
        let mut active = ActivePage::parse(&index, space).unwrap();
        let mut writes = Writes(7);
        for step in 0..steps {
            let changes = writes.next(&active);
            let Ok(changes) = changes else {
                continue;
            };
            let store = Store::parse(&image).unwrap();
            let index = RevisionIndex::parse(&store).unwrap();
            let written =
                crate::write::write_revision_on(&index, space, |_| Ok(changes.clone())).unwrap();
            assert_eq!(
                active.write(&arena, &[], changes).unwrap(),
                written != image,
                "step {step}"
            );
            image = written;
            if step % 40 == 0 {
                assert_stores(&active, &image, space);
            }
        }
        assert_stores(&active, &image, space);
    }

    #[test]
    fn writes_leave_the_page_that_appending_and_reading_their_revisions_does() {
        // Seeded identities: the writes pick nodes in identity order.
        crate::write::GUIDS.set(Some(1 << 61));
        // Past 512 revisions, so the chain checkpoints once.
        check_writes(
            include_bytes!(
                "../../../corpus/native/20260905-05/snapshots/02-text/notebook/synthetic.one"
            ),
            720,
        );
        check_writes(
            include_bytes!("../../../corpus/m6/native-features-01/notebook/Features.one"),
            160,
        );
        check_writes(
            &crate::create_section("model.one", "First", "Author").unwrap(),
            160,
        );
    }
}
