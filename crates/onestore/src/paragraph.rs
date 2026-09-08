use crate::{
    Error, ExGuid, Object, ObjectData, PropertySets, RevisionIndex, Store,
    create::{current_timestamps, properties, string},
    document::{Document, Element, Kind},
    edit::{editable_parents, page_title},
    write::{PropertyObject, fresh_guid, write_revision},
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

/// Splits ordinary paragraph text while retaining the new objects' identities across retries.
/// The original paragraph/text remain on the left; nested children move to the right.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParagraphSplit {
    guid: [u8; 16],
    text: ExGuid,
    offset: u32,
    author: String,
    created: u32,
}

impl ParagraphSplit {
    /// The offset is measured in UTF-16 code units and must lie between Unicode scalars.
    pub fn new(text: ExGuid, offset: u32, author: &str) -> Result<Self, Error> {
        if text.guid == [0; 16] || author.contains('\0') {
            return Err(invalid(
                "Choose paragraph text and an author name without NUL",
            ));
        }
        Ok(Self {
            guid: fresh_guid()?,
            text,
            offset,
            author: author.to_owned(),
            created: current_timestamps()?.0,
        })
    }

    /// Identity of the new right paragraph.
    pub fn object(&self) -> ExGuid {
        ExGuid {
            guid: self.guid,
            n: 1,
        }
    }

    /// Identity of the new right paragraph's text.
    pub fn text_object(&self) -> ExGuid {
        ExGuid {
            guid: self.guid,
            n: 2,
        }
    }

    pub(crate) fn apply(&self, source: &[u8], space: ExGuid) -> Result<Vec<u8>, Error> {
        if self.guid == [0; 16] || self.text.guid == [0; 16] || self.author.contains('\0') {
            return Err(invalid(
                "Choose paragraph text and an author name without NUL",
            ));
        }
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
            return Err(invalid("Splitting requires a single active page"));
        };
        let semantic = document
            .spaces
            .remove(&space)
            .ok_or_else(|| invalid("The active page is unavailable"))?;
        let rid = semantic.contexts[&ExGuid::default()];
        let mut view = semantic
            .revisions
            .into_iter()
            .find_map(|(id, view)| (id == rid).then_some(view))
            .unwrap();
        let parents = editable_parents(&view, &pages, self.text)?;
        let [paragraph] = parents
            .get(&self.text)
            .map(Vec::as_slice)
            .unwrap_or_default()
        else {
            return Err(invalid("Select text belonging to one paragraph"));
        };
        let [parent] = parents
            .get(paragraph)
            .map(Vec::as_slice)
            .unwrap_or_default()
        else {
            return Err(invalid("Select a paragraph with one parent"));
        };
        let left = &view.nodes[paragraph];
        let Kind::Paragraph { lists, .. } = &left.kind else {
            return Err(invalid("Select ordinary paragraph text"));
        };
        if left.content != [self.text]
            || !matches!(
                view.nodes[parent].kind,
                Kind::Outline { .. }
                    | Kind::OutlineGroup
                    | Kind::Paragraph { .. }
                    | Kind::Cell { .. }
            )
        {
            return Err(invalid(
                "Select ordinary paragraph text inside an outline or table cell",
            ));
        }
        let mut ancestors = BTreeSet::new();
        let mut pending = vec![*paragraph];
        while let Some(id) = pending.pop() {
            if !ancestors.insert(id) {
                continue;
            }
            if matches!(view.nodes[&id].kind, Kind::Title) {
                return Err(invalid(
                    "Title containers cannot be split into ordinary paragraphs",
                ));
            }
            pending.extend(parents.get(&id).into_iter().flatten().copied());
        }
        let node = &view.nodes[&self.text];
        let Kind::RichText {
            text,
            runs,
            boilerplate,
            ..
        } = &node.kind
        else {
            return Err(invalid("Select ordinary paragraph text"));
        };
        if *boilerplate || !node.media_ids.is_empty() || node.media_time_ms.is_some() {
            return Err(invalid(
                "Generated or recording-linked text cannot be split",
            ));
        }
        for run in view.text_runs(self.text)? {
            if [
                run.format.hidden,
                run.format.hyperlink,
                run.format.math,
                run.format.embedded_object,
            ]
            .contains(&Some(true))
                || run.text.contains(['\u{fffc}', '\u{fddf}'])
            {
                return Err(invalid(
                    "This paragraph contains a field or embedded object that cannot be split",
                ));
            }
        }
        let raw = index.resolve(space, rid)?;
        let ObjectData::Properties(blob) = raw.objects[&self.text].data else {
            unreachable!()
        };
        if PropertySets::parse(blob)?.sets[0]
            .iter()
            .any(|p| matches!(p.id, 0x40003499 | 0x24003458))
        {
            return Err(invalid(
                "This paragraph contains run metadata that cannot be split",
            ));
        }
        let length = u32::try_from(text.encode_utf16().count())
            .map_err(|_| invalid("Paragraph exceeds the UTF-16 offset range"))?;
        if self.offset > length || lists.len() > 251 {
            return Err(invalid(
                "Choose a position within the paragraph and at most 251 list levels",
            ));
        }
        let author_id = ExGuid {
            guid: self.guid,
            n: 3,
        };
        let normal_id = ExGuid {
            guid: self.guid,
            n: 4,
        };
        let typing = runs.last().and_then(|run| run.format).unwrap_or(normal_id);
        let modified = current_timestamps()?.0.to_le_bytes();
        let mut changed = BTreeMap::new();
        if (self.offset == 0 && !text.is_empty()) || runs.iter().any(|run| run.format.is_none()) {
            changed.insert(
                normal_id,
                PropertyObject {
                    jcid: 0x12004d,
                    bytes: properties(&[])?,
                    global_ids: Arc::new(BTreeMap::from([(0, self.guid)])),
                },
            );
        }
        let mut prefix = fragment(
            &raw.objects[&self.text],
            node,
            0..self.offset,
            if text.is_empty() { typing } else { normal_id },
        )?;
        let mut suffix = fragment(&raw.objects[&self.text], node, self.offset..length, typing)?;
        prefix.set(&[(0x14001d7a, &modified)])?;
        suffix.remove(&[0x40003489])?;
        suffix.set(&[(0x14001d7a, &modified)])?;
        suffix.reference(self.text_object())?;
        changed.insert(self.text, prefix);
        changed.insert(self.text_object(), suffix);
        changed.insert(
            author_id,
            PropertyObject {
                jcid: 0x120001,
                bytes: properties(&[(0x1c001d75, string(&self.author))])?,
                global_ids: Arc::new(BTreeMap::from([(0, self.guid)])),
            },
        );
        let mut right = PropertyObject::from_object(&raw.objects[paragraph])?;
        let content = right.reference(self.text_object())?;
        let author = right.reference(author_id)?;
        right.set(&[
            (0x24001c1f, &content),
            (0x14001d09, &self.created.to_le_bytes()),
            (0x20001d78, &author),
            (0x20001d79, &author),
            (0x14001d7a, &modified),
        ])?;
        if !lists.is_empty() {
            let mut references = Vec::new();
            for (i, old) in lists.iter().enumerate() {
                if !matches!(view.nodes[old].kind, Kind::List { .. }) {
                    return Err(invalid("The paragraph list is unavailable"));
                }
                let id = ExGuid {
                    guid: self.guid,
                    n: 5 + u32::try_from(i).unwrap(),
                };
                let mut list = PropertyObject::from_object(&raw.objects[old])?;
                list.remove(&[0x14001cb7])?;
                list.reference(id)?;
                references.extend_from_slice(&right.reference(id)?);
                changed.insert(id, list);
            }
            right.set(&[(0x24001c26, &references)])?;
        }
        changed.insert(self.object(), right);
        let mut original = PropertyObject::from_object(&raw.objects[paragraph])?;
        original.remove(&[0x24001c20])?;
        let author = original.reference(author_id)?;
        original.set(&[(0x20001d79, &author), (0x14001d7a, &modified)])?;
        changed.insert(*paragraph, original);
        let mut parent_object = PropertyObject::from_object(&raw.objects[parent])?;
        let mut children = Vec::new();
        for child in &view.nodes[parent].children {
            children.extend_from_slice(&parent_object.reference(*child)?);
            if child == paragraph {
                children.extend_from_slice(&parent_object.reference(self.object())?);
            }
        }
        parent_object.set(&[(0x24001c20, &children)])?;
        changed.insert(*parent, parent_object);
        if changed
            .keys()
            .any(|id| id.guid == self.guid && raw.objects.contains_key(id))
        {
            return Err(invalid(
                "A split identity already exists; reconcile the existing edit",
            ));
        }
        for id in ancestors {
            let object = match changed.entry(id) {
                std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(PropertyObject::from_object(&raw.objects[&id])?)
                }
            };
            object.set(&[(0x14001d7a, &modified)])?;
        }
        for (id, object) in &changed {
            view.nodes.insert(
                *id,
                Element::parse(
                    &Object {
                        jcid: object.jcid,
                        reference_count: 0,
                        data: ObjectData::Properties(&object.bytes),
                        global_ids: Arc::clone(&object.global_ids),
                    },
                    &store,
                )?,
            );
        }
        let title = page_title(&view, &pages, None)?;
        drop(view);
        if let Some((_, automatic, title)) = title {
            let metadata = raw
                .roots
                .get(&2)
                .ok_or_else(|| invalid("Page title metadata is unavailable"))?;
            if raw.objects[metadata].jcid != 0x20030 {
                return Err(invalid("Page title metadata is unavailable"));
            }
            let title = string(&title);
            let mut object = PropertyObject::from_object(&raw.objects[metadata])?;
            object.set(&[(0x1c001cf3, &title)])?;
            changed.insert(*metadata, object);
            changed
                .get_mut(page)
                .unwrap()
                .set(&[(0x1c001d3c, if automatic { &title } else { &[0, 0] })])?;
        }
        write_revision(source, space, |_| Ok(changed))
    }
}

fn fragment(
    raw: &Object<'_>,
    node: &Element<'_>,
    range: Range<u32>,
    empty: ExGuid,
) -> Result<PropertyObject, Error> {
    let Kind::RichText { text, runs, .. } = &node.kind else {
        return Err(invalid("Select ordinary paragraph text"));
    };
    let units: Vec<_> = text.encode_utf16().collect();
    let start = usize::try_from(range.start)
        .map_err(|_| invalid("Choose a position within the paragraph"))?;
    let end = usize::try_from(range.end)
        .map_err(|_| invalid("Choose a position within the paragraph"))?;
    let slice = units
        .get(start..end)
        .ok_or_else(|| invalid("Choose a position within the paragraph"))?;
    let text = String::from_utf16(slice)
        .map_err(|_| invalid("Choose a position between Unicode scalars"))?;
    let mut segments = Vec::new();
    for run in runs {
        if run.start < range.end && range.start < run.end {
            segments.push((
                run.end.min(range.end) - range.start,
                run.format.unwrap_or(empty),
            ));
        }
    }
    if segments.is_empty() {
        segments.push((0, empty));
    }
    if !range.is_empty() && end == units.len() {
        let last = runs.last().unwrap();
        if last.start == last.end {
            segments.push((range.end - range.start, last.format.unwrap_or(empty)));
        }
    }
    if segments[..segments.len() - 1]
        .windows(2)
        .any(|pair| pair[0].0 >= pair[1].0)
    {
        return Err(invalid("Text-run boundaries must be strictly increasing"));
    }
    let mut object = PropertyObject::from_object(raw)?;
    let mut ends = Vec::new();
    let mut references = Vec::new();
    for (end, id) in segments {
        ends.extend_from_slice(&end.to_le_bytes());
        references.extend_from_slice(&object.reference(id)?);
    }
    ends.truncate(ends.len() - 4);
    object.set(&[
        (0x1c001c22, &string(&text)),
        (0x1c001e12, &ends),
        (0x24001e13, &references),
    ])?;
    Ok(object)
}
