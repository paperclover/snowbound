use crate::{
    Error, ExGuid, RevisionIndex, Store,
    create::{current_timestamps, properties, string},
    document::{Document, Kind, Revision},
    edit::{editable_parents, update_title},
    write::{PropertyObject, fresh_guid, write_revision},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

fn invalid(message: &'static str) -> Error {
    Error { offset: 0, message }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
enum Placement {
    Delete,
    Move {
        parent: ExGuid,
        before: Option<ExGuid>,
    },
}

/// An atomic subtree move or deletion on one active page.
/// Retain the intent across retries so an emptied cell's replacement keeps its identity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TreeEdit {
    guid: [u8; 16],
    object: ExGuid,
    placement: Placement,
    author: String,
    created: u32,
}

impl TreeEdit {
    /// Removes a paragraph or ordinary outline and its descendants from the active tree.
    /// Historical objects remain available; an emptied table cell receives an empty paragraph.
    pub fn delete(object: ExGuid, author: &str) -> Result<Self, Error> {
        Self::new(object, Placement::Delete, author)
    }

    /// Moves before a direct child, or appends when `before` is None.
    /// Paragraph destinations are outlines, groups, paragraphs or cells on the same page.
    /// Outlines remain direct page children. Content and explicit list formatting are preserved.
    pub fn move_to(
        object: ExGuid,
        parent: ExGuid,
        before: Option<ExGuid>,
        author: &str,
    ) -> Result<Self, Error> {
        Self::new(object, Placement::Move { parent, before }, author)
    }

    fn new(object: ExGuid, placement: Placement, author: &str) -> Result<Self, Error> {
        let intent = Self {
            guid: fresh_guid()?,
            object,
            placement,
            author: author.to_owned(),
            created: current_timestamps()?.0,
        };
        intent.validate()?;
        Ok(intent)
    }

    fn validate(&self) -> Result<(), Error> {
        if self.guid == [0; 16] || self.object.guid == [0; 16] || self.author.contains('\0') {
            return Err(invalid(
                "Choose page content and an author name without NUL",
            ));
        }
        if let Placement::Move { parent, before } = self.placement
            && (parent.guid == [0; 16] || before.is_some_and(|id| id.guid == [0; 16]))
        {
            return Err(invalid("Choose a destination and sibling on the same page"));
        }
        Ok(())
    }

    pub(crate) fn apply(&self, source: &[u8], space: ExGuid) -> Result<Vec<u8>, Error> {
        self.validate()?;
        let store = Store::parse(source)?;
        let index = RevisionIndex::parse(&store)?;
        index.validate_current()?;
        let mut document = Document::parse(&index)?;
        let pages: Vec<_> = document
            .pages()?
            .into_iter()
            .filter_map(|(sid, page)| (sid == space).then_some(page))
            .collect();
        let [page] = pages.as_slice() else {
            return Err(invalid("Tree editing requires a single active page"));
        };
        let semantic = document.spaces.remove(&space).unwrap();
        let rid = semantic.contexts[&ExGuid::default()];
        let mut view = semantic
            .revisions
            .into_iter()
            .find_map(|(id, view)| (id == rid).then_some(view))
            .unwrap();
        let parents = editable_parents(&view, &pages, self.object)?;
        let path = checked_path(&view, &parents, *page, self.object)?;
        let source_parent = *path
            .get(1)
            .ok_or_else(|| invalid("Select a paragraph or ordinary page outline"))?;
        let compatible = |parent: ExGuid| match view.nodes[&self.object].kind {
            Kind::Outline { .. } => parent == *page,
            Kind::Paragraph { .. } => matches!(
                view.nodes[&parent].kind,
                Kind::Outline { .. }
                    | Kind::OutlineGroup
                    | Kind::Paragraph { .. }
                    | Kind::Cell { .. }
            ),
            _ => false,
        };
        if !compatible(source_parent) || !view.nodes[&source_parent].children.contains(&self.object)
        {
            return Err(invalid("Select a paragraph or ordinary page outline"));
        }
        let mut changed_ids = BTreeSet::from([source_parent]);
        let mut affected = BTreeSet::from([self.object]);
        if let Placement::Move { parent, before } = self.placement {
            if checked_path(&view, &parents, *page, parent)?.contains(&self.object) {
                return Err(invalid("A subtree cannot move inside itself"));
            }
            let destination = &view.nodes[&parent];
            if !compatible(parent) || before.is_some_and(|id| !destination.children.contains(&id)) {
                return Err(invalid(
                    "Choose a compatible destination and one of its direct children",
                ));
            }
            let previous = destination.children.clone();
            if parent == source_parent && before == Some(self.object) {
                return Ok(source.to_vec());
            }
            view.nodes
                .get_mut(&source_parent)
                .unwrap()
                .children
                .retain(|id| *id != self.object);
            let children = &mut view.nodes.get_mut(&parent).unwrap().children;
            let position = before
                .map(|id| children.iter().position(|child| *child == id).unwrap())
                .unwrap_or(children.len());
            children.insert(position, self.object);
            if parent == source_parent && *children == previous {
                return Ok(source.to_vec());
            }
            changed_ids.insert(parent);
        } else {
            view.nodes
                .get_mut(&source_parent)
                .unwrap()
                .children
                .retain(|id| *id != self.object);
        }

        let mut empty_cell = None;
        let mut at = source_parent;
        loop {
            let node = &view.nodes[&at];
            if matches!(node.kind, Kind::Page { .. } | Kind::Paragraph { .. })
                && node.children.is_empty()
            {
                break;
            }
            if node.children.is_empty() {
                if matches!(node.kind, Kind::Cell { .. }) {
                    empty_cell = Some(at);
                    break;
                }
                if !matches!(node.kind, Kind::Outline { .. } | Kind::OutlineGroup) {
                    return Err(invalid("This container cannot be emptied by a tree edit"));
                }
                if node.extra[0].iter().any(|field| field.id == 0x08001d0c) {
                    return Err(invalid("The emptied container cannot be deleted"));
                }
                let path = checked_path(&view, &parents, *page, at)?;
                let parent = path[1];
                view.nodes
                    .get_mut(&parent)
                    .unwrap()
                    .children
                    .retain(|id| *id != at);
                changed_ids.remove(&at);
                changed_ids.insert(parent);
                at = parent;
                continue;
            }
            if matches!(
                node.kind,
                Kind::Outline { .. } | Kind::OutlineGroup | Kind::Paragraph { .. }
            ) && node
                .children
                .iter()
                .all(|id| matches!(view.nodes[id].kind, Kind::OutlineGroup))
            {
                let groups = node.children.clone();
                let minimum = groups
                    .iter()
                    .map(|id| view.nodes[id].child_level.unwrap_or(1))
                    .min()
                    .unwrap();
                let level = node
                    .child_level
                    .unwrap_or(1)
                    .checked_add(minimum)
                    .filter(|level| *level <= 31)
                    .ok_or_else(|| {
                        invalid("Removing this group would exceed 31 child indentation levels")
                    })?;
                let mut children = Vec::new();
                for group in groups {
                    affected.insert(group);
                    let group_node = view.nodes.get_mut(&group).unwrap();
                    let relative = group_node.child_level.unwrap_or(1) - minimum;
                    if relative == 0 {
                        if group_node.extra[0]
                            .iter()
                            .any(|field| field.id == 0x08001d0c)
                        {
                            return Err(invalid("The redundant outline group cannot be deleted"));
                        }
                        children.extend_from_slice(&group_node.children);
                        changed_ids.remove(&group);
                    } else {
                        group_node.child_level = Some(relative);
                        children.push(group);
                        changed_ids.insert(group);
                    }
                }
                let node = view.nodes.get_mut(&at).unwrap();
                node.children = children;
                node.child_level = Some(level);
                changed_ids.insert(at);
                continue;
            }
            break;
        }

        let mut pending: Vec<_> = affected.into_iter().collect();
        let mut checked = BTreeSet::new();
        while let Some(id) = pending.pop() {
            if !checked.insert(id) {
                continue;
            }
            checked_path(&view, &parents, *page, id)?;
            let node = &view.nodes[&id];
            if matches!(self.placement, Placement::Delete)
                && node.extra[0].iter().any(|field| field.id == 0x08001d0c)
            {
                return Err(invalid(
                    "The selected subtree contains content that cannot be deleted",
                ));
            }
            pending.extend(
                node.children
                    .iter()
                    .chain(&node.content)
                    .chain(&node.structure)
                    .copied(),
            );
        }
        let raw = index.resolve(space, rid)?;
        let active_parents = editable_parents(&view, &pages, *page)?;
        let modified = current_timestamps()?.0.to_le_bytes();
        let mut changed = BTreeMap::new();
        let author_id = ExGuid {
            guid: self.guid,
            n: 3,
        };
        if let Some(cell) = empty_cell {
            let paragraph = ExGuid {
                guid: self.guid,
                n: 1,
            };
            let text = ExGuid {
                guid: self.guid,
                n: 2,
            };
            view.nodes.get_mut(&cell).unwrap().children.push(paragraph);
            for (id, jcid, values) in [
                (
                    paragraph,
                    0x6000d,
                    vec![
                        (0x24001c1f, 2_u32.to_le_bytes().to_vec()),
                        (0x0c001c03, vec![1]),
                        (0x14001d09, self.created.to_le_bytes().to_vec()),
                        (0x14001d7a, modified.to_vec()),
                        (0x20001d78, 3_u32.to_le_bytes().to_vec()),
                        (0x20001d79, 3_u32.to_le_bytes().to_vec()),
                    ],
                ),
                (
                    text,
                    0x6000e,
                    vec![(0x1c001c22, Vec::new()), (0x14001d7a, modified.to_vec())],
                ),
            ] {
                changed.insert(
                    id,
                    PropertyObject {
                        jcid,
                        bytes: properties(&values)?,
                        global_ids: Arc::new(BTreeMap::from([(0, self.guid)])),
                    },
                );
            }
        }
        let moved_paragraph = matches!(self.placement, Placement::Move { .. })
            && matches!(view.nodes[&self.object].kind, Kind::Paragraph { .. });
        if empty_cell.is_some() || moved_paragraph {
            changed.insert(
                author_id,
                PropertyObject {
                    jcid: 0x120001,
                    bytes: properties(&[(0x1c001d75, string(&self.author))])?,
                    global_ids: Arc::new(BTreeMap::from([(0, self.guid)])),
                },
            );
        }
        if let Some(author) = changed.get(&author_id)
            && raw.objects.get(&author_id).is_some_and(|old| {
                old.jcid == author.jcid && old.data == crate::ObjectData::Properties(&author.bytes)
            })
        {
            changed.remove(&author_id);
        }
        if changed.keys().any(|id| raw.objects.contains_key(id)) {
            return Err(invalid(
                "A tree-edit identity already exists; reconcile the existing edit",
            ));
        }
        let mut ancestors = BTreeSet::new();
        for id in &changed_ids {
            ancestors.extend(checked_path(&view, &active_parents, *page, *id)?);
        }
        for id in ancestors {
            let mut object = PropertyObject::from_object(&raw.objects[&id])?;
            if changed_ids.contains(&id) {
                let node = &view.nodes[&id];
                let mut references = Vec::new();
                for child in &node.children {
                    references.extend_from_slice(&object.reference(*child)?);
                }
                object.set(&[(0x24001c20, &references)])?;
                if let Some(level) = node.child_level {
                    object.set(&[(0x0c001c03, &[level])])?;
                }
            }
            object.set(&[(0x14001d7a, &modified)])?;
            changed.insert(id, object);
        }
        if moved_paragraph {
            let mut object = PropertyObject::from_object(&raw.objects[&self.object])?;
            let author = object.reference(author_id)?;
            object.set(&[(0x20001d79, &author), (0x14001d7a, &modified)])?;
            changed.insert(self.object, object);
        }
        update_title(&store, &raw, view, &pages, &mut changed)?;
        write_revision(source, space, |_| Ok(changed))
    }
}

fn checked_path(
    view: &Revision<'_>,
    parents: &BTreeMap<ExGuid, Vec<ExGuid>>,
    page: ExGuid,
    object: ExGuid,
) -> Result<Vec<ExGuid>, Error> {
    let mut path = Vec::new();
    let mut seen = BTreeSet::new();
    let mut id = object;
    loop {
        if !seen.insert(id) {
            return Err(invalid("Tree ancestry contains a cycle"));
        }
        let node = view
            .nodes
            .get(&id)
            .ok_or_else(|| invalid("The selected page content is unavailable"))?;
        if matches!(node.kind, Kind::Title)
            || node.extra[0]
                .iter()
                .any(|field| matches!(field.id, 0x88001cb4 | 0x88001cf9 | 0x88001cb2 | 0x88001cde))
        {
            return Err(invalid(
                "Title or protected content cannot be moved or deleted here",
            ));
        }
        path.push(id);
        if id == page {
            break;
        }
        let [parent] = parents.get(&id).map(Vec::as_slice).unwrap_or_default() else {
            return Err(invalid("Tree content must have one active parent"));
        };
        id = *parent;
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Insertion, PreparedEdit};

    #[test]
    fn a_retained_move_intent_can_reuse_its_immutable_author_after_another_move() {
        let source = crate::create_section("move.one", "First", "Author").unwrap();
        let store = Store::parse(&source).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let document = Document::parse(&index).unwrap();
        let (sid, page) = document.pages().unwrap()[0];
        let space = &document.spaces[&sid];
        let view = &space.revisions[&space.contexts[&ExGuid::default()]];
        let outline = *view.nodes[&page]
            .children
            .iter()
            .find(|id| matches!(view.nodes[id].kind, Kind::Outline { .. }))
            .unwrap();
        let first = view.nodes[&outline].children[0];
        let second = Insertion::paragraph(outline, None, "Second", "Author").unwrap();
        let source = PreparedEdit::insert(&source, sid, &second)
            .unwrap()
            .as_bytes()
            .to_vec();
        let retained = TreeEdit::move_to(first, outline, None, "Tree author").unwrap();
        let moved = PreparedEdit::tree(&source, sid, &retained).unwrap();
        let reversed =
            TreeEdit::move_to(first, outline, Some(second.object()), "Tree author").unwrap();
        let reversed = PreparedEdit::tree(moved.as_bytes(), sid, &reversed).unwrap();
        let repeated = PreparedEdit::tree(reversed.as_bytes(), sid, &retained).unwrap();
        let store = Store::parse(repeated.as_bytes()).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let document = Document::parse(&index).unwrap();
        let space = &document.spaces[&sid];
        let view = &space.revisions[&space.contexts[&ExGuid::default()]];
        assert_eq!(view.nodes[&outline].children, [second.object(), first]);
        assert_eq!(
            view.nodes[&first].latest_author,
            Some(ExGuid {
                guid: retained.guid,
                n: 3
            })
        );
    }

    #[test]
    fn group_normalization_preserves_unequal_indentation_and_overlapping_moves() {
        let source = crate::create_section("groups.one", "First", "Author").unwrap();
        let store = Store::parse(&source).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let document = Document::parse(&index).unwrap();
        let (sid, page) = document.pages().unwrap()[0];
        let space = &document.spaces[&sid];
        let view = &space.revisions[&space.contexts[&ExGuid::default()]];
        let outline = *view.nodes[&page]
            .children
            .iter()
            .find(|id| matches!(view.nodes[id].kind, Kind::Outline { .. }))
            .unwrap();
        let first = view.nodes[&outline].children[0];
        let second = Insertion::paragraph(outline, None, "Second", "Author").unwrap();
        let third = Insertion::paragraph(outline, None, "Third", "Author").unwrap();
        let source = PreparedEdit::insert(&source, sid, &second)
            .unwrap()
            .as_bytes()
            .to_vec();
        let source = PreparedEdit::insert(&source, sid, &third)
            .unwrap()
            .as_bytes()
            .to_vec();
        let guid = fresh_guid().unwrap();
        let a = ExGuid { guid, n: 1 };
        let b = ExGuid { guid, n: 2 };
        let source = write_revision(&source, sid, |raw| {
            let mut changed = BTreeMap::new();
            for (id, child, level) in [(a, first, 2), (b, second.object(), 1)] {
                let mut group = PropertyObject {
                    jcid: 0x60019,
                    bytes: properties(&[(0x0c001c03, vec![level])])?,
                    global_ids: Arc::new(BTreeMap::from([(0, guid)])),
                };
                let reference = group.reference(child)?;
                group.set(&[(0x24001c20, &reference)])?;
                changed.insert(id, group);
            }
            let mut object = PropertyObject::from_object(&raw.objects[&outline])?;
            let mut references = Vec::new();
            for id in [a, b, third.object()] {
                references.extend_from_slice(&object.reference(id)?);
            }
            object.set(&[(0x24001c20, &references)])?;
            changed.insert(outline, object);
            Ok(changed)
        })
        .unwrap();
        for moving in [false, true] {
            let intent = if moving {
                TreeEdit::move_to(third.object(), a, Some(first), "Author")
            } else {
                TreeEdit::delete(third.object(), "Author")
            }
            .unwrap();
            let edit = PreparedEdit::tree(&source, sid, &intent).unwrap();
            let store = Store::parse(edit.as_bytes()).unwrap();
            let index = RevisionIndex::parse(&store).unwrap();
            index.validate_current().unwrap();
            let document = Document::parse(&index).unwrap();
            let space = &document.spaces[&sid];
            let view = &space.revisions[&space.contexts[&ExGuid::default()]];
            assert_eq!(view.nodes[&outline].children, [a, second.object()]);
            assert_eq!(view.nodes[&outline].child_level, Some(2));
            assert_eq!(view.nodes[&a].child_level, Some(1));
            assert_eq!(
                view.nodes[&a].children,
                if moving {
                    vec![third.object(), first]
                } else {
                    vec![first]
                }
            );
            if !moving && let Some(output) = std::env::var_os("ONESTORE_TREE_OUTPUT") {
                let output = std::path::PathBuf::from(output).join("unequal-groups");
                assert!(output.is_absolute());
                std::fs::create_dir_all(output.parent().unwrap()).unwrap();
                std::fs::create_dir(&output).unwrap();
                std::fs::write(output.join("groups.one"), edit.as_bytes()).unwrap();
            }
        }
    }

    #[test]
    fn protected_descendants_and_ambiguous_parents_reject_before_publication() {
        let source = crate::create_section("protected.one", "Text", "Author").unwrap();
        let store = Store::parse(&source).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let document = Document::parse(&index).unwrap();
        let (sid, page) = document.pages().unwrap()[0];
        let space = &document.spaces[&sid];
        let view = &space.revisions[&space.contexts[&ExGuid::default()]];
        let outline = *view.nodes[&page]
            .children
            .iter()
            .find(|id| matches!(view.nodes[id].kind, Kind::Outline { .. }))
            .unwrap();
        let paragraph = view.nodes[&outline].children[0];
        let text = view.nodes[&paragraph].content[0];
        let deletion = TreeEdit::delete(paragraph, "Author").unwrap();
        for target in [page, outline, paragraph, text] {
            for property in [0x08001cde, 0x08001cb4, 0x08001cf9, 0x08001cb2] {
                for enabled in [false, true] {
                    let bytes = write_revision(&source, sid, |raw| {
                        let mut object = PropertyObject::from_object(&raw.objects[&target])?;
                        object.set(&[(property | (u32::from(enabled) << 31), &[])])?;
                        Ok(BTreeMap::from([(target, object)]))
                    })
                    .unwrap();
                    assert_eq!(
                        PreparedEdit::tree(&bytes, sid, &deletion).is_err(),
                        enabled,
                        "{target}: {property:#x}"
                    );
                }
            }
        }
        for protected in [outline, paragraph, text] {
            let bytes = write_revision(&source, sid, |raw| {
                let mut object = PropertyObject::from_object(&raw.objects[&protected])?;
                object.set(&[(0x08001d0c, &[])])?;
                Ok(BTreeMap::from([(protected, object)]))
            })
            .unwrap();
            assert!(PreparedEdit::tree(&bytes, sid, &deletion).is_err());
        }
        let bytes = write_revision(&source, sid, |raw| {
            let mut object = PropertyObject::from_object(&raw.objects[&outline])?;
            let reference = object.reference(paragraph)?;
            object.set(&[(0x24001c20, &reference.repeat(2))])?;
            Ok(BTreeMap::from([(outline, object)]))
        })
        .unwrap();
        assert!(PreparedEdit::tree(&bytes, sid, &deletion).is_err());
        let mut value = serde_json::to_value(&deletion).unwrap();
        value["guid"] = serde_json::to_value([0_u8; 16]).unwrap();
        let restored = serde_json::from_value(value).unwrap();
        assert!(PreparedEdit::tree(&source, sid, &restored).is_err());
    }
}
