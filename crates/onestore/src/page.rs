use crate::{
    Error, ExGuid,
    create::{current_timestamps, properties, string},
    document::{Document, FieldValue, Kind},
    write::{PropertyObject, RevisionEdit, fresh_guid, write_revisions},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

fn invalid(message: &'static str) -> Error {
    Error { offset: 0, message }
}

fn metadata_id(space: ExGuid) -> ExGuid {
    let mut id = ExGuid {
        guid: space.guid,
        n: 1,
    };
    for (byte, salt) in id.guid.iter_mut().zip([
        0x31, 0xc0, 0xa8, 0x22, 0, 0x36, 0xee, 0x42, 0xb7, 0x14, 0xd7, 0xac, 0xda, 0x24, 0x35, 0xe8,
    ]) {
        *byte ^= salt;
    }
    id
}

fn metadata_guid(metadata: &crate::document::Element<'_>) -> Result<[u8; 16], Error> {
    metadata.extra[0]
        .iter()
        .find_map(|field| match field.value {
            FieldValue::Bytes(bytes) if field.id == 0x1c001c30 => bytes.try_into().ok(),
            _ => None,
        })
        .ok_or_else(|| invalid("Page metadata needs its page identifier"))
}

fn set_references(object: &mut PropertyObject, property: u32, ids: &[ExGuid]) -> Result<(), Error> {
    let mut references = Vec::new();
    for id in ids {
        references.extend_from_slice(&object.reference(*id)?);
    }
    let source = PropertyObject {
        jcid: object.jcid,
        bytes: properties(&[(property, references)])?,
        global_ids: Arc::clone(&object.global_ids),
    };
    object.copy_property(&source, property)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
enum Position {
    Keep,
    Before(Option<ExGuid>),
}

/// A page's explicit position and indentation, with a stable identity for a new series.
/// Moving a page does not implicitly select its following subpages.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageEdit {
    guid: [u8; 16],
    space: ExGuid,
    level: u32,
    position: Position,
}

impl PageEdit {
    /// Changes one page's indentation without changing its position.
    pub fn set_level(space: ExGuid, level: u32) -> Result<Self, Error> {
        Self::new(space, level, Position::Keep)
    }

    /// Moves before an existing page space, or appends when `before` is None.
    pub fn move_to(space: ExGuid, before: Option<ExGuid>, level: u32) -> Result<Self, Error> {
        Self::new(space, level, Position::Before(before))
    }

    pub fn space(&self) -> ExGuid {
        self.space
    }

    fn new(space: ExGuid, level: u32, position: Position) -> Result<Self, Error> {
        let edit = Self {
            guid: fresh_guid()?,
            space,
            level,
            position,
        };
        edit.validate()?;
        Ok(edit)
    }

    fn validate(&self) -> Result<(), Error> {
        if self.guid == [0; 16] || self.space.guid == [0; 16] || !(1..=3).contains(&self.level) {
            return Err(invalid(
                "Choose an existing page space and indentation level 1 through 3",
            ));
        }
        if let Position::Before(Some(before)) = self.position
            && (before.guid == [0; 16] || before == self.space)
        {
            return Err(invalid("Choose a different page as the movement anchor"));
        }
        Ok(())
    }

    pub(crate) fn apply(source: &[u8], edits: &[Self]) -> Result<Vec<u8>, Error> {
        let mut selected = BTreeMap::new();
        let mut guids = BTreeSet::new();
        for edit in edits {
            edit.validate()?;
            if selected.insert(edit.space, edit).is_some() || !guids.insert(edit.guid) {
                return Err(invalid(
                    "Use distinct page and edit identities within a batch",
                ));
            }
        }
        write_revisions(source, |index| {
            let document = Document::parse(index)?;
            let pages = document.pages()?;
            let section = &document.spaces[&document.root];
            let view = &section.revisions[&section.contexts[&ExGuid::default()]];
            let section_id = view.roots[&1];
            let section_node = &view.nodes[&section_id];
            if !matches!(section_node.kind, Kind::Section { .. })
                || section_node.extra[0]
                    .iter()
                    .any(|field| field.id == 0x88001cde)
            {
                return Err(invalid("Choose an editable section for page movement"));
            }
            let mut order = Vec::new();
            let mut levels = BTreeMap::new();
            let mut page_guids = BTreeMap::new();
            for (sid, _) in &pages {
                let page = &document.spaces[sid];
                let page = &page.revisions[&page.contexts[&ExGuid::default()]];
                let Some(metadata) = page.roots.get(&2).and_then(|id| page.nodes.get(id)) else {
                    return Err(invalid("Page metadata is unavailable"));
                };
                let Kind::Metadata { level, .. } = metadata.kind else {
                    return Err(invalid("Choose ordinary pages with page metadata"));
                };
                let level = level.unwrap_or(1);
                if !(1..=3).contains(&level) || levels.insert(*sid, level).is_some() {
                    return Err(invalid(
                        "Each page space must contain one page with a valid level",
                    ));
                }
                if page_guids.insert(metadata_guid(metadata)?, *sid).is_some() {
                    return Err(invalid("Pages must have distinct metadata identifiers"));
                }
                order.push(*sid);
            }
            let original_order = order.clone();
            let original_levels = levels.clone();
            let mut series_by_head = BTreeMap::new();
            let mut copies = BTreeMap::new();
            let mut copy_ids = BTreeSet::new();
            for oid in &section_node.children {
                let series = &view.nodes[oid];
                let Some(head) = series.spaces.first() else {
                    return Err(invalid("A page series must contain at least one page"));
                };
                if levels.get(head) != Some(&1)
                    || series.spaces[1..]
                        .iter()
                        .any(|sid| levels.get(sid).is_none_or(|level| *level == 1))
                {
                    return Err(invalid(
                        "A page series must start with its only top-level page",
                    ));
                }
                series_by_head.insert(*head, *oid);
                if let Some(field) = series.extra[0].iter().find(|field| field.id == 0x24003442) {
                    let FieldValue::Objects(ids) = &field.value else {
                        return Err(invalid("Page metadata copies must be object references"));
                    };
                    if ids.len() != series.spaces.len() {
                        return Err(invalid("Page metadata copies must match their series"));
                    }
                    for id in ids {
                        if !copy_ids.insert(*id)
                            || !matches!(
                                view.nodes.get(id).map(|n| &n.kind),
                                Some(Kind::Metadata { .. })
                            )
                        {
                            return Err(invalid("Each page needs its own ordinary metadata copy"));
                        }
                        let sid = page_guids
                            .get(&metadata_guid(&view.nodes[id])?)
                            .filter(|sid| series.spaces.contains(sid))
                            .ok_or_else(|| {
                                invalid("Metadata copy must identify a page in its series")
                            })?;
                        if copies.insert(*sid, *id).is_some() {
                            return Err(invalid("Each page needs its own ordinary metadata copy"));
                        }
                    }
                }
            }
            for edit in edits {
                let Some(at) = order.iter().position(|sid| *sid == edit.space) else {
                    return Err(invalid("The selected page is no longer in the section"));
                };
                levels.insert(edit.space, edit.level);
                if let Position::Before(before) = edit.position {
                    order.remove(at);
                    let position = match before {
                        None => order.len(),
                        Some(before) => {
                            order.iter().position(|sid| *sid == before).ok_or_else(|| {
                                invalid("The movement anchor is no longer in the section")
                            })?
                        }
                    };
                    order.insert(position, edit.space);
                }
            }
            if order.first().is_some_and(|sid| levels[sid] != 1) {
                return Err(invalid(
                    "The section's first page must have indentation level 1",
                ));
            }
            if order == original_order && levels == original_levels {
                return Ok(BTreeMap::new());
            }
            let raw = index.resolve(document.root, section.contexts[&ExGuid::default()])?;
            let mut groups: Vec<Vec<ExGuid>> = Vec::new();
            for sid in order {
                if levels[&sid] == 1 {
                    groups.push(Vec::new());
                }
                groups.last_mut().unwrap().push(sid);
            }
            let mut changes = BTreeMap::new();
            let mut replacements = BTreeMap::new();
            let mut children = Vec::new();
            for spaces in groups {
                let head = spaces[0];
                let old_id = series_by_head.get(&head).copied();
                let (id, mut series) = if let Some(id) = old_id {
                    (id, PropertyObject::from_object(&raw.objects[&id])?)
                } else {
                    let edit = selected
                        .get(&head)
                        .ok_or_else(|| invalid("A new series must start at an edited page"))?;
                    let id = ExGuid {
                        guid: edit.guid,
                        n: 1,
                    };
                    if raw.objects.contains_key(&id) {
                        return Err(invalid("The new page-series identity already exists"));
                    }
                    (
                        id,
                        PropertyObject {
                            jcid: 0x60008,
                            bytes: properties(&[(0x1c001c30, edit.guid.to_vec())])?,
                            global_ids: Arc::new(BTreeMap::from([(0, edit.guid)])),
                        },
                    )
                };
                children.push(id);
                let membership_changed = old_id.is_none_or(|old| view.nodes[&old].spaces != spaces);
                if !membership_changed
                    && !spaces.iter().any(|sid| levels[sid] != original_levels[sid])
                {
                    continue;
                }
                let mut metadata_ids = Vec::new();
                for sid in &spaces {
                    let page =
                        index.resolve(*sid, index.spaces[sid].labels[&(ExGuid::default(), 1)])?;
                    let page_id = page.roots[&2];
                    let original = PropertyObject::from_object(&page.objects[&page_id])?;
                    if old_id.is_none() && *sid == head {
                        series.copy_property(&original, 0x18001c65)?;
                    }
                    if levels[sid] != original_levels[sid] {
                        let mut metadata = PropertyObject::from_object(&page.objects[&page_id])?;
                        metadata.set(&[(0x14001dff, &levels[sid].to_le_bytes())])?;
                        changes.insert(
                            *sid,
                            RevisionEdit::Update(BTreeMap::from([(page_id, metadata)])),
                        );
                    }
                    let copy_id = copies
                        .get(sid)
                        .copied()
                        .unwrap_or_else(|| metadata_id(*sid));
                    let mut copy = match raw.objects.get(&copy_id) {
                        Some(object) => {
                            if object.jcid != 0x20030
                                || (!copies.contains_key(sid) && copy_ids.contains(&copy_id))
                            {
                                return Err(invalid("Page metadata identities overlap"));
                            }
                            PropertyObject::from_object(object)?
                        }
                        None => original,
                    };
                    copy.set(&[(0x14001dff, &levels[sid].to_le_bytes())])?;
                    copy.reference(copy_id)?;
                    if replacements.insert(copy_id, copy).is_some() {
                        return Err(invalid("Page metadata identities overlap"));
                    }
                    metadata_ids.push(copy_id);
                }
                if membership_changed || spaces.iter().any(|sid| !copies.contains_key(sid)) {
                    set_references(&mut series, 0x2c001d63, &spaces)?;
                    set_references(&mut series, 0x24003442, &metadata_ids)?;
                    if replacements.insert(id, series).is_some() {
                        return Err(invalid("Page-series and metadata identities overlap"));
                    }
                }
            }
            if children != section_node.children {
                let mut parent = PropertyObject::from_object(&raw.objects[&section_id])?;
                set_references(&mut parent, 0x24001c20, &children)?;
                replacements.insert(section_id, parent);
            }
            changes.insert(document.root, RevisionEdit::Update(replacements));
            Ok(changes)
        })
    }
}

/// An empty top-level page with stable identities and creation time.
/// Retain the intent across retries; an existing page identity rejects duplicate creation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageCreation {
    guid: [u8; 16],
    series_guid: [u8; 16],
    before: Option<ExGuid>,
    title: Option<String>,
    author: String,
    created: u32,
}

impl PageCreation {
    /// Appends a page, or inserts before the first page space of an existing series.
    /// `Some("")` creates an empty title field; `None` creates a page without a title node.
    /// The page has no body outlines or generated date/time text and does not apply a template.
    pub fn new(before: Option<ExGuid>, title: Option<&str>, author: &str) -> Result<Self, Error> {
        let page = Self {
            guid: fresh_guid()?,
            series_guid: fresh_guid()?,
            before,
            title: title.map(str::to_owned),
            author: author.to_owned(),
            created: current_timestamps()?.0,
        };
        page.validate()?;
        Ok(page)
    }

    pub fn space(&self) -> ExGuid {
        ExGuid {
            guid: self.guid,
            n: 1,
        }
    }

    pub fn object(&self) -> ExGuid {
        ExGuid {
            guid: self.guid,
            n: 12,
        }
    }

    pub fn title_object(&self) -> Option<ExGuid> {
        self.title.as_ref().map(|_| ExGuid {
            guid: self.guid,
            n: 16,
        })
    }

    /// Changes the insertion anchor while retaining all identities and creation metadata.
    pub fn reposition(&self, before: Option<ExGuid>) -> Result<Self, Error> {
        let mut page = self.clone();
        page.before = before;
        page.validate()?;
        Ok(page)
    }

    fn validate(&self) -> Result<(), Error> {
        if self.guid == [0; 16]
            || self.series_guid == [0; 16]
            || self.guid == self.series_guid
            || self.author.contains('\0')
            || self.before.is_some_and(|id| id.guid == [0; 16])
            || self
                .title
                .as_ref()
                .is_some_and(|title| title.contains(['\0', '\r', '\n', '\u{fffc}', '\u{fddf}']))
        {
            return Err(invalid(
                "Use a new page identity, an existing page anchor, and ordinary single-line title text",
            ));
        }
        Ok(())
    }

    pub(crate) fn apply(&self, source: &[u8]) -> Result<Vec<u8>, Error> {
        self.validate()?;
        write_revisions(source, |index| {
            if index.spaces.contains_key(&self.space()) {
                return Err(invalid(
                    "This page identity already exists; reconcile the original creation",
                ));
            }
            let document = Document::parse(index)?;
            document.pages()?;
            let section = &document.spaces[&document.root];
            let view = &section.revisions[&section.contexts[&ExGuid::default()]];
            let section_id = view.roots[&1];
            let section_node = &view.nodes[&section_id];
            if !matches!(section_node.kind, Kind::Section { .. })
                || section_node.extra[0]
                    .iter()
                    .any(|field| field.id == 0x88001cde)
            {
                return Err(invalid("Choose an editable section for the new page"));
            }
            let position = match self.before {
                None => section_node.children.len(),
                Some(before) => section_node
                    .children
                    .iter()
                    .position(|id| view.nodes[id].spaces.first() == Some(&before))
                    .ok_or_else(|| {
                        invalid("Insert before the first page of an existing series, or append")
                    })?,
            };
            let raw = index.resolve(document.root, section.contexts[&ExGuid::default()])?;
            let id = |n| ExGuid { guid: self.guid, n };
            let reference = |n: u32| n.to_le_bytes().to_vec();
            let timestamp = ((u64::from(self.created) + 315532800 + 11644473600) * 10000000)
                .to_le_bytes()
                .to_vec();
            let modified = || (0x14001d7a, self.created.to_le_bytes().to_vec());
            let mut table = BTreeMap::from([(0, self.guid)]);
            let metadata_id = metadata_id(self.space());
            table.insert(1, metadata_id.guid);
            let table = Arc::new(table);
            let object = |jcid, values: Vec<_>| -> Result<_, Error> {
                Ok(PropertyObject {
                    jcid,
                    bytes: properties(&values)?,
                    global_ids: Arc::clone(&table),
                })
            };
            let title = self.title.as_deref().unwrap_or_default().trim_start();
            let metadata = vec![
                (0x1c001c30, self.guid.to_vec()),
                (0x1c001cf3, string(title)),
                (0x14001d82, reference(40)),
                (0x1400348b, reference(40)),
                (0x14001dff, reference(1)),
                (0x18001c65, timestamp.clone()),
            ];
            let mut page = vec![
                modified(),
                (0x1c001d75, string(&self.author)),
                (0x1c001d3c, string("")),
            ];
            if self.title.is_some() {
                page.push((0x24001d5f, reference(13)));
            }
            let mut objects = BTreeMap::from([
                (id(10), object(0x60037, vec![(0x24001c1f, reference(12))])?),
                (id(11), object(0x20030, metadata.clone())?),
                (id(12), object(0x6000b, page)?),
            ]);
            if let Some(title) = &self.title {
                for (n, jcid, values) in [
                    (
                        13,
                        0x6002c,
                        vec![
                            modified(),
                            (0x24001c20, reference(14)),
                            (0x14001c14, 0_f32.to_le_bytes().to_vec()),
                            (0x14001c15, 0_f32.to_le_bytes().to_vec()),
                        ],
                    ),
                    (
                        14,
                        0x6000c,
                        vec![
                            modified(),
                            (0x24001c20, reference(15)),
                            (0x0c001c03, vec![1]),
                        ],
                    ),
                    (
                        15,
                        0x6000d,
                        vec![
                            modified(),
                            (0x24001c1f, reference(16)),
                            (0x0c001c03, vec![1]),
                            (0x20001d78, reference(17)),
                            (0x20001d79, reference(17)),
                            (0x14001d09, self.created.to_le_bytes().to_vec()),
                            (0x88001cb4, vec![]),
                        ],
                    ),
                    (
                        16,
                        0x6000e,
                        vec![
                            modified(),
                            (0x1c001c22, string(title)),
                            (0x24001e13, reference(19)),
                            (0x2000342c, reference(18)),
                            (0x10001cfe, 0x409_u16.to_le_bytes().to_vec()),
                            (0x88001cb4, vec![]),
                        ],
                    ),
                    (17, 0x120001, vec![(0x1c001d75, string(&self.author))]),
                    (
                        18,
                        0x12004d,
                        vec![
                            (0x1c00345a, string("PageTitle")),
                            (0x1c001c0a, string("Calibri")),
                            (0x10001c0b, 34_u16.to_le_bytes().to_vec()),
                        ],
                    ),
                    (19, 0x12004d, vec![(0x14001c3b, reference(0x409))]),
                ] {
                    objects.insert(id(n), object(jcid, values)?);
                }
            }
            let series = object(
                0x60008,
                vec![
                    (0x1c001c30, self.series_guid.to_vec()),
                    (0x18001c65, timestamp),
                    (0x2c001d63, reference(1)),
                    (0x24003442, reference(257)),
                ],
            )?;
            if raw.objects.contains_key(&id(2)) || raw.objects.contains_key(&metadata_id) {
                return Err(invalid("The new page's section identities already exist"));
            }
            let mut parent = PropertyObject::from_object(&raw.objects[&section_id])?;
            let mut children = section_node.children.clone();
            children.insert(position, id(2));
            let mut references = Vec::new();
            for child in children {
                references.extend_from_slice(&parent.reference(child)?);
            }
            parent.set(&[(0x24001c20, &references)])?;
            Ok(BTreeMap::from([
                (
                    document.root,
                    RevisionEdit::Update(BTreeMap::from([
                        (section_id, parent),
                        (id(2), series),
                        (metadata_id, object(0x20030, metadata)?),
                    ])),
                ),
                (
                    self.space(),
                    RevisionEdit::Create {
                        roots: BTreeMap::from([(1, id(10)), (2, id(11))]),
                        objects,
                    },
                ),
            ]))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{RevisionIndex, Store};

    #[test]
    fn page_movement_restores_an_optional_metadata_cache_from_history() {
        let source =
            include_bytes!("../../../corpus/page-lifecycle/04-nested/notebook/Lifecycle.one");
        let store = Store::parse(source).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let document = Document::parse(&index).unwrap();
        let sid = document.pages().unwrap()[3].0;
        let section = &document.spaces[&document.root];
        let view = &section.revisions[&section.contexts[&ExGuid::default()]];
        let series_id = view.nodes[&view.roots[&1]].children[3];
        let copy_id = metadata_id(sid);
        for unknown in [false, true] {
            let source = if unknown {
                write_revisions(source, |index| {
                    let raw = index.resolve(
                        index.root,
                        index.spaces[&index.root].labels[&(ExGuid::default(), 1)],
                    )?;
                    let mut copy = PropertyObject::from_object(&raw.objects[&copy_id])?;
                    copy.set(&[(0x1400abcd, &77_u32.to_le_bytes())])?;
                    Ok(BTreeMap::from([(
                        index.root,
                        RevisionEdit::Update(BTreeMap::from([(copy_id, copy)])),
                    )]))
                })
                .unwrap()
            } else {
                source.to_vec()
            };
            let source = write_revisions(&source, |index| {
                let raw = index.resolve(
                    index.root,
                    index.spaces[&index.root].labels[&(ExGuid::default(), 1)],
                )?;
                let mut series = PropertyObject::from_object(&raw.objects[&series_id])?;
                series.remove(&[0x24003442])?;
                Ok(BTreeMap::from([(
                    index.root,
                    RevisionEdit::Update(BTreeMap::from([(series_id, series)])),
                )]))
            })
            .unwrap();
            for changed in [false, true] {
                for unrelated_edit in [false, true] {
                    let result = write_revisions(&source, |index| {
                        let raw = index.resolve(
                            index.root,
                            index.spaces[&index.root].labels[&(ExGuid::default(), 1)],
                        )?;
                        assert!(!raw.reachable()?.contains(&copy_id));
                        let mut copy = PropertyObject::from_object(&raw.objects[&copy_id])?;
                        if changed {
                            copy.set(&[(0x14001dff, &2_u32.to_le_bytes())])?;
                        }
                        let mut objects = BTreeMap::from([(copy_id, copy)]);
                        if unrelated_edit {
                            let mut series = PropertyObject::from_object(&raw.objects[&series_id])?;
                            series.set(&[(0x1400abcd, &88_u32.to_le_bytes())])?;
                            objects.insert(series_id, series);
                        }
                        Ok(BTreeMap::from([(
                            index.root,
                            RevisionEdit::Update(objects),
                        )]))
                    });
                    assert_eq!(
                        result.unwrap_err().message,
                        "Edited object is not reachable in the resulting revision"
                    );
                }
            }
            let written =
                PageEdit::apply(&source, &[PageEdit::set_level(sid, 2).unwrap()]).unwrap();
            let store = Store::parse(&written).unwrap();
            let index = RevisionIndex::parse(&store).unwrap();
            index.validate_current().unwrap();
            assert_eq!(Document::parse(&index).unwrap().pages().unwrap().len(), 9);
            let current = index
                .resolve(
                    index.root,
                    index.spaces[&index.root].labels[&(ExGuid::default(), 1)],
                )
                .unwrap();
            assert!(current.reachable().unwrap().contains(&copy_id));
            let copy = PropertyObject::from_object(&current.objects[&copy_id]).unwrap();
            let properties = crate::PropertySets::parse(&copy.bytes).unwrap();
            assert_eq!(unknown, properties.sets[0].iter().any(|field| field.id == 0x1400abcd
            && matches!(field.value, crate::Value::Bytes(bytes) if bytes == 77_u32.to_le_bytes())));
            let old_store = Store::parse(&source).unwrap();
            let old_index = RevisionIndex::parse(&old_store).unwrap();
            for (space, revisions) in &old_index.spaces {
                for revision in revisions.revisions.keys() {
                    assert_eq!(
                        format!("{:?}", old_index.resolve(*space, *revision).unwrap()),
                        format!("{:?}", index.resolve(*space, *revision).unwrap())
                    );
                }
            }
            if !unknown && let Some(output) = std::env::var_os("ONESTORE_PAGE_MOVEMENT_OUTPUT") {
                let output = std::path::Path::new(&output);
                assert!(output.is_absolute());
                for (name, bytes) in [
                    ("optional-cache-source", source),
                    ("optional-cache", written),
                ] {
                    let path = output.join(name);
                    std::fs::create_dir_all(&path).unwrap();
                    std::fs::write(path.join("Lifecycle.one"), bytes).unwrap();
                }
            }
        }
    }

    #[test]
    fn page_movement_rejects_empty_manifests_and_series() {
        let source =
            include_bytes!("../../../corpus/page-lifecycle/04-nested/notebook/Lifecycle.one");
        let store = Store::parse(source).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let document = Document::parse(&index).unwrap();
        let sid = document.pages().unwrap()[3].0;
        for empty_series in [false, true] {
            let invalid_source = write_revisions(source, |index| {
                let target = if empty_series { index.root } else { sid };
                let raw = index.resolve(
                    target,
                    index.spaces[&target].labels[&(ExGuid::default(), 1)],
                )?;
                let id = if empty_series {
                    let space = &document.spaces[&document.root];
                    let view = &space.revisions[&space.contexts[&ExGuid::default()]];
                    view.nodes[&view.roots[&1]].children[3]
                } else {
                    raw.roots[&1]
                };
                let mut object = PropertyObject::from_object(&raw.objects[&id])?;
                set_references(
                    &mut object,
                    if empty_series { 0x2c001d63 } else { 0x24001c1f },
                    &[],
                )?;
                Ok(BTreeMap::from([(
                    target,
                    RevisionEdit::Update(BTreeMap::from([(id, object)])),
                )]))
            })
            .unwrap();
            assert!(
                PageEdit::apply(&invalid_source, &[PageEdit::set_level(sid, 2).unwrap()]).is_err()
            );
        }
    }

    #[test]
    fn page_moves_retain_unknown_series_and_metadata_properties() {
        let source =
            include_bytes!("../../../corpus/page-lifecycle/04-nested/notebook/Lifecycle.one");
        let store = Store::parse(source).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let document = Document::parse(&index).unwrap();
        let pages = document.pages().unwrap();
        let section = &document.spaces[&document.root];
        let view = &section.revisions[&section.contexts[&ExGuid::default()]];
        let series_id = view.nodes[&view.roots[&1]].children[3];
        let field = view.nodes[&series_id].extra[0]
            .iter()
            .find(|f| f.id == 0x24003442)
            .unwrap();
        let FieldValue::Objects(copies) = &field.value else {
            panic!()
        };
        let copy_id = copies[0];
        let source = write_revisions(source, |index| {
            let raw = index.resolve(
                index.root,
                index.spaces[&index.root].labels[&(ExGuid::default(), 1)],
            )?;
            let mut objects = BTreeMap::new();
            for id in [series_id, copy_id] {
                let mut object = PropertyObject::from_object(&raw.objects[&id])?;
                object.set(&[(0x1400abcd, &77_u32.to_le_bytes())])?;
                objects.insert(id, object);
            }
            Ok(BTreeMap::from([(
                index.root,
                RevisionEdit::Update(objects),
            )]))
        })
        .unwrap();
        for edits in [
            vec![PageEdit::set_level(pages[3].0, 2).unwrap()],
            pages[3..6]
                .iter()
                .enumerate()
                .map(|(i, (sid, _))| PageEdit::move_to(*sid, None, i as u32 + 1).unwrap())
                .collect(),
        ] {
            let written = PageEdit::apply(&source, &edits).unwrap();
            let old_store = Store::parse(&source).unwrap();
            let new_store = Store::parse(&written).unwrap();
            let old = RevisionIndex::parse(&old_store).unwrap();
            let new = RevisionIndex::parse(&new_store).unwrap();
            let old_id = old.spaces[&old.root].labels[&(ExGuid::default(), 1)];
            let new_id = new.spaces[&new.root].labels[&(ExGuid::default(), 1)];
            assert_eq!(
                format!("{:?}", old.resolve(old.root, old_id).unwrap()),
                format!("{:?}", new.resolve(new.root, old_id).unwrap())
            );
            let current = new.resolve(new.root, new_id).unwrap();
            for id in [series_id, copy_id] {
                let object = PropertyObject::from_object(&current.objects[&id]).unwrap();
                let properties = crate::PropertySets::parse(&object.bytes).unwrap();
                assert!(properties.sets[0].iter().any(|field| field.id == 0x1400abcd
                    && matches!(field.value, crate::Value::Bytes(bytes) if bytes == 77_u32.to_le_bytes())));
            }
        }
    }
}
