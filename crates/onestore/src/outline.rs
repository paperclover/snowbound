use crate::{
    Error, ExGuid, RevisionIndex, Store,
    create::current_timestamps,
    document::{Document, Kind},
    edit::{editable_parents, update_title},
    write::{PropertyObject, write_revision_on},
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// A layout or saved expansion change that preserves content and object identities.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum OutlineEdit {
    /// Moves an ordinary page outline to coordinates measured in points.
    /// Refreshes automatic page titles when the leading outline changes.
    Position { x: f32, y: f32 },
    /// Changes an ordinary outline's maximum width in points, at least 36.
    /// `user_set` distinguishes an explicit width from an automatic layout hint.
    /// Height remains content-derived; any previous reserved wrapping width is cleared.
    Width { points: f32, user_set: bool },
    /// Sets a paragraph's saved expansion default; native cache-local UI state can override it.
    Collapsed(bool),
}

impl OutlineEdit {
    pub(crate) fn apply(
        self,
        source: &[u8],
        space: ExGuid,
        object: ExGuid,
    ) -> Result<Vec<u8>, Error> {
        let invalid = |message| Error { offset: 0, message };
        let values = match self {
            Self::Position { x, y } => {
                if !x.is_finite() || !y.is_finite() {
                    return Err(invalid("Outline coordinates must be finite"));
                }
                vec![
                    (0x14001c14, (x / 36.0).to_le_bytes().to_vec()),
                    (0x14001c15, (y / 36.0).to_le_bytes().to_vec()),
                ]
            }
            Self::Width { points, user_set } => {
                if !points.is_finite() || points < 36.0 {
                    return Err(invalid(
                        "Outline width must be finite and at least 36 points",
                    ));
                }
                vec![
                    (0x14001c1b, (points / 36.0).to_le_bytes().to_vec()),
                    (0x08001cbd | (u32::from(user_set) << 31), Vec::new()),
                ]
            }
            Self::Collapsed(value) => vec![(0x0c001c11, vec![u8::from(value)])],
        };
        let store = Store::parse(source)?;
        let index = RevisionIndex::parse(&store)?;
        index.validate_current()?;
        let mut document = Document::parse(&index)?;
        let pages = document.pages_in(space)?;
        let [page] = pages.as_slice() else {
            return Err(invalid("Outline editing requires a single active page"));
        };
        let view = document
            .spaces
            .remove(&space)
            .and_then(crate::document::Space::into_active)
            .unwrap();
        let parents = editable_parents(&view, &pages, object)?;
        let node = &view.nodes[&object];
        match self {
            Self::Collapsed(_) => {
                if !matches!(node.kind, Kind::Paragraph { .. }) {
                    return Err(invalid("Select a paragraph for its saved expansion state"));
                }
            }
            _ => {
                if !matches!(node.kind, Kind::Outline { .. })
                    || parents.get(&object).map(Vec::as_slice) != Some(pages.as_slice())
                    || !view.nodes[page].children.contains(&object)
                {
                    return Err(invalid("Select an ordinary outline directly on the page"));
                }
            }
        }
        let mut ancestors = BTreeSet::new();
        let mut pending = vec![object];
        while let Some(id) = pending.pop() {
            if !ancestors.insert(id) {
                return Err(invalid("Outline ancestry contains a cycle"));
            }
            if id != *page && parents.get(&id).is_none_or(|parents| parents.len() != 1) {
                return Err(invalid("Outline content must have one active parent"));
            }
            if matches!(view.nodes[&id].kind, Kind::Title)
                || view.nodes[&id].extra[0]
                    .iter()
                    .any(|field| matches!(field.id, 0x88001cb4 | 0x88001cf9 | 0x88001cb2))
            {
                return Err(invalid(
                    "Title or protected outline content cannot be changed here",
                ));
            }
            pending.extend(parents.get(&id).into_iter().flatten().copied());
        }
        let modified = current_timestamps()?.0.to_le_bytes();
        write_revision_on(&index, space, |raw| {
            let mut target = PropertyObject::from_object(&raw.objects[&object])?;
            target.set(
                &values
                    .iter()
                    .map(|(id, data)| (*id, data.as_slice()))
                    .collect::<Vec<_>>(),
            )?;
            if matches!(self, Self::Width { .. }) {
                target.remove(&[0x14001cdb])?;
            }
            if raw.objects[&object].data == crate::ObjectData::Properties(&target.bytes) {
                return Ok(BTreeMap::new());
            }
            target.set(&[(0x14001d7a, &modified)])?;
            let mut changed = BTreeMap::from([(object, target)]);
            for id in ancestors {
                if id != object {
                    let mut ancestor = PropertyObject::from_object(&raw.objects[&id])?;
                    ancestor.set(&[(0x14001d7a, &modified)])?;
                    changed.insert(id, ancestor);
                }
            }
            if matches!(self, Self::Position { .. }) {
                update_title(&store, raw, view, &pages, &mut changed)?;
            }
            Ok(changed)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::write::write_revision;

    #[test]
    fn protection_on_the_target_or_ancestor_prevents_layout_edits() {
        let source = crate::create_section("layout.one", "Text", "Author").unwrap();
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
        for target in [outline, page] {
            for property in [0x08001cde, 0x08001cb4, 0x08001cf9, 0x08001cb2] {
                for enabled in [false, true] {
                    let bytes = write_revision(&source, sid, |raw| {
                        let mut object = PropertyObject::from_object(&raw.objects[&target])?;
                        object.set(&[(property | (u32::from(enabled) << 31), &[])])?;
                        Ok(BTreeMap::from([(target, object)]))
                    })
                    .unwrap();
                    for (object, edit) in [
                        (outline, OutlineEdit::Position { x: 72.0, y: 72.0 }),
                        (
                            outline,
                            OutlineEdit::Width {
                                points: 144.0,
                                user_set: true,
                            },
                        ),
                        (paragraph, OutlineEdit::Collapsed(true)),
                    ] {
                        assert_eq!(
                            edit.apply(&bytes, sid, object).is_err(),
                            enabled,
                            "{target} {property:x} {edit:?}"
                        );
                    }
                }
            }
        }
        let duplicated = write_revision(&source, sid, |raw| {
            let mut object = PropertyObject::from_object(&raw.objects[&outline])?;
            let reference = object.reference(paragraph)?;
            object.set(&[(0x24001c20, &reference.repeat(2))])?;
            Ok(BTreeMap::from([(outline, object)]))
        })
        .unwrap();
        assert!(
            OutlineEdit::Collapsed(true)
                .apply(&duplicated, sid, paragraph)
                .is_err()
        );
    }
}
