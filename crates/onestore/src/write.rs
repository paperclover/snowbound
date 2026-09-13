use crate::{
    Chunk, Error, ExGuid, FileType, ObjectData, PropertySets, Reference, RevisionIndex, Store,
    Value,
    store::{crc, transaction_crc},
};

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

#[cfg(test)]
mod tests;

type Result<T> = std::result::Result<T, Error>;

pub(crate) fn fresh_guid() -> Result<[u8; 16]> {
    let mut guid = [0; 16];
    getrandom::fill(&mut guid).map_err(|_| Error {
        offset: 0,
        message: "System random source failed",
    })?;
    guid[7] = (guid[7] & 0x0f) | 0x40;
    guid[8] = (guid[8] & 0x3f) | 0x80;
    Ok(guid)
}

impl ExGuid {
    pub(crate) fn encode(self, data: &mut Vec<u8>) {
        data.extend_from_slice(&self.guid);
        data.extend_from_slice(&self.n.to_le_bytes());
    }
}

pub(crate) fn node(id: u16, reference: Option<Reference>, payload: &[u8]) -> Result<Vec<u8>> {
    let size = 4 + if reference.is_some() { 12 } else { 0 } + payload.len();
    if size > 0x1fff {
        return Err(Error {
            offset: 0,
            message: "File node exceeds the format size limit",
        });
    }
    let (base, chunk) = match reference {
        Some(Reference::Data(chunk)) => (1, Some(chunk)),
        Some(Reference::NodeList(chunk)) => (2, Some(chunk)),
        None => (0, None),
    };
    let header = 0x80000000 | (base << 27) | (u32::try_from(size).unwrap() << 10) | u32::from(id);
    let mut bytes = header.to_le_bytes().to_vec();
    if let Some(chunk) = chunk {
        bytes.extend_from_slice(&chunk.offset.to_le_bytes());
        bytes.extend_from_slice(
            &u32::try_from(chunk.length)
                .map_err(|_| Error {
                    offset: 0,
                    message: "Chunk exceeds the encoded length limit",
                })?
                .to_le_bytes(),
        );
    }
    bytes.extend_from_slice(payload);
    Ok(bytes)
}

pub(crate) fn append(data: &mut Vec<u8>, bytes: &[u8]) -> Result<Chunk> {
    let length = u64::from(u32::try_from(bytes.len()).map_err(|_| Error {
        offset: 0,
        message: "Chunk exceeds the encoded length limit",
    })?);
    data.resize(data.len().next_multiple_of(8), 0);
    let offset = u64::try_from(data.len()).unwrap();
    data.extend_from_slice(bytes);
    Ok(Chunk { offset, length })
}

pub(crate) fn append_list(data: &mut Vec<u8>, id: u32, nodes: &[Vec<u8>]) -> Result<Chunk> {
    let mut bytes = 0xa4567ab1f5f7f4c4_u64.to_le_bytes().to_vec();
    bytes.extend_from_slice(&id.to_le_bytes());
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    for node in nodes {
        bytes.extend_from_slice(node);
    }
    bytes.resize((bytes.len() + 20).next_multiple_of(8) - 20, 0);
    bytes.extend_from_slice(&u64::MAX.to_le_bytes());
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.extend_from_slice(&0x8bc215c38233ba4b_u64.to_le_bytes());
    append(data, &bytes)
}

fn compact(id: ExGuid, table: &BTreeMap<u32, [u8; 16]>) -> Result<[u8; 4]> {
    let index = table
        .iter()
        .find_map(|(index, guid)| (*guid == id.guid).then_some(*index))
        .ok_or(Error {
            offset: 0,
            message: "Object identity is absent from its global ID table",
        })?;
    if id.n > 255 {
        return Err(Error {
            offset: 0,
            message: "Object extension exceeds CompactID capacity",
        });
    }
    Ok(((index << 8) | id.n).to_le_bytes())
}

fn field_length(property: &crate::Property<'_>, set_lengths: &[usize]) -> usize {
    match &property.value {
        Value::NoData => 0,
        Value::Bytes(bytes) => bytes.len() + usize::from(property.id >> 26 & 31 == 7) * 4,
        Value::References { .. } => usize::from(property.id >> 26 & 1 != 0) * 4,
        Value::Sets(children) => {
            let prefix = if property.id >> 26 & 31 == 16 {
                if children.is_empty() { 4 } else { 8 }
            } else {
                0
            };
            prefix + children.clone().map(|i| set_lengths[i]).sum::<usize>()
        }
    }
}

fn property_set_lengths(properties: &PropertySets<'_>) -> Vec<usize> {
    let mut lengths = vec![0; properties.sets.len()];
    for (i, set) in properties.sets.iter().enumerate().rev() {
        lengths[i] =
            2 + set.len() * 4 + set.iter().map(|p| field_length(p, &lengths)).sum::<usize>();
    }
    lengths
}

fn patch_properties(
    blob: &[u8],
    updates: &[(u32, &[u8])],
    inserts: &[(u32, &[u8])],
    nested_references: &[u8],
) -> Result<Vec<u8>> {
    let properties = PropertySets::parse(blob)?;
    let root = &properties.sets[0];
    let ids = properties.root_ids.as_ptr().addr() - blob.as_ptr().addr();
    let ids_end = ids + properties.root_ids.len();
    let body_end = blob.len() - properties.padding.len();
    let set_lengths = property_set_lengths(&properties);
    let mut offsets = Vec::with_capacity(root.len());
    let mut offset = ids_end;
    for property in root {
        offsets.push(offset);
        offset += field_length(property, &set_lengths);
    }
    let mut patches = Vec::new();
    let mut added_ids = Vec::new();
    let mut added_fields = Vec::new();
    let mut added_references = Vec::new();
    let object_header = u32::from_le_bytes(blob[..4].try_into().unwrap());
    let mut object_count = i64::from(object_header & 0xffffff);
    let mut seen = BTreeSet::new();
    for (i, &(property, value)) in updates.iter().chain(inserts).enumerate() {
        if !seen.insert(property & 0x7fffffff) {
            return Err(Error {
                offset: 0,
                message: "Duplicate property update",
            });
        }
        let kind = (property >> 26) & 31;
        let valid = match kind {
            2 => value.is_empty(),
            3..=6 => value.len() == 1 << (kind - 3),
            7 => value.len() < 0x40000000,
            8 => value.len() == 4,
            9 => value.len().is_multiple_of(4) && value.len() / 4 <= 0xffffff,
            // An encoded property-set array is inserted whole; its references follow.
            16 => i >= updates.len() && value.len() >= 4,
            _ => {
                return Err(Error {
                    offset: 0,
                    message: "Property type cannot be patched",
                });
            }
        };
        if !valid || (kind != 2 && property & 0x80000000 != 0) {
            return Err(Error {
                offset: 0,
                message: "Replacement has an invalid property value",
            });
        }
        let matches: Vec<_> = root
            .iter()
            .enumerate()
            .filter(|(_, p)| p.id & 0x7fffffff == property & 0x7fffffff)
            .collect();
        let adding = i >= updates.len();
        if matches.len() != usize::from(!adding) {
            return Err(Error {
                offset: 0,
                message: "Property is missing or duplicated",
            });
        }
        let mut encoded = Vec::new();
        match kind {
            7 => encoded.extend_from_slice(&(value.len() as u32).to_le_bytes()),
            9 => encoded.extend_from_slice(&(value.len() as u32 / 4).to_le_bytes()),
            _ => {}
        }
        if (3..=7).contains(&kind) || kind == 16 {
            encoded.extend_from_slice(value);
        }
        if adding {
            added_ids.extend_from_slice(&property.to_le_bytes());
            added_fields.extend_from_slice(&encoded);
            if (8..=9).contains(&kind) {
                object_count += (value.len() / 4) as i64;
                added_references.extend_from_slice(value);
            }
        } else {
            let (index, previous) = matches[0];
            if kind == 2 && previous.id != property {
                patches.push((
                    ids + index * 4,
                    ids + index * 4 + 4,
                    index,
                    property.to_le_bytes().to_vec(),
                ));
            }
            let start = offsets[index];
            let end = start + field_length(previous, &set_lengths);
            if blob[start..end] != encoded {
                patches.push((start, end, index, encoded));
            }
            if let Value::References { compact_ids, .. } = previous.value {
                object_count += (value.len() / 4) as i64 - (compact_ids.len() / 4) as i64;
                if compact_ids != value {
                    let start = compact_ids.as_ptr().addr() - blob.as_ptr().addr();
                    patches.push((start, start + compact_ids.len(), index, value.to_vec()));
                }
            }
        }
    }
    object_count += (nested_references.len() / 4) as i64;
    if !(0..=0xffffff).contains(&object_count) {
        return Err(Error {
            offset: 0,
            message: "Object reference stream exceeds the format limit",
        });
    }
    let header = (object_header & 0xff000000) | object_count as u32;
    if header != object_header {
        patches.push((0, 4, 0, header.to_le_bytes().to_vec()));
    }
    added_references.extend_from_slice(nested_references);
    if !added_references.is_empty() {
        let end = 4 + (object_header as usize & 0xffffff) * 4;
        patches.push((end, end, usize::MAX, added_references));
    }
    if !inserts.is_empty() {
        let count = u16::try_from(root.len() + inserts.len()).map_err(|_| Error {
            offset: ids - 2,
            message: "Root property count exceeds the format limit",
        })?;
        patches.push((ids - 2, ids, 0, count.to_le_bytes().to_vec()));
        patches.push((ids_end, ids_end, usize::MAX - 1, added_ids));
        patches.push((body_end, body_end, usize::MAX, added_fields));
    }
    if patches.is_empty() {
        return Ok(blob.to_vec());
    }
    patches.sort_by_key(|(start, end, order, _)| (*start, *end, *order));
    let mut changed = Vec::new();
    let mut cursor = 0;
    for (start, end, _, value) in patches {
        if start < cursor {
            return Err(Error {
                offset: start,
                message: "Property patches overlap",
            });
        }
        changed.extend_from_slice(&blob[cursor..start]);
        changed.extend_from_slice(&value);
        cursor = end;
    }
    changed.extend_from_slice(&blob[cursor..body_end]);
    changed.resize(changed.len().next_multiple_of(8), 0);
    PropertySets::parse(&changed)?;
    Ok(changed)
}

pub(crate) struct ObjectEdit<'a> {
    pub object: ExGuid,
    pub updates: &'a [(u32, &'a [u8])],
    pub inserts: &'a [(u32, &'a [u8])],
}

/// Replaces a root-level scalar byte property in the default active revision.
/// The caller supplies its encoded value and is responsible for MS-ONE semantics.
/// Returns a complete file image without I/O; use `commit_file_property` to update an existing file.
pub fn replace_property_bytes(
    source: &[u8],
    space: ExGuid,
    object_id: ExGuid,
    property: u32,
    value: &[u8],
) -> Result<Vec<u8>> {
    if !(3..=7).contains(&((property >> 26) & 31)) {
        return Err(Error {
            offset: 0,
            message: "Property does not contain scalar bytes",
        });
    }
    replace_objects(
        source,
        space,
        &[ObjectEdit {
            object: object_id,
            updates: &[(property, value)],
            inserts: &[],
        }],
    )
}

pub(crate) fn replace_objects(
    source: &[u8],
    space: ExGuid,
    edits: &[ObjectEdit<'_>],
) -> Result<Vec<u8>> {
    write_revision(source, space, |revision| {
        let mut changed = BTreeMap::new();
        for edit in edits {
            if changed.contains_key(&edit.object) {
                return Err(Error {
                    offset: 0,
                    message: "Duplicate object edit",
                });
            }
            let object = revision.objects.get(&edit.object).ok_or(Error {
                offset: 0,
                message: "Object is absent from the active revision",
            })?;
            let ObjectData::Properties(blob) = object.data else {
                return Err(Error {
                    offset: 0,
                    message: "Object does not contain editable properties",
                });
            };
            changed.insert(
                edit.object,
                PropertyObject {
                    jcid: object.jcid,
                    bytes: patch_properties(blob, edit.updates, edit.inserts, &[])?,
                    global_ids: Arc::clone(&object.global_ids),
                },
            );
        }
        Ok(changed)
    })
}

/// The object type OneNote gives embedded picture payload declarations; embedded files
/// use `EMBEDDED_FILE_JCID` with the same declaration shape.
pub(crate) const FILE_DATA_JCID: u32 = 0x80039;
pub(crate) const EMBEDDED_FILE_JCID: u32 = 0x80036;

fn is_file_declaration(jcid: u32) -> bool {
    jcid == FILE_DATA_JCID || jcid == EMBEDDED_FILE_JCID
}

pub(crate) struct PropertyObject {
    pub jcid: u32,
    pub bytes: Vec<u8>,
    pub global_ids: Arc<BTreeMap<u32, [u8; 16]>>,
}

pub(crate) enum RevisionEdit {
    Update(BTreeMap<ExGuid, PropertyObject>),
    Create {
        roots: BTreeMap<u32, ExGuid>,
        objects: BTreeMap<ExGuid, PropertyObject>,
    },
}

impl PropertyObject {
    /// A file-data object declaring an embedded payload by its store identity, or an
    /// external payload by name; `extension` includes its leading dot.
    pub fn file(id: ExGuid, reference: &str, extension: &str) -> Result<Self> {
        if reference.is_empty() || reference.contains('\0') || extension.contains('\0') {
            return Err(Error {
                offset: 0,
                message: "File-data references and extensions are nonempty and contain no NUL",
            });
        }
        let mut bytes = Vec::new();
        for text in [reference, extension] {
            let encoded: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
            bytes.extend_from_slice(&u32::try_from(encoded.len()).unwrap().to_le_bytes());
            bytes.extend_from_slice(&encoded);
        }
        Ok(Self {
            jcid: FILE_DATA_JCID,
            bytes,
            global_ids: Arc::new(BTreeMap::from([(0, id.guid)])),
        })
    }

    /// The reference and extension of a file-data declaration built by `file`.
    fn file_declaration(&self) -> Result<(&[u8], &[u8])> {
        let malformed = || Error {
            offset: 0,
            message: "Malformed file-data declaration",
        };
        let mut rest = self.bytes.as_slice();
        let mut parts = Vec::new();
        for _ in 0..2 {
            let length = usize::try_from(u32::from_le_bytes(
                rest.get(..4).ok_or_else(malformed)?.try_into().unwrap(),
            ))
            .map_err(|_| malformed())?;
            let bytes = rest.get(4..4 + length).ok_or_else(malformed)?;
            parts.push(bytes);
            rest = &rest[4 + length..];
        }
        if !rest.is_empty() {
            return Err(malformed());
        }
        Ok((parts[0], parts[1]))
    }

    pub fn from_object(object: &crate::Object<'_>) -> Result<Self> {
        let ObjectData::Properties(bytes) = object.data else {
            return Err(Error {
                offset: 0,
                message: "Object does not contain editable properties",
            });
        };
        Ok(Self {
            jcid: object.jcid,
            bytes: bytes.to_vec(),
            global_ids: Arc::clone(&object.global_ids),
        })
    }

    pub fn set(&mut self, values: &[(u32, &[u8])]) -> Result<()> {
        let properties = PropertySets::parse(&self.bytes)?;
        let (updates, inserts): (Vec<_>, Vec<_>) = values.iter().copied().partition(|(id, _)| {
            properties.sets[0]
                .iter()
                .any(|p| p.id & 0x7fffffff == id & 0x7fffffff)
        });
        self.bytes = patch_properties(&self.bytes, &updates, &inserts, &[])?;
        Ok(())
    }

    /// Replaces a property-set array such as note tags: each set lists scalar values
    /// inline, while an object reference (a kind-8 identity) is supplied as the compact
    /// identity `reference` produced and joins the object's reference stream in order.
    pub fn set_sets(&mut self, id: u32, element: u32, sets: &[Vec<(u32, Vec<u8>)>]) -> Result<()> {
        if (id >> 26) & 31 != 16 || (element >> 26) & 31 != 17 {
            return Err(Error {
                offset: 0,
                message: "Property-set arrays need array and element identifiers",
            });
        }
        self.remove(&[id])?;
        if sets.is_empty() {
            return Ok(());
        }
        let mut encoded = (sets.len() as u32).to_le_bytes().to_vec();
        encoded.extend_from_slice(&element.to_le_bytes());
        let mut references = Vec::new();
        for set in sets {
            encoded.extend_from_slice(&u16::try_from(set.len()).unwrap().to_le_bytes());
            for (field, _) in set {
                encoded.extend_from_slice(&field.to_le_bytes());
            }
            for (field, value) in set {
                match (field >> 26) & 31 {
                    3..=6 => {
                        assert_eq!(value.len(), 1 << (((field >> 26) & 31) - 3));
                        encoded.extend_from_slice(value);
                    }
                    7 => {
                        encoded.extend_from_slice(&(value.len() as u32).to_le_bytes());
                        encoded.extend_from_slice(value);
                    }
                    8 => {
                        assert_eq!(value.len(), 4);
                        references.extend_from_slice(value);
                    }
                    _ => {
                        return Err(Error {
                            offset: 0,
                            message: "Property-set arrays hold scalars and single references",
                        });
                    }
                }
            }
        }
        self.bytes = patch_properties(&self.bytes, &[], &[(id, &encoded)], &references)?;
        Ok(())
    }

    pub fn remove(&mut self, ids: &[u32]) -> Result<()> {
        let properties = PropertySets::parse(&self.bytes)?;
        let removed = |id: u32| {
            ids.iter()
                .any(|wanted| id & 0x7fffffff == wanted & 0x7fffffff)
        };
        if !properties.sets[0].iter().any(|p| removed(p.id)) {
            return Ok(());
        }
        let lengths = property_set_lengths(&properties);
        let mut offset = properties.root_ids.as_ptr().addr() - self.bytes.as_ptr().addr()
            + properties.root_ids.len();
        let mut retained_ids = Vec::new();
        let mut fields = Vec::new();
        let mut references: [Vec<u8>; 3] = std::array::from_fn(|_| Vec::new());
        for property in &properties.sets[0] {
            let end = offset + field_length(property, &lengths);
            if !removed(property.id) {
                retained_ids.extend_from_slice(&property.id.to_le_bytes());
                fields.extend_from_slice(&self.bytes[offset..end]);
                let mut pending = vec![property];
                while let Some(field) = pending.pop() {
                    match &field.value {
                        Value::References {
                            stream,
                            compact_ids,
                        } => {
                            let index = match stream {
                                crate::IdStream::Objects => 0,
                                crate::IdStream::ObjectSpaces => 1,
                                crate::IdStream::Contexts => 2,
                            };
                            references[index].extend_from_slice(compact_ids);
                        }
                        Value::Sets(children) => {
                            for child in children.clone().rev() {
                                pending.extend(properties.sets[child].iter().rev());
                            }
                        }
                        _ => {}
                    }
                }
            }
            offset = end;
        }
        let mut cursor = crate::bytes::Cursor {
            bytes: &self.bytes,
            offset: 0,
        };
        let streams = crate::properties::reference_streams(&mut cursor)?;
        let mut bytes = Vec::new();
        for (stream, retained) in streams.iter().zip(&references) {
            if stream.offset == 0 {
                continue;
            }
            let header = u32::from_le_bytes(
                self.bytes[stream.offset - 4..stream.offset]
                    .try_into()
                    .unwrap(),
            );
            let count = u32::try_from(retained.len() / 4).unwrap();
            bytes.extend_from_slice(&((header & 0xff000000) | count).to_le_bytes());
            bytes.extend_from_slice(retained);
        }
        bytes.extend_from_slice(&u16::try_from(retained_ids.len() / 4).unwrap().to_le_bytes());
        bytes.extend_from_slice(&retained_ids);
        bytes.extend_from_slice(&fields);
        bytes.resize(bytes.len().next_multiple_of(8), 0);
        PropertySets::parse(&bytes)?;
        self.bytes = bytes;
        Ok(())
    }

    pub fn reference(&mut self, id: ExGuid) -> Result<[u8; 4]> {
        if !self.global_ids.values().any(|guid| *guid == id.guid) {
            let mut index = 0;
            for key in self.global_ids.keys() {
                if *key != index {
                    break;
                }
                index += 1;
            }
            if index >= 0xffffff || id.guid == [0; 16] {
                return Err(Error {
                    offset: 0,
                    message: "Object identity cannot be added to the global ID table",
                });
            }
            Arc::make_mut(&mut self.global_ids).insert(index, id.guid);
        }
        compact(id, &self.global_ids)
    }

    pub fn copy_property(&mut self, source: &Self, id: u32) -> Result<()> {
        let properties = PropertySets::parse(&source.bytes)?;
        let mut target = Self {
            jcid: self.jcid,
            bytes: self.bytes.clone(),
            global_ids: Arc::clone(&self.global_ids),
        };
        target.remove(&[id])?;
        let lengths = property_set_lengths(&properties);
        let mut offset = properties.root_ids.as_ptr().addr() - source.bytes.as_ptr().addr()
            + properties.root_ids.len();
        let mut selected = None;
        for property in &properties.sets[0] {
            let end = offset + field_length(property, &lengths);
            if property.id & 0x7fffffff == id & 0x7fffffff {
                selected = Some((property, &source.bytes[offset..end]));
                break;
            }
            offset = end;
        }
        if let Some((property, field)) = selected {
            let mut added: [Vec<u8>; 3] = std::array::from_fn(|_| Vec::new());
            let mut pending = vec![property];
            while let Some(property) = pending.pop() {
                match &property.value {
                    Value::References {
                        stream,
                        compact_ids,
                    } => {
                        let index = match stream {
                            crate::IdStream::Objects => 0,
                            crate::IdStream::ObjectSpaces => 1,
                            crate::IdStream::Contexts => 2,
                        };
                        let mut cursor = crate::bytes::Cursor {
                            bytes: compact_ids,
                            offset: 0,
                        };
                        while !cursor.bytes.is_empty() {
                            let id = cursor.compact(&source.global_ids)?;
                            added[index].extend_from_slice(&target.reference(id)?);
                        }
                    }
                    Value::Sets(children) => {
                        for child in children.clone().rev() {
                            pending.extend(properties.sets[child].iter().rev());
                        }
                    }
                    _ => {}
                }
            }
            let old = PropertySets::parse(&target.bytes)?;
            let count = u16::try_from(old.sets[0].len() + 1).map_err(|_| Error {
                offset: 0,
                message: "Root property count exceeds the format limit",
            })?;
            let streams = crate::properties::reference_streams(&mut crate::bytes::Cursor {
                bytes: &target.bytes,
                offset: 0,
            })?;
            let stream_count = (0..3)
                .rev()
                .find(|i| streams[*i].offset != 0 || !added[*i].is_empty())
                .unwrap()
                + 1;
            let mut bytes = Vec::new();
            for i in 0..stream_count {
                let stream = &streams[i];
                let count = u32::try_from((stream.bytes.len() + added[i].len()) / 4)
                    .ok()
                    .filter(|n| *n <= 0xffffff)
                    .ok_or(Error {
                        offset: 0,
                        message: "Reference stream exceeds the format limit",
                    })?;
                let reserved = if stream.offset == 0 {
                    0
                } else {
                    u32::from_le_bytes(
                        target.bytes[stream.offset - 4..stream.offset]
                            .try_into()
                            .unwrap(),
                    ) & 0x3f000000
                };
                let flags = match (i, stream_count) {
                    (0, 1) => 0x80000000,
                    (0 | 1, 3) => 0x40000000,
                    _ => 0,
                };
                bytes.extend_from_slice(&(count | reserved | flags).to_le_bytes());
                bytes.extend_from_slice(stream.bytes);
                bytes.extend_from_slice(&added[i]);
            }
            bytes.extend_from_slice(&count.to_le_bytes());
            bytes.extend_from_slice(old.root_ids);
            bytes.extend_from_slice(&property.id.to_le_bytes());
            let start =
                old.root_ids.as_ptr().addr() - target.bytes.as_ptr().addr() + old.root_ids.len();
            bytes.extend_from_slice(&target.bytes[start..target.bytes.len() - old.padding.len()]);
            bytes.extend_from_slice(field);
            bytes.resize(bytes.len().next_multiple_of(8), 0);
            PropertySets::parse(&bytes)?;
            target.bytes = bytes;
        }
        *self = target;
        Ok(())
    }
}

pub(crate) fn write_revision(
    source: &[u8],
    space: ExGuid,
    edit: impl FnOnce(&crate::ResolvedRevision<'_>) -> Result<BTreeMap<ExGuid, PropertyObject>>,
) -> Result<Vec<u8>> {
    write_revision_with_payloads(source, space, &[], edit)
}

pub(crate) fn write_revision_with_payloads(
    source: &[u8],
    space: ExGuid,
    payloads: &[([u8; 16], &[u8])],
    edit: impl FnOnce(&crate::ResolvedRevision<'_>) -> Result<BTreeMap<ExGuid, PropertyObject>>,
) -> Result<Vec<u8>> {
    write_revisions_with_payloads(source, payloads, |index| {
        let rid = index.active(space)?;
        let revision = index.resolve(space, rid)?;
        Ok(BTreeMap::from([(
            space,
            RevisionEdit::Update(edit(&revision)?),
        )]))
    })
}

fn append_fragment(
    source: &[u8],
    output: &mut Vec<u8>,
    list: &crate::NodeList,
    nodes: &[Vec<u8>],
) -> Result<(u32, usize)> {
    let last_fragment = *list.fragments.last().unwrap();
    let list_start = usize::try_from(last_fragment.offset).unwrap();
    let list_id = u32::from_le_bytes(source[list_start + 8..list_start + 12].try_into().unwrap());
    let chunk = append_list(output, list_id, nodes)?;
    let start = usize::try_from(chunk.offset).unwrap();
    let sequence = u32::try_from(list.fragments.len()).map_err(|_| Error {
        offset: list_start,
        message: "File-node fragment sequences are exhausted",
    })?;
    output[start + 12..start + 16].copy_from_slice(&sequence.to_le_bytes());
    let last_node = list.nodes.last().unwrap();
    let node_header = u32::from_le_bytes(
        source[last_node.offset..last_node.offset + 4]
            .try_into()
            .unwrap(),
    );
    let nodes_end = last_node.offset + usize::try_from((node_header >> 10) & 0x1fff).unwrap();
    let tail = list_start + usize::try_from(last_fragment.length).unwrap() - 20;
    if tail - nodes_end >= 4 {
        output[nodes_end..nodes_end + 4].copy_from_slice(&node(0xff, None, &[])?);
    }
    output[tail..tail + 8].copy_from_slice(&chunk.offset.to_le_bytes());
    output[tail + 8..tail + 12].copy_from_slice(&(chunk.length as u32).to_le_bytes());

    Ok((list_id, list.nodes.len() + nodes.len()))
}

pub(crate) fn write_revisions(
    source: &[u8],
    edit: impl FnOnce(&RevisionIndex<'_>) -> Result<BTreeMap<ExGuid, RevisionEdit>>,
) -> Result<Vec<u8>> {
    write_revisions_with_payloads(source, &[], edit)
}

/// `write_revisions` that also stores embedded payloads: each becomes a file-data store
/// object referenced from the root file node list under its identity, as OneNote embeds
/// pictures and attachments.
pub(crate) fn write_revisions_with_payloads(
    source: &[u8],
    payloads: &[([u8; 16], &[u8])],
    edit: impl FnOnce(&RevisionIndex<'_>) -> Result<BTreeMap<ExGuid, RevisionEdit>>,
) -> Result<Vec<u8>> {
    let store = Store::parse(source)?;
    let is_section = store.header.file_type == FileType::Section;
    if !store.checksum_mismatches.is_empty() {
        return Err(Error {
            offset: store.checksum_mismatches[0],
            message: "Cannot write a file with transaction checksum damage",
        });
    }
    let index = RevisionIndex::parse(&store)?;
    index.validate_current()?;
    let changes = edit(&index)?;
    let mut output = source.to_vec();
    // Native files reserve 1 KiB per transaction-log fragment; a fragment that ends the file
    // keeps that room before the first new chunk.
    let tail = store.transaction_fragments.last().unwrap().chunk;
    let reserved = usize::try_from(tail.offset + tail.length.max(1024) + 32).unwrap();
    if usize::try_from(tail.offset + tail.length)
        .unwrap()
        .next_multiple_of(8)
        >= output.len()
    {
        output.resize(reserved, 0);
    }
    let mut maximum = store
        .transaction_fragments
        .iter()
        .flat_map(|fragment| fragment.entries.chunks_exact(8))
        .map(|entry| u32::from_le_bytes(entry[..4].try_into().unwrap()))
        .max()
        .unwrap();
    let mut allocate_list = || {
        maximum = maximum.checked_add(1).ok_or(Error {
            offset: 0,
            message: "File-node list identities are exhausted",
        })?;
        Ok::<_, Error>(maximum)
    };
    let mut counts = Vec::new();
    let mut root_nodes = Vec::new();
    for (space, change) in changes {
        let (rid, mut revision, mut replacements) = match change {
            RevisionEdit::Update(objects) => {
                let rid = index.active(space)?;
                (Some(rid), index.resolve(space, rid)?, objects)
            }
            RevisionEdit::Create { roots, objects } => {
                if !is_section || space.guid == [0; 16] || index.spaces.contains_key(&space) {
                    return Err(Error {
                        offset: 0,
                        message: "Choose a new object-space identity in a section file",
                    });
                }
                if roots.is_empty() || objects.is_empty() {
                    return Err(Error {
                        offset: 0,
                        message: "New object space needs roots and objects",
                    });
                }
                (
                    None,
                    crate::ResolvedRevision {
                        roots,
                        objects: BTreeMap::new(),
                    },
                    objects,
                )
            }
        };
        let reachable = if rid.is_some() {
            revision.reachable()?
        } else {
            BTreeSet::new()
        };
        for (id, replacement) in &replacements {
            if let Some(object) = revision.objects.get(id) {
                if object.jcid & 0x100000 != 0 {
                    return Err(Error {
                        offset: 0,
                        message: "Read-only object requires a new identity",
                    });
                }
                if replacement.jcid != object.jcid
                    || !matches!(object.data, ObjectData::Properties(_))
                {
                    return Err(Error {
                        offset: 0,
                        message: "An existing object's type cannot be changed",
                    });
                }
            } else if !is_section {
                return Err(Error {
                    offset: 0,
                    message: "New objects require a section file",
                });
            }
            if replacement.global_ids.keys().any(|i| *i > 0xffffff) {
                return Err(Error {
                    offset: 0,
                    message: "Invalid property object declaration",
                });
            }
            compact(*id, &replacement.global_ids)?;
            if is_file_declaration(replacement.jcid) {
                if !is_section {
                    return Err(Error {
                        offset: 0,
                        message: "File-data objects require a section file",
                    });
                }
                replacement.file_declaration()?;
            } else if replacement.jcid & 0x20000 == 0 {
                return Err(Error {
                    offset: 0,
                    message: "Invalid property object declaration",
                });
            } else {
                PropertySets::parse(&replacement.bytes)?;
            }
        }
        // Native coalescing of duplicate readonly styles can leave dangling references.
        let mut aliases = BTreeMap::new();
        for (id, replacement) in &replacements {
            if revision.objects.contains_key(id)
                || replacement.jcid & 0x100000 == 0
                || PropertySets::parse(&replacement.bytes)?
                    .sets
                    .iter()
                    .flatten()
                    .any(|p| matches!(p.value, Value::References { .. }))
            {
                continue;
            }
            let existing = revision.objects.iter().find_map(|(other, object)| {
                (reachable.contains(other)
                    && object.jcid == replacement.jcid
                    && object.data == ObjectData::Properties(&replacement.bytes))
                .then_some(*other)
            });
            let existing = existing.or_else(|| {
                replacements.range(..id).find_map(|(other, object)| {
                    (object.jcid == replacement.jcid && object.bytes == replacement.bytes)
                        .then_some(*aliases.get(other).unwrap_or(other))
                })
            });
            if let Some(existing) = existing {
                aliases.insert(*id, existing);
            }
        }
        for id in aliases.keys() {
            replacements.remove(id);
        }
        for object in replacements.values_mut() {
            if is_file_declaration(object.jcid) {
                continue;
            }
            let mut remapped = Vec::new();
            for property in PropertySets::parse(&object.bytes)?.sets.iter().flatten() {
                if let Value::References {
                    stream: crate::IdStream::Objects,
                    compact_ids,
                } = property.value
                {
                    for bytes in compact_ids.chunks_exact(4) {
                        let offset = bytes.as_ptr().addr() - object.bytes.as_ptr().addr();
                        let id =
                            crate::bytes::Cursor { bytes, offset }.compact(&object.global_ids)?;
                        if let Some(existing) = aliases.get(&id) {
                            remapped.push((offset, *existing));
                        }
                    }
                }
            }
            for (offset, id) in remapped {
                let reference = object.reference(id)?;
                object.bytes[offset..offset + 4].copy_from_slice(&reference);
            }
        }
        replacements.retain(|id, replacement| {
            // Detached objects must pass the final reachability check even when unchanged.
            !reachable.contains(id)
                || !revision.objects.get(id).is_some_and(|object| {
                    object.data == ObjectData::Properties(&replacement.bytes)
                        && object.global_ids == replacement.global_ids
                })
        });
        if replacements.is_empty() {
            continue;
        }
        let mut changed: BTreeSet<_> = replacements.keys().copied().collect();
        for (id, replacement) in &replacements {
            let data = if is_file_declaration(replacement.jcid) {
                let (reference, extension) = replacement.file_declaration()?;
                ObjectData::File {
                    reference,
                    extension,
                }
            } else {
                ObjectData::Properties(&replacement.bytes)
            };
            revision.objects.insert(
                *id,
                crate::Object {
                    jcid: replacement.jcid,
                    reference_count: 0,
                    data,
                    global_ids: Arc::clone(&replacement.global_ids),
                },
            );
        }
        let incoming = revision.reference_counts()?;
        if replacements.keys().any(|id| !incoming.contains_key(id)) {
            return Err(Error {
                offset: 0,
                message: "Edited object is not reachable in the resulting revision",
            });
        }
        for (id, object) in &mut revision.objects {
            if reachable.contains(id) || incoming.contains_key(id) {
                let count = incoming.get(id).copied().unwrap_or(0);
                if object.reference_count != count {
                    object.reference_count = count;
                    changed.insert(*id);
                }
            }
        }

        // Native cold-open fails on long dependency chains; cap their depth at 512.
        let checkpoint = rid.is_none()
            || std::iter::successors(rid, |id| index.spaces[&space].revisions[id].dependency)
                .nth(511)
                .is_some();
        let selected: Vec<_> = revision
            .objects
            .iter()
            .filter(|(id, _)| checkpoint || changed.contains(id))
            .collect();
        let toc_table = if checkpoint && !is_section {
            if selected.iter().any(|(_, object)| {
                object.jcid != 0x20001 || !matches!(object.data, ObjectData::Properties(_))
            }) {
                return Err(Error {
                    offset: 0,
                    message: "TOC checkpoint requires table-of-contents property objects",
                });
            }
            let guids: BTreeSet<_> = selected
                .iter()
                .flat_map(|(_, object)| object.global_ids.values().copied())
                .collect();
            if guids.len() > 0x1000000 {
                return Err(Error {
                    offset: 0,
                    message: "Global ID table exceeds CompactID capacity",
                });
            }
            Some(
                guids
                    .into_iter()
                    .enumerate()
                    .map(|(i, guid)| (u32::try_from(i).unwrap(), guid))
                    .collect::<BTreeMap<_, _>>(),
            )
        } else {
            None
        };
        let mut groups = BTreeMap::<_, Vec<_>>::new();
        for (id, object) in selected {
            groups
                .entry(toc_table.as_ref().unwrap_or(&object.global_ids))
                .or_default()
                .push((*id, object));
        }
        let new_rid = ExGuid {
            guid: fresh_guid()?,
            n: 1,
        };
        let mut start = Vec::new();
        new_rid.encode(&mut start);
        if checkpoint {
            ExGuid::default()
        } else {
            rid.unwrap()
        }
        .encode(&mut start);
        if !is_section {
            start.extend_from_slice(&0_u64.to_le_bytes());
        }
        start.extend_from_slice(&1_u32.to_le_bytes());
        start.extend_from_slice(&0_u16.to_le_bytes());
        let mut manifest = Vec::new();
        if rid.is_none() {
            let mut payload = Vec::new();
            space.encode(&mut payload);
            payload.extend_from_slice(&0_u32.to_le_bytes());
            manifest.push(node(0x14, None, &payload)?);
        }
        manifest.push(node(if is_section { 0x1e } else { 0x1b }, None, &start)?);
        for (table, objects) in groups {
            let mut payload = Vec::new();
            let mut group = if is_section {
                ExGuid {
                    guid: fresh_guid()?,
                    n: 1,
                }
                .encode(&mut payload);
                vec![node(0xb4, None, &payload)?, node(0x22, None, &[])?]
            } else {
                vec![node(0x21, None, &[0])?]
            };
            for (id, guid) in table {
                let mut entry = id.to_le_bytes().to_vec();
                entry.extend_from_slice(guid);
                group.push(node(0x24, None, &entry)?);
            }
            group.push(node(0x28, None, &[])?);
            let mut override_crc = u32::MAX;
            for (id, object) in objects {
                let mut declaration = compact(id, table)?.to_vec();
                override_crc = crc(
                    override_crc,
                    &object.reference_count.to_le_bytes(),
                    FileType::Section,
                );
                match object.data {
                    ObjectData::File {
                        reference,
                        extension,
                    } => {
                        declaration.extend_from_slice(&object.jcid.to_le_bytes());
                        declaration.extend_from_slice(&object.reference_count.to_le_bytes());
                        for bytes in [reference, extension] {
                            declaration.extend_from_slice(
                                &u32::try_from(bytes.len() / 2).unwrap().to_le_bytes(),
                            );
                            declaration.extend_from_slice(bytes);
                        }
                        group.push(node(0x73, None, &declaration)?);
                    }
                    ObjectData::Properties(bytes) => {
                        let references = object.references()?;
                        let flags = u8::from(!references.objects.is_empty())
                            | (u8::from(
                                !references.object_spaces.is_empty()
                                    || !references.contexts.is_empty(),
                            ) << 1);
                        let data = if toc_table.is_some() {
                            let mut mapped = bytes.to_vec();
                            for property in PropertySets::parse(bytes)?.sets.iter().flatten() {
                                if let Value::References { compact_ids, .. } = property.value {
                                    let offset =
                                        compact_ids.as_ptr().addr() - bytes.as_ptr().addr();
                                    for (i, value) in compact_ids.chunks_exact(4).enumerate() {
                                        let value = u32::from_le_bytes(value.try_into().unwrap());
                                        let target = ExGuid {
                                            guid: object.global_ids[&(value >> 8)],
                                            n: value & 255,
                                        };
                                        mapped[offset + i * 4..offset + i * 4 + 4]
                                            .copy_from_slice(&compact(target, table)?);
                                    }
                                }
                            }
                            append(&mut output, &mapped)?
                        } else if replacements.contains_key(&id) {
                            append(&mut output, bytes)?
                        } else {
                            Chunk {
                                offset: u64::try_from(
                                    bytes.as_ptr().addr() - source.as_ptr().addr(),
                                )
                                .unwrap(),
                                length: u64::try_from(bytes.len()).unwrap(),
                            }
                        };
                        if is_section {
                            declaration.extend_from_slice(&object.jcid.to_le_bytes());
                            declaration.push(flags);
                        } else if checkpoint {
                            let body = 1_u64 | (u64::from(flags & 1) << 16);
                            declaration.extend_from_slice(&body.to_le_bytes()[..6]);
                        } else {
                            declaration.extend_from_slice(&u32::from(flags).to_le_bytes());
                        }
                        declaration.extend_from_slice(&object.reference_count.to_le_bytes());
                        let readonly = object.jcid & 0x100000 != 0;
                        if readonly {
                            declaration.extend_from_slice(&md5::compute(bytes).0);
                        }
                        group.push(node(
                            if is_section {
                                if readonly { 0xc5 } else { 0xa5 }
                            } else if checkpoint {
                                0x2e
                            } else {
                                0x42
                            },
                            Some(Reference::Data(data)),
                            &declaration,
                        )?);
                    }
                    ObjectData::Encrypted(_) => {
                        return Err(Error {
                            offset: 0,
                            message: "Encrypted objects cannot be checkpointed",
                        });
                    }
                }
            }
            if is_section {
                group.push(node(0xb8, None, &[])?);
                let group_id = allocate_list()?;
                let chunk = append_list(&mut output, group_id, &group)?;
                counts.push((group_id, group.len()));
                manifest.push(node(0xb0, Some(Reference::NodeList(chunk)), &payload)?);
                let mut overrides = vec![0; 8];
                overrides.extend_from_slice(&(!override_crc).to_le_bytes());
                manifest.push(node(
                    0x84,
                    Some(Reference::Data(Chunk {
                        offset: u64::MAX,
                        length: 0,
                    })),
                    &overrides,
                )?);
            } else {
                manifest.extend(group);
            }
        }
        if checkpoint {
            for (role, id) in &revision.roots {
                let mut payload = Vec::new();
                if is_section {
                    id.encode(&mut payload);
                } else {
                    payload.extend_from_slice(&compact(*id, toc_table.as_ref().unwrap())?);
                }
                payload.extend_from_slice(&role.to_le_bytes());
                manifest.push(node(if is_section { 0x5a } else { 0x59 }, None, &payload)?);
            }
        }
        manifest.push(node(0x1c, None, &[])?);
        if rid.is_none() {
            let list_id = allocate_list()?;
            let chunk = append_list(&mut output, list_id, &manifest)?;
            counts.push((list_id, manifest.len()));
            let mut payload = Vec::new();
            space.encode(&mut payload);
            let nodes = vec![
                node(0xc, None, &payload)?,
                node(0x10, Some(Reference::NodeList(chunk)), &[])?,
            ];
            let list_id = allocate_list()?;
            let chunk = append_list(&mut output, list_id, &nodes)?;
            counts.push((list_id, nodes.len()));
            root_nodes.push(node(8, Some(Reference::NodeList(chunk)), &payload)?);
        } else {
            let root = store.list(store.header.root)?;
            let space_node = root
                .nodes
                .iter()
                .find(|node| node.id == 8 && node.fields(&store).exguid() == Ok(space))
                .ok_or(Error {
                    offset: 0,
                    message: "Object space is absent from the root list",
                })?;
            let space_list = space_node.referenced_list(&store)?;
            let revision_node = space_list.iter().rfind(|node| node.id == 0x10).unwrap();
            let Some(Reference::NodeList(manifest_reference)) = revision_node.reference else {
                unreachable!()
            };
            let revision_list = store.list(manifest_reference)?;
            counts.push(append_fragment(
                source,
                &mut output,
                revision_list,
                &manifest,
            )?);
        }
    }
    let mut data_nodes = Vec::new();
    for (guid, payload) in payloads {
        if !is_section {
            return Err(Error {
                offset: 0,
                message: "Embedded payloads require a section file",
            });
        }
        let mut blob = vec![
            0xe7, 0x16, 0xe3, 0xbd, 0x65, 0x26, 0x11, 0x45, 0xa4, 0xc4, 0x8d, 0x4d, 0x0b, 0x7a,
            0x9e, 0xac,
        ];
        blob.extend_from_slice(&(payload.len() as u64).to_le_bytes());
        blob.extend_from_slice(&[0; 12]);
        blob.extend_from_slice(payload);
        blob.resize(blob.len().next_multiple_of(8), 0);
        blob.extend_from_slice(&[
            0x22, 0xa7, 0xfb, 0x71, 0x79, 0x0f, 0x0b, 0x4a, 0xbb, 0x13, 0x89, 0x92, 0x56, 0x42,
            0x6b, 0x24,
        ]);
        let chunk = append(&mut output, &blob)?;
        data_nodes.push(node(0x94, Some(Reference::Data(chunk)), guid)?);
    }
    if !data_nodes.is_empty() {
        // Payload declarations live in the file-data store list the root list references.
        let root = store.list(store.header.root)?;
        match root.nodes.iter().find(|node| node.id == 0x90) {
            Some(reference) => {
                let Some(Reference::NodeList(chunk)) = reference.reference else {
                    return Err(Error {
                        offset: reference.offset,
                        message: "File-data store reference lacks a list",
                    });
                };
                counts.push(append_fragment(
                    source,
                    &mut output,
                    store.list(chunk)?,
                    &data_nodes,
                )?);
            }
            None => {
                let list_id = allocate_list()?;
                let chunk = append_list(&mut output, list_id, &data_nodes)?;
                counts.push((list_id, data_nodes.len()));
                root_nodes.push(node(0x90, Some(Reference::NodeList(chunk)), &[])?);
            }
        }
    }
    if !root_nodes.is_empty() {
        counts.push(append_fragment(
            source,
            &mut output,
            store.list(store.header.root)?,
            &root_nodes,
        )?);
    }
    if counts.is_empty() {
        return Ok(source.to_vec());
    }

    let transactions = store.header.transaction_count.checked_add(1).ok_or(Error {
        offset: 96,
        message: "Transaction counter is exhausted",
    })?;
    let changed_bytes = store.header.transaction_count ^ transactions;
    let commit_byte = (31 - changed_bytes.leading_zeros()) / 8;
    let ceiling = transactions | ((1_u32 << (commit_byte * 8)) - 1);
    let mut entries = Vec::new();
    for (id, count) in counts {
        entries.extend_from_slice(&id.to_le_bytes());
        entries.extend_from_slice(
            &u32::try_from(count)
                .map_err(|_| Error {
                    offset: 0,
                    message: "File-node count exceeds the format limit",
                })?
                .to_le_bytes(),
        );
    }
    let mut checksum = if is_section { u32::MAX } else { 0 };
    for fragment in &store.transaction_fragments {
        checksum = transaction_crc(
            checksum,
            fragment.entries,
            store.header.file_type,
            fragment.entries.len() + 20 > fragment.chunk.length as usize,
        );
    }
    let last_log = store.transaction_fragments.last().unwrap();
    let mut crc_used = last_log.entries.len();
    let mut crc_capacity = (last_log.chunk.length as usize - 12) & !7;
    let mut advance_crc = |checksum, entry: &[u8]| {
        if crc_used == crc_capacity {
            crc_used = 0;
            crc_capacity = 1008;
        }
        crc_used += 8;
        transaction_crc(
            checksum,
            entry,
            store.header.file_type,
            crc_used == crc_capacity,
        )
    };
    for entry in entries.chunks_exact(8) {
        checksum = advance_crc(checksum, entry);
    }
    for _ in transactions..=ceiling {
        let sentinel_crc = if is_section { !checksum } else { checksum };
        entries.extend_from_slice(&1_u32.to_le_bytes());
        entries.extend_from_slice(&sentinel_crc.to_le_bytes());
        checksum = advance_crc(checksum, &entries[entries.len() - 8..]);
    }
    let mut log_chunk = last_log.chunk;
    let mut used = last_log.entries.len();
    let mut remaining = entries.as_slice();
    while !remaining.is_empty() {
        let capacity = usize::try_from(log_chunk.length)
            .unwrap()
            .checked_sub(12)
            .map(|size| size & !7)
            .filter(|capacity| *capacity >= used)
            .ok_or(Error {
                offset: usize::try_from(log_chunk.offset).unwrap(),
                message: "Transaction fragment has no room for continuation",
            })?;
        let offset = usize::try_from(log_chunk.offset).unwrap();
        let size = remaining.len().min(capacity - used);
        output[offset + used..offset + used + size].copy_from_slice(&remaining[..size]);
        remaining = &remaining[size..];
        if !remaining.is_empty() {
            let mut fragment = vec![0; 1024];
            fragment[1008..1016].copy_from_slice(&u64::MAX.to_le_bytes());
            let next = append(&mut output, &fragment)?;
            output[offset + capacity..offset + capacity + 8]
                .copy_from_slice(&next.offset.to_le_bytes());
            output[offset + capacity + 8..offset + capacity + 12]
                .copy_from_slice(&(next.length as u32).to_le_bytes());
            log_chunk = next;
            used = 0;
        }
    }
    output[96..100].copy_from_slice(&transactions.to_le_bytes());
    let length = u64::try_from(output.len()).unwrap();
    output[196..204].copy_from_slice(&length.to_le_bytes());
    output[212..228].copy_from_slice(&fresh_guid()?);
    output[236..252].copy_from_slice(&fresh_guid()?);
    let generation = store.header.generation.checked_add(1).ok_or(Error {
        offset: 228,
        message: "File generation counter is exhausted",
    })?;
    output[228..236].copy_from_slice(&generation.to_le_bytes());
    let written = Store::parse(&output)?;
    let written_index = RevisionIndex::parse(&written)?;
    written_index.validate_current()?;
    Ok(output)
}
