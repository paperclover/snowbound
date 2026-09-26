use crate::{
    Error, ExGuid, Object, ObjectData,
    active::{ActivePage, Changes},
    create::{current_timestamps, default_text_style, properties, string},
    document::Kind,
    write::{PropertyObject, fresh_guid},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Range,
    sync::Arc,
};

fn invalid(message: &'static str) -> Error {
    Error { offset: 0, message }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
enum Placement {
    Paragraph { before: Option<ExGuid> },
    Outline { x: f32, y: f32 },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FormatSpan {
    guid: [u8; 16],
    range: Range<u32>,
    attributes: Vec<crate::TextAttribute>,
}

/// A paragraph or outline insertion with stable object identities and creation time.
/// Retain this intent across rebases; constructing another intent allocates different identities.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Insertion {
    guid: [u8; 16],
    parent: ExGuid,
    placement: Placement,
    text: String,
    author: String,
    created: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    formats: Vec<FormatSpan>,
}

impl Insertion {
    /// Adds a nonoverlapping UTF-16 formatting span, retaining all existing identities.
    /// Gaps use the ordinary insertion style; only empty text accepts a zero-length span.
    pub fn with_formatting(
        &self,
        range: Range<u32>,
        attributes: &[crate::TextAttribute],
    ) -> Result<Self, Error> {
        let mut intent = self.clone();
        intent.formats.push(FormatSpan {
            guid: fresh_guid()?,
            range,
            attributes: attributes.to_vec(),
        });
        intent.formats.sort_by_key(|span| span.range.start);
        intent.validate()?;
        Ok(intent)
    }

    /// Inserts before a direct child, or appends when `before` is None.
    /// The parent must be an editable outline, paragraph, outline group or table cell.
    /// Carriage returns represent soft line breaks; line feeds and embedded-field markers are rejected.
    pub fn paragraph(
        parent: ExGuid,
        before: Option<ExGuid>,
        text: &str,
        author: &str,
    ) -> Result<Self, Error> {
        Self::new(parent, Placement::Paragraph { before }, text, author)
    }

    /// Adds an outline to an editable page at coordinates measured in points.
    pub fn outline(page: ExGuid, x: f32, y: f32, text: &str, author: &str) -> Result<Self, Error> {
        Self::new(page, Placement::Outline { x, y }, text, author)
    }

    /// Changes a paragraph intent's placement while retaining its identities, text and author.
    pub fn reposition_paragraph(
        &self,
        parent: ExGuid,
        before: Option<ExGuid>,
    ) -> Result<Self, Error> {
        if !matches!(self.placement, Placement::Paragraph { .. }) {
            return Err(invalid("An outline intent cannot become a paragraph"));
        }
        let mut intent = self.clone();
        intent.parent = parent;
        intent.placement = Placement::Paragraph { before };
        intent.validate()?;
        Ok(intent)
    }

    /// Changes an outline intent's placement while retaining its identities, text and author.
    pub fn reposition_outline(&self, page: ExGuid, x: f32, y: f32) -> Result<Self, Error> {
        if !matches!(self.placement, Placement::Outline { .. }) {
            return Err(invalid("A paragraph intent cannot become an outline"));
        }
        let mut intent = self.clone();
        intent.parent = page;
        intent.placement = Placement::Outline { x, y };
        intent.validate()?;
        Ok(intent)
    }

    fn new(parent: ExGuid, placement: Placement, text: &str, author: &str) -> Result<Self, Error> {
        let intent = Self {
            guid: fresh_guid()?,
            parent,
            placement,
            text: text.to_owned(),
            author: author.to_owned(),
            created: current_timestamps()?.0,
            formats: Vec::new(),
        };
        intent.validate()?;
        Ok(intent)
    }

    /// Identity of the new paragraph, or the new outline for an outline insertion.
    pub fn object(&self) -> ExGuid {
        ExGuid {
            guid: self.guid,
            n: 1,
        }
    }

    /// Identity of the insertion's ordinary rich-text object.
    pub fn text_object(&self) -> ExGuid {
        ExGuid {
            guid: self.guid,
            n: 2,
        }
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
        if !self.formats.is_empty() {
            let units: Vec<_> = self.text.encode_utf16().collect();
            let length = u32::try_from(units.len())
                .map_err(|_| invalid("Text exceeds UTF-16 offset range"))?;
            let mut identities = BTreeSet::from([self.guid]);
            let mut end = 0;
            for span in &self.formats {
                if span.guid == [0; 16] || !identities.insert(span.guid) {
                    return Err(invalid("Formatting identities must be distinct"));
                }
                if span.range.start > span.range.end
                    || span.range.end > length
                    || span.range.start < end
                    || (span.range.is_empty() && (!units.is_empty() || self.formats.len() != 1))
                {
                    return Err(invalid(
                        "Formatting spans must be nonoverlapping ranges within the new text",
                    ));
                }
                for boundary in [span.range.start, span.range.end] {
                    if boundary > 0
                        && boundary < length
                        && (0xd800..=0xdbff).contains(&units[boundary as usize - 1])
                    {
                        return Err(invalid("Formatting boundary splits a surrogate pair"));
                    }
                }
                crate::formatting::attribute_values(&span.attributes)?;
                end = span.range.end;
            }
        }
        Ok(())
    }

    pub(crate) fn apply(&self, source: &[u8], space: ExGuid) -> Result<Vec<u8>, Error> {
        self.validate()?;
        crate::active::write(source, space, |active| self.changes(active))
    }

    pub(crate) fn changes(&self, active: &ActivePage<'_>) -> Result<Changes, Error> {
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
        let paragraph_n = if matches!(self.placement, Placement::Outline { .. }) {
            3
        } else {
            1
        };
        let table = Arc::new(BTreeMap::from([(0, self.guid)]));
        let reference = |n: u32| n.to_le_bytes().to_vec();
        let mut new = BTreeMap::new();
        for (n, jcid, values) in [
            (
                paragraph_n,
                0x6000d,
                vec![
                    (0x14001d7a, modified.to_vec()),
                    (0x14001d09, self.created.to_le_bytes().to_vec()),
                    (0x0c001c03, vec![1]),
                    (0x24001c1f, reference(2)),
                    (0x20001d78, reference(4)),
                    (0x20001d79, reference(4)),
                ],
            ),
            (
                2,
                0x6000e,
                vec![
                    (0x14001d7a, modified.to_vec()),
                    (0x1c001c22, string(&self.text)),
                    (0x24001e13, reference(5)),
                    (0x10001cfe, 0x409_u16.to_le_bytes().to_vec()),
                ],
            ),
            (4, 0x120001, vec![(0x1c001d75, string(&self.author))]),
            (5, 0x12004d, default_text_style()),
        ] {
            new.insert(
                ExGuid { guid: self.guid, n },
                PropertyObject {
                    jcid,
                    bytes: properties(&values)?,
                    global_ids: Arc::clone(&table),
                },
            );
        }
        if !self.formats.is_empty() {
            let default = ExGuid {
                guid: self.guid,
                n: 5,
            };
            let mut segments = Vec::new();
            let mut end = 0;
            for span in &self.formats {
                if end < span.range.start {
                    segments.push((span.range.start, default));
                }
                let id = ExGuid {
                    guid: span.guid,
                    n: 1,
                };
                let mut style = PropertyObject {
                    jcid: 0x12004d,
                    bytes: properties(&default_text_style())?,
                    global_ids: Arc::new(BTreeMap::from([(0, span.guid)])),
                };
                let values = crate::formatting::attribute_values(&span.attributes)?;
                style.set(
                    &values
                        .iter()
                        .map(|(id, value)| (*id, value.as_slice()))
                        .collect::<Vec<_>>(),
                )?;
                new.insert(id, style);
                segments.push((span.range.end, id));
                end = span.range.end;
            }
            let length = u32::try_from(self.text.encode_utf16().count())
                .map_err(|_| invalid("Text exceeds UTF-16 offset range"))?;
            if end < length {
                segments.push((length, default));
            }
            if !segments.iter().any(|(_, id)| *id == default) {
                new.remove(&default);
            }
            let target = new.get_mut(&self.text_object()).unwrap();
            let mut references = Vec::new();
            let mut ends = Vec::new();
            for (end, id) in segments {
                references.extend_from_slice(&target.reference(id)?);
                ends.extend_from_slice(&end.to_le_bytes());
            }
            ends.truncate(ends.len() - 4);
            target.set(&[(0x24001e13, &references), (0x1c001e12, &ends)])?;
        }
        if let Placement::Outline { x, y } = self.placement {
            new.insert(
                self.object(),
                PropertyObject {
                    jcid: 0x6000c,
                    global_ids: table,
                    bytes: properties(&[
                        (0x14001d7a, modified.to_vec()),
                        (0x24001c20, reference(3)),
                        (0x0c001c03, vec![1]),
                        (0x1c001c12, vec![1, 0, 0, 0, 0, 0, 0, 0]),
                        (0x14001c14, (x / 36.0).to_le_bytes().to_vec()),
                        (0x14001c15, (y / 36.0).to_le_bytes().to_vec()),
                        (0x14001c1b, 13_f32.to_le_bytes().to_vec()),
                        (0x14001c1c, 0.6_f32.to_le_bytes().to_vec()),
                    ])?,
                },
            );
        }
        let raw = &active.live.revision;
        if new.keys().any(|id| raw.objects.contains_key(id)) {
            return Err(invalid(
                "An insertion identity is already present; reconcile the existing edit",
            ));
        }
        // Drop the overlay before moving the property bytes it borrows.
        let title = {
            let mut overlay = BTreeMap::new();
            let mut parent = view.nodes[&self.parent].clone();
            parent.children.insert(position, self.object());
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
        let child = parent.reference(self.object())?;
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
            if raw.objects[metadata].jcid != 0x20030 {
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
