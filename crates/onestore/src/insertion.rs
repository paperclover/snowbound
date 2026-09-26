use crate::{
    Error, ExGuid, Object, ObjectData,
    active::{ActivePage, Changes},
    create::{current_timestamps, default_text_style, properties, string},
    document::Kind,
    op::content::{NATIVE_INDENTS, measurement_bytes},
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
    Paragraph { before: Option<ExGuid> },
    Outline { x: f32, y: f32 },
}

/// A paragraph or outline insertion with its creation time; `changes_as` names the new
/// objects.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Insertion {
    guid: [u8; 16],
    parent: ExGuid,
    placement: Placement,
    text: String,
    author: String,
    created: u32,
}

impl Insertion {
    /// Inserts before a direct child, or appends when `before` is None.
    /// The parent must be an editable outline, paragraph, outline group or table cell.
    /// Carriage returns represent soft line breaks; line feeds and embedded-field markers are rejected.
    pub(crate) fn paragraph(
        parent: ExGuid,
        before: Option<ExGuid>,
        text: &str,
        author: &str,
    ) -> Result<Self, Error> {
        Self::new(parent, Placement::Paragraph { before }, text, author)
    }

    /// Adds an outline to an editable page at coordinates measured in points.
    pub(crate) fn outline(page: ExGuid, x: f32, y: f32, text: &str, author: &str) -> Result<Self, Error> {
        Self::new(page, Placement::Outline { x, y }, text, author)
    }

    fn new(parent: ExGuid, placement: Placement, text: &str, author: &str) -> Result<Self, Error> {
        let intent = Self {
            guid: fresh_guid()?,
            parent,
            placement,
            text: text.to_owned(),
            author: author.to_owned(),
            created: current_timestamps()?.0,
        };
        intent.validate()?;
        Ok(intent)
    }

    fn validate(&self) -> Result<(), Error> {
        if self.guid == [0; 16]
            || self.parent.guid == [0; 16]
            || self.text.contains(['\0', '\n', '\u{fffc}'])
            || self.author.contains('\0')
        {
            return Err(invalid(
                "Use an editable parent, ordinary paragraph text and a valid author",
            ));
        }
        if let Placement::Outline { x, y } = self.placement
            && (!x.is_finite() || !y.is_finite())
        {
            return Err(invalid("Outline coordinates must be finite"));
        }
        Ok(())
    }

    /// The objects creating the outline or paragraph `object`, the paragraph `paragraph` and
    /// its text `text` under those identities.
    pub(crate) fn changes_as(
        &self,
        active: &ActivePage<'_>,
        object: ExGuid,
        paragraph: ExGuid,
        text: ExGuid,
    ) -> Result<Changes, Error> {
        let [page] = active.pages.as_slice() else {
            return Err(invalid("Insertion requires a single active page"));
        };
        let view = &active.view;
        let parents = active.editable_parents(self.parent)?;
        let parent = &view.nodes[&self.parent];
        let position = match self.placement {
            Placement::Paragraph { before } => {
                if !matches!(
                    parent.kind,
                    Kind::Outline { .. }
                        | Kind::Paragraph { .. }
                        | Kind::OutlineGroup
                        | Kind::Cell { .. }
                ) {
                    return Err(invalid(
                        "Select an outline, paragraph, outline group or table cell",
                    ));
                }
                if let Some(id) = before {
                    parent
                        .children
                        .iter()
                        .position(|child| *child == id)
                        .ok_or_else(|| {
                            invalid("The insertion anchor is no longer a direct child")
                        })?
                } else {
                    parent.children.len()
                }
            }
            Placement::Outline { .. } => {
                if self.parent != *page || !matches!(parent.kind, Kind::Page { .. }) {
                    return Err(invalid("Select an active page for the new outline"));
                }
                parent.children.len()
            }
        };
        let mut ancestors = BTreeSet::new();
        let mut pending = vec![self.parent];
        while let Some(id) = pending.pop() {
            if !ancestors.insert(id) {
                continue;
            }
            if matches!(view.nodes[&id].kind, Kind::Title) {
                return Err(invalid(
                    "Title containers do not accept ordinary paragraphs",
                ));
            }
            pending.extend(parents.get(&id).into_iter().flatten().copied());
        }
        let modified = current_timestamps()?.0.to_le_bytes();
        let (author, default) = (
            ExGuid {
                guid: self.guid,
                n: 4,
            },
            ExGuid {
                guid: self.guid,
                n: 5,
            },
        );
        let mut table = BTreeMap::from([(0, self.guid)]);
        for id in [object, paragraph, text] {
            if !table.values().any(|guid| *guid == id.guid) {
                table.insert(table.len() as u32, id.guid);
            }
        }
        let table = Arc::new(table);
        let reference =
            |id: ExGuid| crate::write::compact(id, &table).map(|compact| compact.to_vec());
        let mut new = BTreeMap::new();
        for (id, jcid, values) in [
            (
                paragraph,
                0x6000d,
                vec![
                    (0x14001d7a, modified.to_vec()),
                    (0x14001d09, self.created.to_le_bytes().to_vec()),
                    (0x0c001c03, vec![1]),
                    (0x24001c1f, reference(text)?),
                    (0x20001d78, reference(author)?),
                    (0x20001d79, reference(author)?),
                ],
            ),
            (
                text,
                0x6000e,
                vec![
                    (0x14001d7a, modified.to_vec()),
                    (0x1c001c22, string(&self.text)),
                    (0x24001e13, reference(default)?),
                    (0x10001cfe, 0x409_u16.to_le_bytes().to_vec()),
                ],
            ),
            (author, 0x120001, crate::create::author_properties(&self.author)),
            (default, 0x12004d, default_text_style()),
        ] {
            new.insert(
                id,
                PropertyObject {
                    jcid,
                    bytes: properties(&values)?,
                    global_ids: Arc::clone(&table),
                },
            );
        }
        if let Placement::Outline { x, y } = self.placement {
            let children = reference(paragraph)?;
            new.insert(
                object,
                PropertyObject {
                    jcid: 0x6000c,
                    global_ids: table,
                    bytes: properties(&[
                        (0x14001d7a, modified.to_vec()),
                        (0x24001c20, children),
                        (0x0c001c03, vec![1]),
                        (0x1c001c12, measurement_bytes(&NATIVE_INDENTS, 4)?),
                        (0x14001c14, (x / 36.0).to_le_bytes().to_vec()),
                        (0x14001c15, (y / 36.0).to_le_bytes().to_vec()),
                        (0x14001c1b, 13_f32.to_le_bytes().to_vec()),
                        (0x14001c1c, 0.6_f32.to_le_bytes().to_vec()),
                    ])?,
                },
            );
        }
        let raw = &active.live.revision;
        // Drop the overlay before moving the property bytes it borrows.
        let title = {
            let mut overlay = BTreeMap::new();
            let mut parent = view.nodes[&self.parent].clone();
            parent.children.insert(position, object);
            overlay.insert(self.parent, parent);
            for (id, object) in &new {
                overlay.insert(
                    *id,
                    active.element(&Object {
                        jcid: object.jcid,
                        reference_count: 0,
                        data: ObjectData::Properties(&object.bytes),
                        global_ids: Arc::clone(&object.global_ids),
                    })?,
                );
            }
            active.title(&overlay, None)?
        };
        let mut changed = new;
        for id in &ancestors {
            let mut object = PropertyObject::from_object(&raw.objects[id])?;
            object.set(&[(0x14001d7a, &modified)])?;
            changed.insert(*id, object);
        }
        let parent = changed.get_mut(&self.parent).unwrap();
        let properties = crate::PropertySets::parse(&parent.bytes)?;
        let existing = properties.sets[0].iter().find(|p| p.id == 0x24001c20);
        let mut ids = match existing.map(|p| &p.value) {
            Some(crate::Value::References { compact_ids, .. }) => compact_ids.to_vec(),
            None => Vec::new(),
            _ => return Err(invalid("The parent has an invalid child list")),
        };
        let child = parent.reference(object)?;
        if position > ids.len() / 4 {
            return Err(invalid("The parent has an invalid child list"));
        }
        ids.splice(position * 4..position * 4, child);
        parent.set(&[(0x24001c20, &ids)])?;
        if matches!(self.placement, Placement::Paragraph { .. }) {
            let properties = crate::PropertySets::parse(&parent.bytes)?;
            if !properties.sets[0].iter().any(|p| p.id == 0x0c001c03) {
                parent.set(&[(0x0c001c03, &[1])])?;
            }
        }
        if let Some((page, automatic, title)) = title {
            let metadata = raw
                .roots
                .get(&2)
                .ok_or_else(|| invalid("Page title metadata is unavailable"))?;
            if ![0x20030, 0x20038].contains(&raw.objects[metadata].jcid) {
                return Err(invalid("Page title metadata is unavailable"));
            }
            let title = string(&title);
            let mut metadata_object = PropertyObject::from_object(&raw.objects[metadata])?;
            metadata_object.set(&[(0x1c001cf3, &title)])?;
            changed.insert(*metadata, metadata_object);
            changed
                .get_mut(&page)
                .unwrap()
                .set(&[(0x1c001d3c, if automatic { &title } else { &[0, 0] })])?;
        }
        Ok(changed)
    }
}
