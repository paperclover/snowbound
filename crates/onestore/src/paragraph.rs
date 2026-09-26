use crate::{
    Error, ExGuid, Object, ObjectData, PropertySets,
    active::{ActivePage, Changes},
    create::{current_timestamps, properties, string},
    document::{Element, Kind},
    edit::update_title,
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

    /// Original text identity and UTF-16 split boundary.
    pub fn position(&self) -> (ExGuid, u32) {
        (self.text, self.offset)
    }

    /// Changes the split boundary while retaining allocated identities and creation time.
    /// Preparing the edit validates the offset against the supplied text.
    pub fn reposition(&self, offset: u32) -> Self {
        let mut split = self.clone();
        split.offset = offset;
        split
    }

    pub(crate) fn apply(&self, source: &[u8], space: ExGuid) -> Result<Vec<u8>, Error> {
        if self.guid == [0; 16] || self.text.guid == [0; 16] || self.author.contains('\0') {
            return Err(invalid(
                "Choose paragraph text and an author name without NUL",
            ));
        }
        crate::active::write(source, space, |active| self.changes(active))
    }

    pub(crate) fn changes(&self, active: &ActivePage<'_>) -> Result<Changes, Error> {
        let lists = (5..256)
            .map(|n| ExGuid {
                guid: self.guid,
                n,
            })
            .collect::<Vec<_>>();
        let changes = self.changes_as(active, self.object(), self.text_object(), &lists)?;
        let raw = &active.live.revision;
        if changes
            .keys()
            .any(|id| id.guid == self.guid && raw.objects.contains_key(id))
        {
            return Err(invalid(
                "A split identity already exists; reconcile the existing edit",
            ));
        }
        Ok(changes)
    }

    /// `changes` creating the right paragraph `paragraph` with text `right`, and a copy of
    /// the left paragraph's list nodes under `lists`, in order.
    pub(crate) fn changes_as(
        &self,
        active: &ActivePage<'_>,
        paragraph_id: ExGuid,
        right_text: ExGuid,
        list_ids: &[ExGuid],
    ) -> Result<Changes, Error> {
        let [_] = active.pages.as_slice() else {
            return Err(invalid("Splitting requires a single active page"));
        };
        let view = &active.view;
        let parents = active.editable_parents(self.text)?;
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
        let raw = &active.live.revision;
        let (text, runs) = ordinary_text(view, raw, self.text)?;
        let resolved = view.text_runs(self.text)?;
        let link = |at: usize| {
            resolved
                .get(at)
                .is_some_and(|run| run.format.hyperlink == Some(true))
        };
        if let Some(after) = runs
            .iter()
            .position(|run| run.start < self.offset && self.offset < run.end)
        {
            if link(after) {
                return Err(invalid("A split cannot divide a hyperlink"));
            }
        } else if let Some(after) = runs
            .iter()
            .position(|run| run.start == self.offset && self.offset > 0)
            && link(after - 1)
            && link(after)
            && !resolved[after].text.starts_with('\u{fddf}')
        {
            return Err(invalid("A split cannot divide a hyperlink"));
        }
        let node = &view.nodes[&self.text];
        let length = u32::try_from(text.encode_utf16().count())
            .map_err(|_| invalid("Paragraph exceeds the UTF-16 offset range"))?;
        if self.offset > length || lists.len() > 251 || lists.len() > list_ids.len() {
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
        // Enter at the start leaves the emptied upper paragraph the first run's style, as
        // OneNote does (`evidence/structural-edits/xml/c2s-*`).
        let leading = runs.first().and_then(|run| run.format);
        let modified = current_timestamps()?.0.to_le_bytes();
        let mut changed = BTreeMap::new();
        if (self.offset == 0 && !text.is_empty() && leading.is_none())
            || runs.iter().any(|run| run.format.is_none())
        {
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
            if text.is_empty() {
                typing
            } else if self.offset == 0 {
                leading.unwrap_or(normal_id)
            } else {
                normal_id
            },
        )?;
        let mut suffix = fragment(&raw.objects[&self.text], node, self.offset..length, typing)?;
        prefix.set(&[(0x14001d7a, &modified)])?;
        // Note tags and the recording link stay with the original text.
        suffix.remove(&[0x40003489, 0x1c001c98, 0x14001c99])?;
        suffix.set(&[(0x14001d7a, &modified)])?;
        suffix.reference(right_text)?;
        changed.insert(self.text, prefix);
        changed.insert(right_text, suffix);
        changed.insert(
            author_id,
            PropertyObject {
                jcid: 0x120001,
                bytes: properties(&[(0x1c001d75, string(&self.author))])?,
                global_ids: Arc::new(BTreeMap::from([(0, self.guid)])),
            },
        );
        let mut right = PropertyObject::from_object(&raw.objects[paragraph])?;
        right.reference(paragraph_id)?;
        let content = right.reference(right_text)?;
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
                let id = list_ids[i];
                let mut list = PropertyObject::from_object(&raw.objects[old])?;
                list.remove(&[0x14001cb7])?;
                list.reference(id)?;
                references.extend_from_slice(&right.reference(id)?);
                changed.insert(id, list);
            }
            right.set(&[(0x24001c26, &references)])?;
        }
        changed.insert(paragraph_id, right);
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
                children.extend_from_slice(&parent_object.reference(paragraph_id)?);
            }
        }
        parent_object.set(&[(0x24001c20, &children)])?;
        changed.insert(*parent, parent_object);
        for id in ancestors {
            let object = match changed.entry(id) {
                std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(PropertyObject::from_object(&raw.objects[&id])?)
                }
            };
            object.set(&[(0x14001d7a, &modified)])?;
        }
        update_title(active, view.clone(), &mut changed)?;
        Ok(changed)
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

/// A text object's text and runs, unless it holds what a split or join cannot divide or
/// merge: generated title text, equations, embedded objects or per-run data. Hyperlink
/// fields are ordinary runs; a recording link belongs to the text object.
fn ordinary_text<'a>(
    view: &'a crate::document::Revision<'_>,
    raw: &crate::ResolvedRevision<'_>,
    id: ExGuid,
) -> Result<(&'a str, &'a [crate::document::TextRun]), Error> {
    let node = &view.nodes[&id];
    let Kind::RichText {
        text,
        runs,
        boilerplate,
        ..
    } = &node.kind
    else {
        return Err(invalid("Select ordinary paragraph text"));
    };
    if *boilerplate {
        return Err(invalid("Generated text cannot be split or joined"));
    }
    for run in view.text_runs(id)? {
        if [run.format.math, run.format.embedded_object].contains(&Some(true))
            || run.text.contains('\u{fffc}')
        {
            return Err(invalid(
                "This paragraph contains an equation or embedded object that cannot be split or joined",
            ));
        }
    }
    let ObjectData::Properties(blob) = raw.objects[&id].data else {
        unreachable!()
    };
    if PropertySets::parse(blob)?.sets[0]
        .iter()
        .any(|p| matches!(p.id, 0x40003499 | 0x24003458))
    {
        return Err(invalid(
            "This paragraph contains run metadata that cannot be split or joined",
        ));
    }
    Ok((text, runs))
}

/// Joins adjacent ordinary text in one outline or table cell.
/// The left paragraph survives. Empty left text adopts the right text identity;
/// otherwise the left text survives. The left paragraph's tags are retained.
/// Right-side tags are removed from active text, including when the left text is empty.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParagraphJoin {
    left: ExGuid,
    right: ExGuid,
    author: String,
}

impl ParagraphJoin {
    /// Ordered left and right text identities.
    pub fn texts(&self) -> [ExGuid; 2] {
        [self.left, self.right]
    }

    /// Select the preceding leaf paragraph's text and the following paragraph's text.
    pub fn new(left: ExGuid, right: ExGuid, author: &str) -> Result<Self, Error> {
        if left == right || left.guid == [0; 16] || right.guid == [0; 16] || author.contains('\0') {
            return Err(invalid(
                "Choose two distinct text objects and an author name without NUL",
            ));
        }
        Ok(Self {
            left,
            right,
            author: author.to_owned(),
        })
    }

    pub(crate) fn apply(&self, source: &[u8], space: ExGuid) -> Result<Vec<u8>, Error> {
        if self.left == self.right
            || self.left.guid == [0; 16]
            || self.right.guid == [0; 16]
            || self.author.contains('\0')
        {
            return Err(invalid(
                "Choose two distinct text objects and an author name without NUL",
            ));
        }
        crate::active::write(source, space, |active| self.changes(active))
    }

    pub(crate) fn changes(&self, active: &ActivePage<'_>) -> Result<Changes, Error> {
        let pages = &active.pages;
        if pages.len() != 1 {
            return Err(invalid("Joining requires a single active page"));
        }
        let view = &active.view;
        let parents = active.editable_parents(self.left)?;
        active.editable_parents(self.right)?;
        let parent = |id| -> Result<ExGuid, Error> {
            let [parent] = parents.get(&id).map(Vec::as_slice).unwrap_or_default() else {
                return Err(invalid("Select text with a unique editable parent"));
            };
            Ok(*parent)
        };
        let left = parent(self.left)?;
        let right = parent(self.right)?;
        for (paragraph, text) in [(left, self.left), (right, self.right)] {
            if !matches!(view.nodes[&paragraph].kind, Kind::Paragraph { .. })
                || view.nodes[&paragraph].content != [text]
            {
                return Err(invalid("Select ordinary paragraph text"));
            }
            let mut at = paragraph;
            while at != pages[0] {
                if matches!(view.nodes[&at].kind, Kind::Title) {
                    return Err(invalid(
                        "Title containers cannot be joined with ordinary paragraphs",
                    ));
                }
                at = parent(at)?;
            }
        }
        if !view.nodes[&left].children.is_empty() {
            return Err(invalid("Select the preceding paragraph's last descendant"));
        }
        let right_parent = parent(right)?;
        if !matches!(
            view.nodes[&right_parent].kind,
            Kind::Outline { .. } | Kind::OutlineGroup | Kind::Paragraph { .. } | Kind::Cell { .. }
        ) {
            return Err(invalid("Select paragraphs in one outline or table cell"));
        }
        let mut stem = left;
        while parent(stem)? != right_parent {
            let previous = stem;
            stem = parent(stem)?;
            if !matches!(
                view.nodes[&stem].kind,
                Kind::Paragraph { .. } | Kind::OutlineGroup
            ) || view.nodes[&stem].children.last() != Some(&previous)
            {
                return Err(invalid(
                    "Select adjacent text within one outline or table cell",
                ));
            }
        }
        let siblings = &view.nodes[&right_parent].children;
        if !siblings.windows(2).any(|pair| pair == [stem, right]) {
            return Err(invalid("Select adjacent paragraph text in document order"));
        }
        let children = &view.nodes[&right].children;
        if !children.is_empty()
            && !view.nodes[&stem].children.is_empty()
            && view.nodes[&stem].child_level != view.nodes[&right].child_level
        {
            return Err(invalid(
                "Joining these child indentation levels requires a hierarchy edit",
            ));
        }
        let raw = &active.live.revision;
        let (a, a_runs) = ordinary_text(view, raw, self.left)?;
        let (b, b_runs) = ordinary_text(view, raw, self.right)?;
        let length = u32::try_from(a.encode_utf16().count())
            .map_err(|_| invalid("Paragraph exceeds the UTF-16 offset range"))?;
        let right_length = u32::try_from(b.encode_utf16().count())
            .map_err(|_| invalid("Paragraph exceeds the UTF-16 offset range"))?;
        length
            .checked_add(right_length)
            .ok_or_else(|| invalid("Joined text exceeds the UTF-16 offset range"))?;
        let modified = current_timestamps()?.0.to_le_bytes();
        let mut changed = BTreeMap::new();
        let (survivor, mut text) = if a.is_empty() {
            let mut target = PropertyObject::from_object(&raw.objects[&self.right])?;
            target.copy_property(
                &PropertyObject::from_object(&raw.objects[&self.left])?,
                0x40003489,
            )?;
            (self.right, target)
        } else {
            let mut target = PropertyObject::from_object(&raw.objects[&self.left])?;
            let mut styles = BTreeMap::new();
            let mut ends = Vec::new();
            let mut references = Vec::new();
            let mut empty_style = None;
            for run in a_runs.iter().filter(|run| run.start < run.end) {
                let id = if let Some(id) = run.format {
                    id
                } else if let Some(id) = empty_style {
                    id
                } else {
                    let id = ExGuid {
                        guid: fresh_guid()?,
                        n: 1,
                    };
                    changed.insert(
                        id,
                        PropertyObject {
                            jcid: 0x12004d,
                            bytes: properties(&[])?,
                            global_ids: Arc::new(BTreeMap::from([(0, id.guid)])),
                        },
                    );
                    empty_style = Some(id);
                    id
                };
                references.extend_from_slice(&target.reference(id)?);
                ends.extend_from_slice(&run.end.to_le_bytes());
            }
            let left_base = character_properties(view, raw, self.left)?;
            let right_base = character_properties(view, raw, self.right)?;
            for (i, run) in b_runs.iter().enumerate() {
                if run.start == run.end && i != b_runs.len() - 1 {
                    continue;
                }
                let id = if let Some(id) = styles.get(&run.format) {
                    *id
                } else {
                    let mut style = if let Some(id) = run.format {
                        PropertyObject::from_object(&raw.objects[&id])?
                    } else {
                        PropertyObject {
                            jcid: 0x12004d,
                            bytes: properties(&[])?,
                            global_ids: Arc::new(BTreeMap::new()),
                        }
                    };
                    if left_base != right_base {
                        let mut values = right_base.clone();
                        for property in &PropertySets::parse(&style.bytes)?.sets[0] {
                            let key = property.id & 0x7fffffff;
                            if is_character_property(key) {
                                let value = match property.value {
                                    crate::Value::NoData => &[][..],
                                    crate::Value::Bytes(bytes) => bytes,
                                    _ => unreachable!(),
                                };
                                values.insert(key, (property.id, value.to_vec()));
                            }
                        }
                        for key in left_base.keys() {
                            if values.contains_key(key) {
                                continue;
                            }
                            let value = match key >> 26 & 31 {
                                2 => Vec::new(),
                                _ if matches!(*key, 0x14001c0c | 0x14001c0d) => {
                                    0xff000000_u32.to_le_bytes().to_vec()
                                }
                                _ => {
                                    return Err(invalid(
                                        "The right paragraph's implicit font or language cannot be preserved under the left style",
                                    ));
                                }
                            };
                            values.insert(*key, (*key, value));
                        }
                        style.set(
                            &values
                                .values()
                                .map(|(id, value)| (*id, value.as_slice()))
                                .collect::<Vec<_>>(),
                        )?;
                    }
                    let id = if let Some(id) = run
                        .format
                        .filter(|id| raw.objects[id].data == ObjectData::Properties(&style.bytes))
                    {
                        id
                    } else {
                        let id = ExGuid {
                            guid: fresh_guid()?,
                            n: 1,
                        };
                        style.reference(id)?;
                        changed.insert(id, style);
                        id
                    };
                    styles.insert(run.format, id);
                    id
                };
                references.extend_from_slice(&target.reference(id)?);
                ends.extend_from_slice(&(length + run.end).to_le_bytes());
            }
            ends.truncate(ends.len() - 4);
            if ends
                .chunks_exact(4)
                .map(|bytes| u32::from_le_bytes(bytes.try_into().unwrap()))
                .collect::<Vec<_>>()
                .windows(2)
                .any(|pair| pair[0] >= pair[1])
            {
                return Err(invalid("Text-run boundaries must be strictly increasing"));
            }
            target.set(&[
                (0x1c001c22, &string(&format!("{a}{b}"))),
                (0x1c001e12, &ends),
                (0x24001e13, &references),
            ])?;
            (self.left, target)
        };
        text.set(&[(0x14001d7a, &modified)])?;
        changed.insert(survivor, text);
        let author = ExGuid {
            guid: fresh_guid()?,
            n: 1,
        };
        changed.insert(
            author,
            PropertyObject {
                jcid: 0x120001,
                bytes: properties(&[(0x1c001d75, string(&self.author))])?,
                global_ids: Arc::new(BTreeMap::from([(0, author.guid)])),
            },
        );
        let mut left_object = PropertyObject::from_object(&raw.objects[&left])?;
        let content = left_object.reference(survivor)?;
        let author = left_object.reference(author)?;
        left_object.set(&[(0x24001c1f, &content), (0x20001d79, &author)])?;
        changed.insert(left, left_object);
        if !children.is_empty() {
            let target = match changed.entry(stem) {
                std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(PropertyObject::from_object(&raw.objects[&stem])?)
                }
            };
            let mut references = Vec::new();
            for id in view.nodes[&stem].children.iter().chain(children) {
                references.extend_from_slice(&target.reference(*id)?);
            }
            target.set(&[(0x24001c20, &references)])?;
            target.copy_property(
                &PropertyObject::from_object(&raw.objects[&right])?,
                0x0c001c03,
            )?;
        }
        let mut container = PropertyObject::from_object(&raw.objects[&right_parent])?;
        let mut references = Vec::new();
        for id in siblings.iter().filter(|id| **id != right) {
            references.extend_from_slice(&container.reference(*id)?);
        }
        container.set(&[(0x24001c20, &references)])?;
        changed.insert(right_parent, container);
        let mut pending = vec![left, right_parent];
        let mut ancestors = BTreeSet::new();
        while let Some(id) = pending.pop() {
            if !ancestors.insert(id) {
                continue;
            }
            let object = match changed.entry(id) {
                std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(PropertyObject::from_object(&raw.objects[&id])?)
                }
            };
            object.set(&[(0x14001d7a, &modified)])?;
            pending.extend(parents.get(&id).into_iter().flatten().copied());
        }
        update_title(active, view.clone(), &mut changed)?;
        Ok(changed)
    }
}

fn is_character_property(id: u32) -> bool {
    matches!(
        id,
        0x08001c04
            ..=0x08001c09
                | 0x08001e16
                | 0x08001e14
                | 0x08001e19
                | 0x08003401
                | 0x08001e22
                | 0x08003476
                | 0x1c001c0a
                | 0x10001c0b
                | 0x14001c0c
                | 0x14001c0d
                | 0x14001c3b
    )
}

fn character_properties(
    view: &crate::document::Revision<'_>,
    raw: &crate::ResolvedRevision<'_>,
    text: ExGuid,
) -> Result<BTreeMap<u32, (u32, Vec<u8>)>, Error> {
    let Kind::RichText {
        paragraph_style, ..
    } = view.nodes[&text].kind
    else {
        unreachable!()
    };
    let mut values = BTreeMap::new();
    for id in paragraph_style.into_iter().chain([text]) {
        let ObjectData::Properties(bytes) = raw.objects[&id].data else {
            unreachable!()
        };
        for property in &PropertySets::parse(bytes)?.sets[0] {
            let key = property.id & 0x7fffffff;
            if is_character_property(key) {
                let value = match property.value {
                    crate::Value::NoData => &[][..],
                    crate::Value::Bytes(bytes) => bytes,
                    _ => unreachable!(),
                };
                values.insert(key, (property.id, value.to_vec()));
            }
        }
    }
    Ok(values)
}
