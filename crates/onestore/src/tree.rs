use crate::{
    Error, ExGuid,
    active::{ActivePage, Changes},
    create::{current_timestamps, properties},
    document::{Kind, Revision},
    edit::{editable_parents, update_title},
    write::{PropertyObject, fresh_guid},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

fn invalid(message: &'static str) -> Error {
    Error { offset: 0, message }
}

#[derive(Debug, Clone, PartialEq)]
enum Placement {
    Delete,
    Move {
        parent: ExGuid,
        before: Option<ExGuid>,
    },
}

/// An atomic subtree move or deletion on one active page.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TreeEdit {
    guid: [u8; 16],
    object: ExGuid,
    placement: Placement,
    author: String,
}

impl TreeEdit {
    /// Removes a paragraph or ordinary outline and its descendants from the active tree.
    /// Historical objects remain available.
    pub(crate) fn delete(object: ExGuid, author: &str) -> Result<Self, Error> {
        Self::new(object, Placement::Delete, author)
    }

    /// Moves before a direct child, or appends when `before` is None.
    /// Paragraph destinations are outlines, groups, paragraphs or cells on the same page.
    /// Outlines remain direct page children. Content and explicit list formatting are preserved.
    pub(crate) fn move_to(
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

    pub(crate) fn changes(&self, active: &ActivePage<'_>) -> Result<Changes, Error> {
        let pages = &active.pages;
        let [page] = pages.as_slice() else {
            return Err(invalid("Tree editing requires a single active page"));
        };
        let mut view = active.view.clone();
        let parents = active.editable_parents(self.object)?;
        let path = checked_path(&view, parents, *page, self.object)?;
        let source_parent = *path
            .get(1)
            .ok_or_else(|| invalid("Select a paragraph or ordinary page outline"))?;
        let compatible = |parent: ExGuid| match view.nodes[&self.object].kind {
            Kind::Outline { .. } | Kind::Image { .. } | Kind::Ink { .. } => parent == *page,
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
            if checked_path(&view, parents, *page, parent)?.contains(&self.object) {
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
                return Ok(Changes::new());
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
                return Ok(Changes::new());
            }
            changed_ids.insert(parent);
        } else {
            view.nodes
                .get_mut(&source_parent)
                .unwrap()
                .children
                .retain(|id| *id != self.object);
        }

        let mut at = source_parent;
        loop {
            let node = &view.nodes[&at];
            if matches!(node.kind, Kind::Page { .. } | Kind::Paragraph { .. })
                && node.children.is_empty()
            {
                break;
            }
            if node.children.is_empty() {
                // `Section::apply` refuses an edit that ends with the cell still empty.
                if matches!(node.kind, Kind::Cell { .. }) {
                    break;
                }
                if !matches!(node.kind, Kind::Outline { .. } | Kind::OutlineGroup) {
                    return Err(invalid("This container cannot be emptied by a tree edit"));
                }
                if node.extra[0].iter().any(|field| field.id == 0x08001d0c) {
                    return Err(invalid("The emptied container cannot be deleted"));
                }
                let path = checked_path(&view, parents, *page, at)?;
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
            checked_path(&view, parents, *page, id)?;
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
        let raw = &active.live.revision;
        let active_parents = editable_parents(&view, pages, *page)?;
        let modified = current_timestamps()?.0.to_le_bytes();
        let mut changed = BTreeMap::new();
        let author_id = ExGuid {
            guid: self.guid,
            n: 3,
        };
        let moved_paragraph = matches!(self.placement, Placement::Move { .. })
            && matches!(view.nodes[&self.object].kind, Kind::Paragraph { .. });
        if moved_paragraph {
            changed.insert(
                author_id,
                PropertyObject {
                    jcid: 0x120001,
                    bytes: properties(&crate::create::author_properties(&self.author))?,
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
        update_title(active, view, &mut changed)?;
        Ok(changed)
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
    use crate::{
        RevisionIndex, Store,
        document::{Document, Format},
        op::{
            PageOp,
            tests::{edited, text_paragraph},
        },
        write::write_revision,
    };

    #[test]
    fn group_normalization_preserves_unequal_indentation_and_overlapping_moves() {
        let source = crate::create_section("groups.one", "First", "Author").unwrap();
        let store = Store::parse(&source).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let document = Document::parse(&index).unwrap();
        let (sid, page) = document.pages().unwrap()[0];
        let view = document.active(sid).unwrap();
        let outline = *view.nodes[&page]
            .children
            .iter()
            .find(|id| matches!(view.nodes[id].kind, Kind::Outline { .. }))
            .unwrap();
        let first = view.nodes[&outline].children[0];
        let paragraphs = vec![
            text_paragraph("Second", Format::default()),
            text_paragraph("Third", Format::default()),
        ];
        let (second, third) = (paragraphs[0].id, paragraphs[1].id);
        let insert = PageOp::Insert {
            container: outline,
            before: None,
            paragraphs,
        };
        let source = edited(&source, sid, vec![insert]).unwrap();
        let guid = fresh_guid().unwrap();
        let a = ExGuid { guid, n: 1 };
        let b = ExGuid { guid, n: 2 };
        let source = write_revision(&source, sid, |raw| {
            let mut changed = BTreeMap::new();
            for (id, child, level) in [(a, first, 2), (b, second, 1)] {
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
            for id in [a, b, third] {
                references.extend_from_slice(&object.reference(id)?);
            }
            object.set(&[(0x24001c20, &references)])?;
            changed.insert(outline, object);
            Ok(changed)
        })
        .unwrap();
        let page = |image: &[u8]| {
            let store = Store::parse(image).unwrap();
            let index = RevisionIndex::parse(&store).unwrap();
            index.validate_current().unwrap();
            crate::page::Page::from_space(&Document::parse(&index).unwrap(), sid).unwrap()
        };
        let written = edited(&source, sid, vec![PageOp::Delete { object: third }]).unwrap();
        let store = Store::parse(&written).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let document = Document::parse(&index).unwrap();
        let view = document.active(sid).unwrap();
        assert_eq!(view.nodes[&outline].children, [a, second]);
        assert_eq!(view.nodes[&outline].child_level, Some(2));
        assert_eq!(view.nodes[&a].child_level, Some(1));
        assert_eq!(view.nodes[&a].children, [first]);
        if let Some(output) = std::env::var_os("ONESTORE_TREE_OUTPUT") {
            let output = std::path::PathBuf::from(output).join("unequal-groups");
            assert!(output.is_absolute());
            std::fs::create_dir_all(output.parent().unwrap()).unwrap();
            std::fs::create_dir(&output).unwrap();
            std::fs::write(output.join("groups.one"), &written).unwrap();
        }
        // A move keeps the paragraph's level; the writers regroup the outline around it.
        let moved = PageOp::Move {
            object: third,
            parent: Some(outline),
            before: Some(first),
        };
        let mut predicted = page(&source);
        crate::op::predict(&mut predicted, &moved).unwrap();
        assert_eq!(page(&edited(&source, sid, vec![moved]).unwrap()), predicted);
    }

    #[test]
    fn protected_descendants_and_ambiguous_parents_reject_before_publication() {
        let source = crate::create_section("protected.one", "Text", "Author").unwrap();
        let store = Store::parse(&source).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let document = Document::parse(&index).unwrap();
        let (sid, page) = document.pages().unwrap()[0];
        let view = document.active(sid).unwrap();
        let outline = *view.nodes[&page]
            .children
            .iter()
            .find(|id| matches!(view.nodes[id].kind, Kind::Outline { .. }))
            .unwrap();
        let paragraph = view.nodes[&outline].children[0];
        let text = view.nodes[&paragraph].content[0];
        let deleted =
            |bytes: &[u8]| edited(bytes, sid, vec![PageOp::Delete { object: paragraph }]).is_ok();
        for target in [page, outline, paragraph, text] {
            for property in [0x08001cde, 0x08001cb4, 0x08001cf9, 0x08001cb2] {
                for enabled in [false, true] {
                    let bytes = write_revision(&source, sid, |raw| {
                        let mut object = PropertyObject::from_object(&raw.objects[&target])?;
                        object.set(&[(property | (u32::from(enabled) << 31), &[])])?;
                        Ok(BTreeMap::from([(target, object)]))
                    })
                    .unwrap();
                    assert_eq!(deleted(&bytes), !enabled, "{target}: {property:#x}");
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
            assert!(!deleted(&bytes));
        }
        let bytes = write_revision(&source, sid, |raw| {
            let mut object = PropertyObject::from_object(&raw.objects[&outline])?;
            let reference = object.reference(paragraph)?;
            object.set(&[(0x24001c20, &reference.repeat(2))])?;
            Ok(BTreeMap::from([(outline, object)]))
        })
        .unwrap();
        assert!(!deleted(&bytes));
    }
}
