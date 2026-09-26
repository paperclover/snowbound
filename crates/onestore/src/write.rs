use crate::{
    Chunk, Error, ExGuid, FileType, Node, ObjectData, PropertySets, Reference, RevisionIndex,
    Store, Value,
    bytes::Cursor,
    store::{Fragment, ListTail, StoreState, crc, transaction_crc},
};

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

#[cfg(test)]
mod tests;

type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
thread_local! {
    /// When set, `fresh_guid` counts from it instead of drawing randomness, so tests can
    /// compare builds byte for byte.
    pub(crate) static GUIDS: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

pub(crate) fn fresh_guid() -> Result<[u8; 16]> {
    let mut guid = [0; 16];
    getrandom::fill(&mut guid).map_err(|_| Error {
        offset: 0,
        message: "System random source failed",
    })?;
    #[cfg(test)]
    if let Some(next) = GUIDS.get() {
        GUIDS.set(Some(next + 1));
        guid[..8].copy_from_slice(&next.to_le_bytes());
        guid[8..].copy_from_slice(&0x5eed_u64.to_le_bytes());
    }
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
    append(data, &fragment(id, 0, nodes))
}

/// The file-node list fragment `sequence` of list `id`, holding `nodes` and no successor.
fn fragment(id: u32, sequence: u32, nodes: &[Vec<u8>]) -> Vec<u8> {
    let mut bytes = 0xa4567ab1f5f7f4c4_u64.to_le_bytes().to_vec();
    bytes.extend_from_slice(&id.to_le_bytes());
    bytes.extend_from_slice(&sequence.to_le_bytes());
    for node in nodes {
        bytes.extend_from_slice(node);
    }
    bytes.resize((bytes.len() + 20).next_multiple_of(8) - 20, 0);
    bytes.extend_from_slice(&u64::MAX.to_le_bytes());
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.extend_from_slice(&0x8bc215c38233ba4b_u64.to_le_bytes());
    bytes
}

pub(crate) fn compact(id: ExGuid, table: &BTreeMap<u32, [u8; 16]>) -> Result<[u8; 4]> {
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

/// The global id table entries a property object's references name.
pub(crate) fn table_entries(bytes: &[u8]) -> Result<Vec<u32>> {
    let mut entries = Vec::new();
    for property in PropertySets::parse(bytes)?.sets.iter().flatten() {
        if let Value::References { compact_ids, .. } = property.value {
            entries.extend(
                compact_ids
                    .chunks_exact(4)
                    .map(|id| u32::from_le_bytes(id.try_into().unwrap()) >> 8),
            );
        }
    }
    Ok(entries)
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
    let store = Store::parse(source)?;
    let index = RevisionIndex::parse(&store)?;
    index.validate_current()?;
    replace_objects(
        &index,
        space,
        &[ObjectEdit {
            object: object_id,
            updates: &[(property, value)],
            inserts: &[],
        }],
    )
}

/// Patches objects of a source the caller has parsed and validated.
pub(crate) fn replace_objects(
    index: &RevisionIndex<'_>,
    space: ExGuid,
    edits: &[ObjectEdit<'_>],
) -> Result<Vec<u8>> {
    write_revision_on(index, space, |revision| patched(revision, edits))
}

/// Objects of `revision` with `edits` applied.
pub(crate) fn patched(
    revision: &crate::ResolvedRevision<'_>,
    edits: &[ObjectEdit<'_>],
) -> Result<BTreeMap<ExGuid, PropertyObject>> {
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
}

/// Whether `after` stores as `before` does: the same type and bytes, naming the same
/// identities. Stored tables keep only the entries an object names, so those decide.
pub(crate) fn unchanged(before: &crate::Object<'_>, after: &crate::Object<'_>) -> Result<bool> {
    if before.jcid != after.jcid || before.data != after.data {
        return Ok(false);
    }
    let entries = match after.data {
        ObjectData::Properties(bytes) => table_entries(bytes)?,
        _ => Vec::new(),
    };
    Ok(entries
        .iter()
        .all(|entry| before.global_ids.get(entry) == after.global_ids.get(entry)))
}

/// `object` as a replacement a revision stores under `id`.
pub(crate) fn replacement(id: ExGuid, object: &crate::Object<'_>) -> Result<PropertyObject> {
    let mut replacement = match object.data {
        ObjectData::Properties(_) => PropertyObject::from_object(object)?,
        ObjectData::File {
            reference,
            extension,
        } => {
            let text = |bytes: &[u8]| {
                String::from_utf16(
                    &bytes
                        .chunks_exact(2)
                        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                        .collect::<Vec<_>>(),
                )
                .map_err(|_| Error {
                    offset: 0,
                    message: "Invalid UTF-16 file-data declaration",
                })
            };
            let mut replacement = PropertyObject::file(id, &text(reference)?, &text(extension)?)?;
            replacement.jcid = object.jcid;
            replacement
        }
        ObjectData::Encrypted(_) => {
            return Err(Error {
                offset: 0,
                message: "Page edits only produce property objects",
            });
        }
    };
    replacement.reference(id)?;
    Ok(replacement)
}

/// The object type OneNote gives embedded picture payload declarations; embedded files
/// use `EMBEDDED_FILE_JCID` with the same declaration shape.
pub(crate) const FILE_DATA_JCID: u32 = 0x80039;
pub(crate) const EMBEDDED_FILE_JCID: u32 = 0x80036;

/// The reference and extension of a file-data declaration built by `PropertyObject::file`.
fn file_declaration(bytes: &[u8]) -> Result<(&[u8], &[u8])> {
    let malformed = || Error {
        offset: 0,
        message: "Malformed file-data declaration",
    };
    let mut rest = bytes;
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

/// An object of type `jcid` stored as `bytes`, as a revision declares it.
pub(crate) fn declared(
    jcid: u32,
    bytes: &[u8],
    global_ids: Arc<BTreeMap<u32, [u8; 16]>>,
) -> Result<crate::Object<'_>> {
    let data = if is_file_declaration(jcid) {
        let (reference, extension) = file_declaration(bytes)?;
        ObjectData::File {
            reference,
            extension,
        }
    } else {
        ObjectData::Properties(bytes)
    };
    Ok(crate::Object {
        jcid,
        reference_count: 0,
        data,
        global_ids,
    })
}

fn is_file_declaration(jcid: u32) -> bool {
    jcid == FILE_DATA_JCID || jcid == EMBEDDED_FILE_JCID
}

#[derive(Clone)]
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
    /// A complete revision of an existing space under a context and role, without history.
    #[cfg(feature = "protected")]
    Label {
        context: ExGuid,
        role: u32,
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

/// A password-protected section's unlocked view, through which its revisions are
/// read as plaintext and written back in their stored form.
pub(crate) trait Protection {
    fn resolve(&self, space: ExGuid, revision: ExGuid) -> Result<crate::ResolvedRevision<'_>>;
    /// The stored bytes a resolved object's plaintext was decoded from.
    fn stored(&self, clear: &[u8]) -> Option<&[u8]>;
    fn seal_property(&self, clear: &[u8]) -> Result<Vec<u8>>;
    fn seal_file(&self, clear: &[u8]) -> Vec<u8>;
}

/// The global-ID entries a group of objects declared together uses: each object's own
/// and those its properties reference. A section's group stores only these.
fn used_entries<'o>(
    table: &BTreeMap<u32, [u8; 16]>,
    objects: impl IntoIterator<Item = (ExGuid, &'o crate::Object<'o>)>,
) -> Result<Vec<u32>> {
    let mut used = Vec::new();
    for (id, object) in objects {
        used.push(u32::from_le_bytes(compact(id, table)?) >> 8);
        if let ObjectData::Properties(bytes) = object.data {
            used.extend(table_entries(bytes)?);
        }
    }
    used.sort_unstable();
    used.dedup();
    Ok(used)
}

/// A space's revision as appending a revision and reading it back leaves it.
#[derive(Clone)]
pub(crate) struct LiveRevision<'a> {
    pub revision: crate::ResolvedRevision<'a>,
    /// Incoming references of each object reachable from the roots; a root counts once.
    incoming: BTreeMap<ExGuid, u32>,
    /// Reachable read-only property objects by type and content, which identical new
    /// objects alias.
    readonly: BTreeMap<u32, BTreeMap<&'a [u8], BTreeSet<ExGuid>>>,
    /// Revisions in the dependency chain, counted to 512; zero before the first.
    pub depth: usize,
}

impl<'a> LiveRevision<'a> {
    pub(crate) fn new(revision: crate::ResolvedRevision<'a>, depth: usize) -> Result<Self> {
        let incoming = if depth == 0 {
            BTreeMap::new()
        } else {
            revision.checked_counts()?
        };
        let mut readonly = BTreeMap::new();
        for id in incoming.keys() {
            index(&mut readonly, *id, &revision.objects[id], true);
        }
        Ok(Self {
            revision,
            incoming,
            readonly,
            depth,
        })
    }

    pub(crate) fn is_reachable(&self, id: ExGuid) -> bool {
        self.incoming.contains_key(&id)
    }

    /// Validates replacements and drops what the revision would not store: new read-only
    /// objects identical to reachable or earlier ones, whose references move to those, and
    /// reachable objects that stay as they are.
    pub(crate) fn prepare(
        &self,
        mut replacements: BTreeMap<ExGuid, PropertyObject>,
        is_section: bool,
    ) -> Result<BTreeMap<ExGuid, PropertyObject>> {
        let revision = &self.revision;
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
            } else if !is_section && replacement.jcid != 0x20001 {
                return Err(Error {
                    offset: 0,
                    message: "New objects in a table of contents are its entries",
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
                file_declaration(&replacement.bytes)?;
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
            let existing = self
                .readonly
                .get(&replacement.jcid)
                .and_then(|contents| contents.get(replacement.bytes.as_slice()))
                .and_then(|ids| ids.first().copied());
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
            // The compact IDs this object's table gives aliased identities.
            let targets: Vec<(u32, ExGuid)> = aliases
                .iter()
                .filter(|(id, _)| id.n <= 0xff)
                .filter_map(|(id, existing)| {
                    let (index, _) = object.global_ids.iter().find(|(_, g)| **g == id.guid)?;
                    Some(((index << 8) | id.n, *existing))
                })
                .collect();
            if targets.is_empty() {
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
                        let raw = u32::from_le_bytes(bytes.try_into().unwrap());
                        if let Some((_, existing)) = targets.iter().find(|(id, _)| *id == raw) {
                            let offset = bytes.as_ptr().addr() - object.bytes.as_ptr().addr();
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
        // Detached objects must pass the final reachability check even when unchanged. Equal
        // bytes are the same object when the table entries they name agree: stored tables
        // keep only those.
        let mut unchanged = Vec::new();
        for (id, replacement) in &replacements {
            if let Some(object) = revision.objects.get(id)
                && self.is_reachable(*id)
                && object.data == ObjectData::Properties(&replacement.bytes)
                && (Arc::ptr_eq(&object.global_ids, &replacement.global_ids)
                    || table_entries(&replacement.bytes)?.iter().all(|entry| {
                        object.global_ids.get(entry) == replacement.global_ids.get(entry)
                    }))
            {
                unchanged.push(*id);
            }
        }
        for id in unchanged {
            replacements.remove(&id);
        }
        Ok(replacements)
    }

    /// Stores prepared replacements as the next revision, which checkpoints when the chain
    /// would exceed 512 revisions.
    pub(crate) fn commit(
        &mut self,
        replacements: Vec<(ExGuid, crate::Object<'a>)>,
    ) -> Result<Commit> {
        let mut changed = BTreeSet::new();
        // Whether each object whose count may move was reachable before.
        let mut touched = BTreeMap::new();
        let (mut added, mut removed) = (Vec::new(), Vec::new());
        for (id, object) in replacements {
            changed.insert(id);
            let reachable = self.is_reachable(id);
            touched.insert(id, reachable);
            if reachable {
                moved_references(
                    &self.revision.objects[&id],
                    &object,
                    &mut removed,
                    &mut added,
                )?;
            }
            self.revision.objects.insert(id, object);
        }
        let roots: BTreeSet<ExGuid> = self.revision.roots.values().copied().collect();
        if roots.len() != self.revision.roots.len() {
            return Err(Error {
                offset: 0,
                message: "Object is the root of multiple roles",
            });
        }
        added.extend(
            roots
                .into_iter()
                .filter(|id| !self.incoming.contains_key(id)),
        );
        // Attaching before detaching keeps a moved subtree from being walked twice.
        while let Some(id) = added.pop() {
            let object = self.revision.objects.get(&id).ok_or(Error {
                offset: 0,
                message: "Reachable object has no declaration",
            })?;
            let count = self.incoming.entry(id).or_default();
            touched.entry(id).or_insert(*count > 0);
            *count = count.checked_add(1).ok_or(Error {
                offset: 0,
                message: "Object reference count overflows",
            })?;
            if *count == 1 {
                added.extend(object.references()?.objects);
                index(&mut self.readonly, id, object, true);
            }
        }
        while let Some(id) = removed.pop() {
            touched.entry(id).or_insert(true);
            let count = self.incoming.get_mut(&id).unwrap();
            *count -= 1;
            if *count == 0 {
                self.incoming.remove(&id);
                let object = &self.revision.objects[&id];
                removed.extend(object.references()?.objects);
                index(&mut self.readonly, id, object, false);
            }
        }
        for (&id, &reachable) in &touched {
            let count = self.incoming.get(&id).copied();
            if changed.contains(&id) && count.is_none() {
                return Err(Error {
                    offset: 0,
                    message: "Edited object is not reachable in the resulting revision",
                });
            }
            let object = self.revision.objects.get_mut(&id).unwrap();
            if (reachable || count.is_some()) && object.reference_count != count.unwrap_or(0) {
                object.reference_count = count.unwrap_or(0);
                changed.insert(id);
            }
        }
        // Native cold-open fails on long dependency chains; cap their depth at 512.
        let checkpoint = self.depth == 0 || self.depth >= 512;
        self.depth = if checkpoint { 1 } else { self.depth + 1 };
        Ok(Commit {
            checkpoint,
            changed,
            touched: touched.into_keys().collect(),
        })
    }

    /// Gives the objects a revision declares the id tables reading it back yields: objects
    /// sharing a table are declared in one group, which keeps the entries they use.
    pub(crate) fn settle(&mut self, declared: impl IntoIterator<Item = ExGuid>) -> Result<()> {
        let mut groups = BTreeMap::<_, Vec<_>>::new();
        for id in declared {
            let table = Arc::clone(&self.revision.objects[&id].global_ids);
            groups.entry(table).or_default().push(id);
        }
        for (table, ids) in groups {
            let used = used_entries(
                &table,
                ids.iter().map(|id| (*id, &self.revision.objects[id])),
            )?;
            if used.iter().eq(table.keys()) {
                continue;
            }
            let kept: Arc<BTreeMap<_, _>> = Arc::new(
                table
                    .iter()
                    .filter(|(entry, _)| used.binary_search(entry).is_ok())
                    .map(|(entry, guid)| (*entry, *guid))
                    .collect(),
            );
            for id in ids {
                self.revision.objects.get_mut(&id).unwrap().global_ids = Arc::clone(&kept);
            }
        }
        Ok(())
    }
}

/// Adds a reachable read-only property object to `readonly`, or removes an unreachable one.
fn index<'a>(
    readonly: &mut BTreeMap<u32, BTreeMap<&'a [u8], BTreeSet<ExGuid>>>,
    id: ExGuid,
    object: &crate::Object<'a>,
    reachable: bool,
) {
    let ObjectData::Properties(bytes) = object.data else {
        return;
    };
    if object.jcid & 0x100000 == 0 {
        return;
    }
    let ids = readonly
        .entry(object.jcid)
        .or_default()
        .entry(bytes)
        .or_default();
    if reachable {
        ids.insert(id);
    } else {
        ids.remove(&id);
    }
}

/// Adds the object references `before` has and `after`, a later version of the object,
/// lacks to `removed`, and those it gains to `added`.
fn moved_references(
    before: &crate::Object<'_>,
    after: &crate::Object<'_>,
    removed: &mut Vec<ExGuid>,
    added: &mut Vec<ExGuid>,
) -> Result<()> {
    let compact_ids = |object: &crate::Object<'_>| -> Result<Vec<[u8; 4]>> {
        let ObjectData::Properties(bytes) = object.data else {
            return Ok(Vec::new());
        };
        let mut ids = Vec::new();
        for property in PropertySets::parse(bytes)?.sets.iter().flatten() {
            if let Value::References {
                stream: crate::IdStream::Objects,
                compact_ids,
            } = property.value
            {
                ids.extend(
                    compact_ids
                        .chunks_exact(4)
                        .map(|id| <[u8; 4]>::try_from(id).unwrap()),
                );
            }
        }
        Ok(ids)
    };
    let decode = |ids: &[[u8; 4]], table| -> Result<Vec<ExGuid>> {
        let bytes = ids.concat();
        let mut cursor = crate::bytes::Cursor {
            bytes: &bytes,
            offset: 0,
        };
        std::iter::from_fn(|| (!cursor.bytes.is_empty()).then(|| cursor.compact(table))).collect()
    };
    // Equal compact IDs name the same objects where the later table keeps the earlier entries.
    let (old, new) = (&before.global_ids, &after.global_ids);
    let extended = Arc::ptr_eq(old, new) || {
        let mut later = new.iter();
        old.iter()
            .all(|entry| later.find(|later| later.0 >= entry.0) == Some(entry))
    };
    if extended {
        let (old_ids, new_ids) = (compact_ids(before)?, compact_ids(after)?);
        let (gone, gained) = differing(&old_ids, &new_ids);
        removed.extend(decode(gone, old)?);
        added.extend(decode(gained, new)?);
    } else {
        let (old_refs, new_refs) = (before.references()?.objects, after.references()?.objects);
        let (gone, gained) = differing(&old_refs, &new_refs);
        removed.extend_from_slice(gone);
        added.extend_from_slice(gained);
    }
    Ok(())
}

/// The parts of two sequences between their common prefix and suffix: removing the first
/// from `before` and adding the second yields `after` as a multiset.
pub(crate) fn differing<'s, T: PartialEq>(before: &'s [T], after: &'s [T]) -> (&'s [T], &'s [T]) {
    let prefix = before.iter().zip(after).take_while(|(a, b)| a == b).count();
    let suffix = before[prefix..]
        .iter()
        .rev()
        .zip(after[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    (
        &before[prefix..before.len() - suffix],
        &after[prefix..after.len() - suffix],
    )
}

/// A revision `LiveRevision::commit` stored.
pub(crate) struct Commit {
    /// Whether the revision declares every object, depending on none.
    pub checkpoint: bool,
    /// The objects it declares otherwise: the replacements and those whose counts moved.
    pub changed: BTreeSet<ExGuid>,
    /// Objects whose reachability or reference count may have moved.
    pub touched: BTreeSet<ExGuid>,
}

/// Revisions in the dependency chain of `rid`, counted to 512.
pub(crate) fn chain_depth(index: &RevisionIndex<'_>, space: ExGuid, rid: ExGuid) -> usize {
    std::iter::successors(Some(rid), |id| {
        index.spaces[&space].revisions[id].dependency
    })
    .take(512)
    .count()
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
    write_revisions_with_payloads(source, payloads, update(space, edit))
}

/// One space's active revision edited into its update.
fn update(
    space: ExGuid,
    edit: impl FnOnce(&crate::ResolvedRevision<'_>) -> Result<BTreeMap<ExGuid, PropertyObject>>,
) -> impl FnOnce(&RevisionIndex<'_>) -> Result<BTreeMap<ExGuid, RevisionEdit>> {
    move |index| {
        let revision = index.resolve(space, index.active(space)?)?;
        Ok(BTreeMap::from([(
            space,
            RevisionEdit::Update(edit(&revision)?),
        )]))
    }
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
    publish(source, payloads, None, true, edit)
}

/// `write_revisions_with_payloads` without validating that current revisions are
/// complete: a protected section is validated by unlocking it, through `protection`.
#[cfg(feature = "protected")]
pub(crate) fn append_revisions(
    source: &[u8],
    payloads: &[([u8; 16], &[u8])],
    protection: Option<&dyn Protection>,
    edit: impl FnOnce(&RevisionIndex<'_>) -> Result<BTreeMap<ExGuid, RevisionEdit>>,
) -> Result<Vec<u8>> {
    publish(source, payloads, protection, false, edit)
}

fn publish(
    source: &[u8],
    payloads: &[([u8; 16], &[u8])],
    protection: Option<&dyn Protection>,
    validate: bool,
    edit: impl FnOnce(&RevisionIndex<'_>) -> Result<BTreeMap<ExGuid, RevisionEdit>>,
) -> Result<Vec<u8>> {
    // The parsed source is released before the result is parsed.
    let output = build(source, payloads, protection, validate, edit)?;
    check(&output, validate)?;
    Ok(output)
}

/// Parses a written image, and with `validate` requires its current revisions complete.
pub(crate) fn check(output: &[u8], validate: bool) -> Result<()> {
    let store = Store::parse(output)?;
    let index = RevisionIndex::parse(&store)?;
    if validate {
        index.validate_current()?;
    }
    Ok(())
}

/// The written image, unchecked: `check` follows once the caller has released whatever
/// `edit` borrowed.
pub(crate) fn build(
    source: &[u8],
    payloads: &[([u8; 16], &[u8])],
    protection: Option<&dyn Protection>,
    validate: bool,
    edit: impl FnOnce(&RevisionIndex<'_>) -> Result<BTreeMap<ExGuid, RevisionEdit>>,
) -> Result<Vec<u8>> {
    let store = Store::parse(source)?;
    let index = RevisionIndex::parse(&store)?;
    if validate {
        index.validate_current()?;
    }
    build_on(&index, payloads, protection, edit)
}

/// `write_revision` on a source the caller has parsed and validated.
pub(crate) fn write_revision_on(
    index: &RevisionIndex<'_>,
    space: ExGuid,
    edit: impl FnOnce(&crate::ResolvedRevision<'_>) -> Result<BTreeMap<ExGuid, PropertyObject>>,
) -> Result<Vec<u8>> {
    let output = build_on(index, &[], None, update(space, edit))?;
    check(&output, true)?;
    Ok(output)
}

#[cfg(test)]
thread_local! {
    /// Revisions `build_on` has built on this thread.
    pub(crate) static BUILDS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

pub(crate) fn build_on(
    index: &RevisionIndex<'_>,
    payloads: &[([u8; 16], &[u8])],
    protection: Option<&dyn Protection>,
    edit: impl FnOnce(&RevisionIndex<'_>) -> Result<BTreeMap<ExGuid, RevisionEdit>>,
) -> Result<Vec<u8>> {
    #[cfg(test)]
    BUILDS.with(|builds| builds.set(builds.get() + 1));
    let store = index.store;
    let source = store.data;
    let is_section = store.header.file_type == FileType::Section;
    if !store.checksum_mismatches.is_empty() {
        return Err(Error {
            offset: store.checksum_mismatches[0],
            message: "Cannot write a file with transaction checksum damage",
        });
    }
    let resolve = |space, rid| match protection {
        Some(protection) => protection.resolve(space, rid),
        None => index.resolve(space, rid),
    };
    let changes = edit(index)?;
    let mut appending = Appending::new(store.state()?);
    for (space, change) in changes {
        let new_space = matches!(change, RevisionEdit::Create { .. });
        let current = (ExGuid::default(), 1_u32);
        let (rid, label, revision, replacements) = match change {
            RevisionEdit::Update(objects) => {
                let rid = index.active(space)?;
                (Some(rid), current, resolve(space, rid)?, objects)
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
                    current,
                    crate::ResolvedRevision {
                        roots,
                        objects: BTreeMap::new(),
                    },
                    objects,
                )
            }
            #[cfg(feature = "protected")]
            RevisionEdit::Label {
                context,
                role,
                roots,
                objects,
            } => {
                if !is_section || role > 0xffff || !index.spaces.contains_key(&space) {
                    return Err(Error {
                        offset: 0,
                        message: "Choose a label in a section's object space",
                    });
                }
                (
                    None,
                    (context, role),
                    crate::ResolvedRevision {
                        roots,
                        objects: BTreeMap::new(),
                    },
                    objects,
                )
            }
        };
        let depth = rid.map_or(0, |rid| chain_depth(index, space, rid));
        let mut live = LiveRevision::new(revision, depth)?;
        let replacements = live.prepare(replacements, is_section)?;
        if replacements.is_empty() {
            continue;
        }
        let replaced: BTreeSet<ExGuid> = replacements.keys().copied().collect();
        let created = replaced
            .iter()
            .filter(|id| !live.revision.objects.contains_key(id))
            .copied()
            .collect();
        let commit = live.commit(
            replacements
                .iter()
                .map(|(id, replacement)| {
                    let global_ids = Arc::clone(&replacement.global_ids);
                    Ok((
                        *id,
                        declared(replacement.jcid, &replacement.bytes, global_ids)?,
                    ))
                })
                .collect::<Result<Vec<_>>>()?,
        )?;
        // A dependent revision inherits its key, as OneNote writes it.
        let key = if protection.is_some() && commit.checkpoint {
            Some(
                index
                    .spaces
                    .get(&space)
                    .and_then(|space| {
                        space.revisions.values().find_map(|revision| {
                            revision.nodes.first().filter(|node| node.id == 0x7c)
                        })
                    })
                    .ok_or(Error {
                        offset: 0,
                        message: "Protected revisions continue a protected object space",
                    })?,
            )
        } else {
            None
        };
        appending.revision(
            &Sealing {
                space,
                previous: rid,
                new_space,
                label,
                live: &live,
                commit: &commit,
                replaced: &replaced,
                created: &created,
            },
            &[(0, source)],
            protection,
            key,
        )?;
    }
    appending.payloads(payloads, protection)?;
    Ok(match appending.finish()? {
        Some((transaction, _)) => {
            let mut output = source.to_vec();
            transaction.apply(&mut output)?;
            output
        }
        None => source.to_vec(),
    })
}

/// A space's revision as `LiveRevision::commit` left it, ready to append.
pub(crate) struct Sealing<'r, 'a> {
    pub space: ExGuid,
    /// The revision it depends on unless it is a checkpoint; none for a new space or label.
    pub previous: Option<ExGuid>,
    pub new_space: bool,
    /// The context and role it is current under.
    pub label: (ExGuid, u32),
    pub live: &'r LiveRevision<'a>,
    pub commit: &'r Commit,
    /// Objects whose bytes the revision stores rather than referencing stored bytes.
    pub replaced: &'r BTreeSet<ExGuid>,
    /// Replaced objects the space did not hold before.
    pub created: &'r BTreeSet<ExGuid>,
}

/// The chunk `bytes` occupies in a store whose bytes `segments` hold at their offsets.
fn located(segments: &[(u64, &[u8])], bytes: &[u8]) -> Result<Chunk> {
    let address = bytes.as_ptr().addr();
    segments
        .iter()
        .find_map(|(offset, segment)| {
            let start = address.checked_sub(segment.as_ptr().addr())?;
            (start + bytes.len() <= segment.len()).then(|| Chunk {
                offset: offset + start as u64,
                length: bytes.len() as u64,
            })
        })
        .ok_or(Error {
            offset: 0,
            message: "Stored object data lies outside the store",
        })
}

/// A transaction under construction: bytes appended at the end of a store and patches
/// inside it, advancing `state` as its lists grow.
pub(crate) struct Appending {
    pub state: StoreState,
    base: crate::Stamp,
    append: Vec<u8>,
    patches: Vec<(u64, Vec<u8>)>,
    /// File-node counts of the lists this transaction writes, as log entries.
    counts: Vec<(u32, usize)>,
    /// Nodes the root list gains: new object spaces and the file-data store.
    root_nodes: Vec<Vec<u8>>,
}

impl Appending {
    pub(crate) fn new(state: StoreState) -> Self {
        let base = state.stamp.clone();
        let mut append = Vec::with_capacity(1 << 16);
        // Native files reserve 1 KiB per transaction-log fragment; a fragment that ends the
        // file keeps that room before the first new chunk.
        let (tail, _) = state.log;
        if (tail.offset + tail.length).next_multiple_of(8) >= base.length {
            let reserved = tail.offset + tail.length.max(1024) + 32;
            append.resize(reserved.saturating_sub(base.length) as usize, 0);
        }
        Self {
            state,
            base,
            append,
            patches: Vec::new(),
            counts: Vec::new(),
            root_nodes: Vec::new(),
        }
    }

    fn end(&self) -> u64 {
        self.base.length + self.append.len() as u64
    }

    fn append(&mut self, bytes: &[u8]) -> Result<Chunk> {
        let length = u64::from(u32::try_from(bytes.len()).map_err(|_| Error {
            offset: 0,
            message: "Chunk exceeds the encoded length limit",
        })?);
        let padding = self.end().next_multiple_of(8) - self.end();
        self.append.resize(self.append.len() + padding as usize, 0);
        let offset = self.end();
        self.append.extend_from_slice(bytes);
        Ok(Chunk { offset, length })
    }

    /// Writes `bytes` at a file offset: into the appended bytes, or as a patch.
    fn write(&mut self, offset: u64, bytes: &[u8]) {
        match offset.checked_sub(self.base.length) {
            Some(at) => {
                let at = at as usize;
                self.append[at..at + bytes.len()].copy_from_slice(bytes);
            }
            None => self.patches.push((offset, bytes.to_vec())),
        }
    }

    fn allocate_list(&mut self) -> Result<u32> {
        self.state.max_list = self.state.max_list.checked_add(1).ok_or(Error {
            offset: 0,
            message: "File-node list identities are exhausted",
        })?;
        Ok(self.state.max_list)
    }

    /// Appends `nodes` as a new list.
    fn list(&mut self, nodes: &[Vec<u8>]) -> Result<(Chunk, ListTail)> {
        let id = self.allocate_list()?;
        let chunk = self.append(&fragment(id, 0, nodes))?;
        self.counts.push((id, nodes.len()));
        Ok((
            chunk,
            ListTail {
                id,
                fragments: 1,
                last: chunk,
                nodes: nodes.len(),
                end: chunk.offset + 16 + nodes.iter().map(Vec::len).sum::<usize>() as u64,
            },
        ))
    }

    /// Appends `nodes` to the list ending at `tail` as its next fragment.
    fn extend(&mut self, tail: &mut ListTail, nodes: &[Vec<u8>]) -> Result<()> {
        let chunk = self.append(&fragment(tail.id, tail.fragments, nodes))?;
        let next = tail.last.offset + tail.last.length - 20;
        if next - tail.end >= 4 {
            self.write(tail.end, &node(0xff, None, &[])?);
        }
        let mut reference = chunk.offset.to_le_bytes().to_vec();
        reference.extend_from_slice(&(chunk.length as u32).to_le_bytes());
        self.write(next, &reference);
        *tail = ListTail {
            id: tail.id,
            fragments: tail.fragments.checked_add(1).ok_or(Error {
                offset: 0,
                message: "File-node fragment sequences are exhausted",
            })?,
            last: chunk,
            nodes: tail.nodes + nodes.len(),
            end: chunk.offset + 16 + nodes.iter().map(Vec::len).sum::<usize>() as u64,
        };
        self.counts.push((tail.id, tail.nodes));
        Ok(())
    }

    /// Appends a space's next revision: its manifest, object groups and new data. Returns the
    /// revision's identity and where it stored each replaced object's bytes.
    pub(crate) fn revision(
        &mut self,
        sealing: &Sealing<'_, '_>,
        segments: &[(u64, &[u8])],
        protection: Option<&dyn Protection>,
        key: Option<&crate::Node<'_>>,
    ) -> Result<(ExGuid, Vec<(ExGuid, Chunk)>)> {
        let is_section = self.state.file_type == FileType::Section;
        let Sealing {
            space,
            label,
            live,
            commit,
            ..
        } = *sealing;
        let revision = &live.revision;
        let checkpoint = commit.checkpoint;
        let selected: Vec<(&ExGuid, &crate::Object<'_>)> = if checkpoint {
            revision.objects.iter().collect()
        } else {
            commit
                .changed
                .iter()
                .map(|id| (id, &revision.objects[id]))
                .collect()
        };
        // A table-of-contents manifest has one global id table (sections group objects,
        // each group with its own table); OneNote resolves every node against it.
        let toc_table = if is_section {
            None
        } else {
            if checkpoint
                && selected.iter().any(|(_, object)| {
                    object.jcid != 0x20001 || !matches!(object.data, ObjectData::Properties(_))
                })
            {
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
            sealing.previous.ok_or(Error {
                offset: 0,
                message: "A dependent revision needs the revision it follows",
            })?
        }
        .encode(&mut start);
        if !is_section {
            start.extend_from_slice(&0_u64.to_le_bytes());
        }
        start.extend_from_slice(&label.1.to_le_bytes());
        start.extend_from_slice(&(if protection.is_some() { 2_u16 } else { 0 }).to_le_bytes());
        let contextual = label.0 != ExGuid::default();
        if contextual {
            label.0.encode(&mut start);
        }
        let mut manifest = Vec::new();
        if sealing.new_space {
            let mut payload = Vec::new();
            space.encode(&mut payload);
            payload.extend_from_slice(&0_u32.to_le_bytes());
            manifest.push(node(0x14, None, &payload)?);
        }
        manifest.push(node(
            match (is_section, contextual) {
                (true, true) => 0x1f,
                (true, false) => 0x1e,
                (false, _) => 0x1b,
            },
            None,
            &start,
        )?);
        if protection.is_some() && checkpoint {
            let key = key.ok_or(Error {
                offset: 0,
                message: "Protected revisions continue a protected object space",
            })?;
            manifest.push(node(0x7c, key.reference, key.payload)?);
        }
        let mut stored = Vec::new();
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
            // A table read from a long-lived page names every session that edited it; the
            // group stores the entries its objects use.
            let used = used_entries(table, objects.iter().copied())?;
            for (id, guid) in table {
                if is_section && used.binary_search(id).is_err() {
                    continue;
                }
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
                            self.append(&mapped)?
                        } else if sealing.replaced.contains(&id) {
                            let chunk = match protection {
                                Some(protection) => {
                                    self.append(&protection.seal_property(bytes)?)?
                                }
                                None => self.append(bytes)?,
                            };
                            stored.push((id, chunk));
                            chunk
                        } else {
                            located(
                                segments,
                                match protection {
                                    Some(protection) => protection.stored(bytes).ok_or(Error {
                                        offset: 0,
                                        message: "Protected object has no stored form",
                                    })?,
                                    None => bytes,
                                },
                            )?
                        };
                        // A table-of-contents object is declared when the revision is a
                        // checkpoint or the object is new, and revised otherwise.
                        let declared = checkpoint || sealing.created.contains(&id);
                        if is_section {
                            declaration.extend_from_slice(&object.jcid.to_le_bytes());
                            declaration.push(flags);
                        } else if declared {
                            let body = 1_u64 | (u64::from(flags & 1) << 16);
                            declaration.extend_from_slice(&body.to_le_bytes()[..6]);
                        } else {
                            declaration.extend_from_slice(&u32::from(flags).to_le_bytes());
                        }
                        declaration.extend_from_slice(&object.reference_count.to_le_bytes());
                        let readonly = object.jcid & 0x100000 != 0;
                        if readonly {
                            // A protected declaration hashes the plaintext as aligned.
                            let mut hash = md5::Context::new();
                            hash.consume(bytes);
                            if protection.is_some() {
                                hash.consume(&[0; 7][..(8 - bytes.len() % 8) % 8]);
                            }
                            declaration.extend_from_slice(&hash.finalize().0);
                        }
                        group.push(node(
                            if is_section {
                                if readonly { 0xc5 } else { 0xa5 }
                            } else if declared {
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
                let (chunk, _) = self.list(&group)?;
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
        if sealing.new_space {
            let (chunk, tail) = self.list(&manifest)?;
            let mut payload = Vec::new();
            space.encode(&mut payload);
            let (chunk, _) = self.list(&[
                node(0xc, None, &payload)?,
                node(0x10, Some(Reference::NodeList(chunk)), &[])?,
            ])?;
            self.root_nodes
                .push(node(8, Some(Reference::NodeList(chunk)), &payload)?);
            self.state.spaces.insert(space, tail);
        } else {
            let mut tail = *self.state.spaces.get(&space).ok_or(Error {
                offset: 0,
                message: "Object space is absent from the root list",
            })?;
            self.extend(&mut tail, &manifest)?;
            self.state.spaces.insert(space, tail);
        }
        Ok((new_rid, stored))
    }

    /// Embeds payloads as file-data store objects referenced from the root file node list
    /// under their identities, as OneNote embeds pictures and attachments.
    pub(crate) fn payloads(
        &mut self,
        payloads: &[([u8; 16], &[u8])],
        protection: Option<&dyn Protection>,
    ) -> Result<()> {
        let mut data_nodes = Vec::new();
        for (guid, payload) in payloads {
            if self.state.file_type != FileType::Section {
                return Err(Error {
                    offset: 0,
                    message: "Embedded payloads require a section file",
                });
            }
            let mut blob = vec![
                0xe7, 0x16, 0xe3, 0xbd, 0x65, 0x26, 0x11, 0x45, 0xa4, 0xc4, 0x8d, 0x4d, 0x0b, 0x7a,
                0x9e, 0xac,
            ];
            let sealed = protection.map(|protection| protection.seal_file(payload));
            let payload = sealed.as_deref().unwrap_or(*payload);
            blob.extend_from_slice(&(payload.len() as u64).to_le_bytes());
            blob.extend_from_slice(&[0; 12]);
            blob.extend_from_slice(payload);
            blob.resize(blob.len().next_multiple_of(8), 0);
            blob.extend_from_slice(&[
                0x22, 0xa7, 0xfb, 0x71, 0x79, 0x0f, 0x0b, 0x4a, 0xbb, 0x13, 0x89, 0x92, 0x56, 0x42,
                0x6b, 0x24,
            ]);
            let chunk = self.append(&blob)?;
            data_nodes.push(node(0x94, Some(Reference::Data(chunk)), guid)?);
        }
        if data_nodes.is_empty() {
            return Ok(());
        }
        // Payload declarations live in the file-data store list the root list references.
        match self.state.files {
            Some(mut tail) => {
                self.extend(&mut tail, &data_nodes)?;
                self.state.files = Some(tail);
            }
            None => {
                let (chunk, tail) = self.list(&data_nodes)?;
                self.state.files = Some(tail);
                self.root_nodes
                    .push(node(0x90, Some(Reference::NodeList(chunk)), &[])?);
            }
        }
        Ok(())
    }

    /// The transaction committing what was appended, and the state it leaves; none when
    /// nothing was.
    pub(crate) fn finish(mut self) -> Result<Option<(crate::Transaction, StoreState)>> {
        if !self.root_nodes.is_empty() {
            let nodes = std::mem::take(&mut self.root_nodes);
            let mut root = self.state.root;
            self.extend(&mut root, &nodes)?;
            self.state.root = root;
        }
        if self.counts.is_empty() {
            return Ok(None);
        }
        let file_type = self.state.file_type;
        let is_section = file_type == FileType::Section;
        let mut header = self.base.header;
        let count = u32::from_le_bytes(header[96..100].try_into().unwrap());
        let transactions = count.checked_add(1).ok_or(Error {
            offset: 96,
            message: "Transaction counter is exhausted",
        })?;
        let changed_bytes = count ^ transactions;
        let commit_byte = (31 - changed_bytes.leading_zeros()) / 8;
        let ceiling = transactions | ((1_u32 << (commit_byte * 8)) - 1);
        let mut entries = Vec::new();
        for (id, count) in std::mem::take(&mut self.counts) {
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
        let (mut log_chunk, mut used) = self.state.log;
        let mut checksum = self.state.log_crc;
        let mut crc_used = used;
        let mut crc_capacity = (log_chunk.length as usize - 12) & !7;
        let mut advance_crc = |checksum, entry: &[u8]| {
            if crc_used == crc_capacity {
                crc_used = 0;
                crc_capacity = 1008;
            }
            crc_used += 8;
            transaction_crc(checksum, entry, file_type, crc_used == crc_capacity)
        };
        for entry in entries.chunks_exact(8) {
            checksum = advance_crc(checksum, entry);
        }
        // Past a counter carry, sentinels up to the ceiling follow the one that commits; the
        // next transaction's entries overwrite them, as a reader counting sentinels expects.
        let committed = entries.len() + 8;
        let mut committed_crc = None;
        for _ in transactions..=ceiling {
            let sentinel_crc = if is_section { !checksum } else { checksum };
            entries.extend_from_slice(&1_u32.to_le_bytes());
            entries.extend_from_slice(&sentinel_crc.to_le_bytes());
            checksum = advance_crc(checksum, &entries[entries.len() - 8..]);
            committed_crc.get_or_insert(checksum);
        }
        let mut log = self.state.log;
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
            let size = remaining.len().min(capacity - used);
            self.write(log_chunk.offset + used as u64, &remaining[..size]);
            let written = entries.len() - remaining.len();
            if (written + 1..=written + size).contains(&committed) {
                log = (log_chunk, used + committed - written);
            }
            used += size;
            remaining = &remaining[size..];
            if !remaining.is_empty() {
                let mut fragment = vec![0; 1024];
                fragment[1008..1016].copy_from_slice(&u64::MAX.to_le_bytes());
                let next = self.append(&fragment)?;
                let mut reference = next.offset.to_le_bytes().to_vec();
                reference.extend_from_slice(&(next.length as u32).to_le_bytes());
                self.write(log_chunk.offset + capacity as u64, &reference);
                log_chunk = next;
                used = 0;
            }
        }
        let length = self.end();
        header[96..100].copy_from_slice(&transactions.to_le_bytes());
        header[196..204].copy_from_slice(&length.to_le_bytes());
        header[212..228].copy_from_slice(&fresh_guid()?);
        header[236..252].copy_from_slice(&fresh_guid()?);
        let generation = u64::from_le_bytes(header[228..236].try_into().unwrap())
            .checked_add(1)
            .ok_or(Error {
                offset: 228,
                message: "File generation counter is exhausted",
            })?;
        header[228..236].copy_from_slice(&generation.to_le_bytes());
        self.state.stamp = crate::Stamp { header, length };
        self.state.log = log;
        self.state.log_crc = committed_crc.unwrap();
        Ok(Some((
            crate::Transaction {
                base: self.base,
                append: self.append,
                patches: self.patches,
                header,
            },
            self.state,
        )))
    }
}

/// The `required` nodes of fragment `sequence` of list `id`, which `bytes` at `at` hold and
/// which ends its list.
fn nodes(
    bytes: &[u8],
    at: Chunk,
    id: u32,
    sequence: u32,
    required: usize,
) -> Result<Vec<Node<'_>>> {
    let fragment = Fragment::parse(bytes, at.offset as usize)?;
    if fragment.id != id || fragment.sequence != sequence || !fragment.next.absent() {
        return Err(Error {
            offset: at.offset as usize,
            message: "An appended fragment has the wrong list, sequence or successor",
        });
    }
    let mut nodes = Vec::new();
    fragment.nodes(required, &mut nodes, |_| Ok(()))?;
    if nodes.len() != required {
        return Err(Error {
            offset: at.offset as usize,
            message: "An appended fragment lacks logged nodes",
        });
    }
    Ok(nodes)
}

/// A revision `Appending::revision` wrote, as `check_transaction` expects to read it back.
pub(crate) struct Written<'r, 'a> {
    pub space: ExGuid,
    pub rid: ExGuid,
    pub previous: Option<ExGuid>,
    pub live: &'r LiveRevision<'a>,
    pub commit: &'r Commit,
}

/// Reads back what `transaction` appends to the section `before` describes, whose bytes
/// `segments` hold, and checks it stores `revisions` and `payloads` as `after` expects:
/// fragments link and decode to the logged counts under the logged checksum; each declared
/// object parses, names identities its group's table holds and objects reachable after its
/// revision, carries its incremental reference count and, if read-only, its MD5; roots stay
/// unless a space is new. Validates nothing the transaction leaves as it was.
pub(crate) fn check_transaction(
    before: &StoreState,
    after: &StoreState,
    transaction: &crate::Transaction,
    segments: &[(u64, &[u8])],
    revisions: &[Written<'_, '_>],
    payloads: &[([u8; 16], &[u8])],
) -> Result<()> {
    let wrong = |message| Error { offset: 0, message };
    let base = transaction.base.length;
    // A chunk's bytes with this transaction's writes: nothing read here is patched earlier.
    let read = |chunk: Chunk| -> Result<Vec<u8>> {
        let (start, end) = (chunk.offset, chunk.offset + chunk.length);
        let mut bytes = if start >= base {
            transaction
                .append
                .get((start - base) as usize..(end - base) as usize)
                .ok_or(wrong("An appended chunk lies outside the transaction"))?
                .to_vec()
        } else {
            let (offset, segment) = segments
                .iter()
                .rfind(|(offset, _)| *offset <= start)
                .ok_or(wrong("A chunk lies outside the store"))?;
            segment
                .get((start - offset) as usize..(end - offset) as usize)
                .ok_or(wrong("A chunk lies outside the store"))?
                .to_vec()
        };
        for (offset, patch) in &transaction.patches {
            let (from, to) = (start.max(*offset), end.min(offset + patch.len() as u64));
            if from < to {
                bytes[(from - start) as usize..(to - start) as usize]
                    .copy_from_slice(&patch[(from - offset) as usize..(to - offset) as usize]);
            }
        }
        Ok(bytes)
    };
    let header = &transaction.header;
    let count = |header: &[u8; 1024]| u32::from_le_bytes(header[96..100].try_into().unwrap());
    if *header != after.stamp.header
        || count(header) != count(&before.stamp.header) + 1
        || u64::from_le_bytes(header[196..204].try_into().unwrap()) != after.stamp.length
        || after.stamp.length != base + transaction.append.len() as u64
    {
        return Err(wrong("The transaction header does not commit its bytes"));
    }

    // The log: entries from where it ended, through the sentinel that commits them.
    let mut counts = BTreeMap::new();
    let (mut chunk, mut used) = before.log;
    let mut checksum = before.log_crc;
    loop {
        let capacity = (chunk.length as usize - 12) & !7;
        let bytes = read(chunk)?;
        if used == capacity {
            let next = u64::from_le_bytes(bytes[capacity..capacity + 8].try_into().unwrap());
            let length = u32::from_le_bytes(bytes[capacity + 8..capacity + 12].try_into().unwrap());
            (chunk, used) = (
                Chunk {
                    offset: next,
                    length: u64::from(length),
                },
                0,
            );
            continue;
        }
        let entry = &bytes[used..used + 8];
        used += 8;
        let id = u32::from_le_bytes(entry[..4].try_into().unwrap());
        let value = u32::from_le_bytes(entry[4..].try_into().unwrap());
        if id == 1 {
            if value != !checksum {
                return Err(wrong("The transaction log checksum does not match"));
            }
            checksum = transaction_crc(checksum, entry, FileType::Section, used == capacity);
            break;
        }
        checksum = transaction_crc(checksum, entry, FileType::Section, used == capacity);
        counts.insert(id, value as usize);
    }
    if (chunk, used) != after.log || checksum != after.log_crc {
        return Err(wrong("The transaction log ends elsewhere than its state"));
    }

    // Nodes a list gained: the fragment `after` ends with, linked from where `before` ended.
    let gained = |before: Option<&ListTail>, after: &ListTail| -> Result<Vec<u8>> {
        if counts.get(&after.id) != Some(&after.nodes) {
            return Err(wrong("A list's nodes disagree with the transaction log"));
        }
        if let Some(before) = before {
            let previous = read(before.last)?;
            if Fragment::parse(&previous, before.last.offset as usize)?.next != after.last
                || after.fragments != before.fragments + 1
                || after.id != before.id
            {
                return Err(wrong("An appended fragment is not linked to its list"));
            }
        }
        read(after.last)
    };
    for written in revisions {
        let Written {
            space,
            rid,
            live,
            commit,
            ..
        } = *written;
        let old = before.spaces.get(&space);
        let new = after
            .spaces
            .get(&space)
            .ok_or(wrong("A sealed space has no list"))?;
        let bytes = gained(old, new)?;
        let manifest = nodes(
            &bytes,
            new.last,
            new.id,
            old.map_or(0, |old| old.fragments),
            new.nodes - old.map_or(0, |old| old.nodes),
        )?;
        let mut manifest = manifest.iter();
        let mut next = || {
            manifest
                .next()
                .ok_or(wrong("A revision manifest is truncated"))
        };
        let mut node = next()?;
        if old.is_none() {
            let mut c = Cursor {
                bytes: node.payload,
                offset: node.offset,
            };
            if node.id != 0x14 || c.exguid()? != space {
                return Err(wrong("A new space's revision list names another space"));
            }
            node = next()?;
        }
        let mut c = Cursor {
            bytes: node.payload,
            offset: node.offset,
        };
        let dependency = if commit.checkpoint {
            ExGuid::default()
        } else {
            written
                .previous
                .ok_or(wrong("A dependent revision follows none"))?
        };
        if node.id != 0x1e
            || c.exguid()? != rid
            || c.exguid()? != dependency
            || c.read::<4>()? != 1_u32.to_le_bytes()
            || c.read::<2>()? != [0, 0]
        {
            return Err(wrong(
                "A revision starts with the wrong identity or dependency",
            ));
        }
        let mut declared = BTreeSet::new();
        let mut roots = BTreeMap::new();
        loop {
            let node = next()?;
            match node.id {
                0xb0 => {
                    let Some(Reference::NodeList(at)) = node.reference else {
                        return Err(wrong("An object group lacks its list"));
                    };
                    let bytes = read(at)?;
                    let id = Fragment::parse(&bytes, at.offset as usize)?.id;
                    let required = *counts
                        .get(&id)
                        .ok_or(wrong("An object group is not logged"))?;
                    let group = nodes(&bytes, at, id, 0, required)?;
                    let mut table = BTreeMap::new();
                    let mut override_crc = u32::MAX;
                    for item in &group {
                        let mut c = Cursor {
                            bytes: item.payload,
                            offset: item.offset,
                        };
                        match item.id {
                            0xb4 | 0x22 | 0x28 | 0xb8 => {}
                            0x24 => {
                                table.insert(u32::from_le_bytes(c.read()?), c.read::<16>()?);
                            }
                            0xa5 | 0xc5 | 0x73 => {
                                let id = c.compact(&table)?;
                                let object = live
                                    .revision
                                    .objects
                                    .get(&id)
                                    .ok_or(wrong("A revision declares an unknown object"))?;
                                let jcid = u32::from_le_bytes(c.read()?);
                                let flags = if item.id == 0x73 {
                                    None
                                } else {
                                    Some(c.read::<1>()?[0])
                                };
                                let count = u32::from_le_bytes(c.read()?);
                                if !declared.insert(id)
                                    || jcid != object.jcid
                                    || count != object.reference_count
                                {
                                    return Err(wrong(
                                        "A declaration disagrees with its object or its count",
                                    ));
                                }
                                override_crc =
                                    crc(override_crc, &count.to_le_bytes(), FileType::Section);
                                if item.id == 0x73 {
                                    continue;
                                }
                                let Some(Reference::Data(at)) = item.reference else {
                                    return Err(wrong("An object declaration lacks its data"));
                                };
                                let data = read(at)?;
                                let stored = crate::Object {
                                    jcid,
                                    reference_count: count,
                                    data: ObjectData::Properties(&data),
                                    global_ids: Arc::new(table.clone()),
                                };
                                let references = stored.references()?;
                                let expected = u8::from(!references.objects.is_empty())
                                    | (u8::from(
                                        !references.object_spaces.is_empty()
                                            || !references.contexts.is_empty(),
                                    ) << 1);
                                if object.data != ObjectData::Properties(&data)
                                    || flags != Some(expected)
                                    || (item.id == 0xc5) != (jcid & 0x100000 != 0)
                                    || (item.id == 0xc5 && c.read::<16>()? != md5::compute(&data).0)
                                {
                                    return Err(wrong(
                                        "An object's stored bytes differ from its revision",
                                    ));
                                }
                                if live.is_reachable(id)
                                    && !references
                                        .objects
                                        .iter()
                                        .all(|target| live.is_reachable(*target))
                                {
                                    return Err(wrong(
                                        "An object references one its revision cannot reach",
                                    ));
                                }
                            }
                            _ => return Err(wrong("Unexpected node in an object group")),
                        }
                    }
                    let node = next()?;
                    let mut c = Cursor {
                        bytes: node.payload,
                        offset: node.offset,
                    };
                    if node.id != 0x84
                        || c.read::<8>()? != [0; 8]
                        || u32::from_le_bytes(c.read()?) != !override_crc
                    {
                        return Err(wrong("An object group's count overrides do not match"));
                    }
                }
                0x5a => {
                    let mut c = Cursor {
                        bytes: node.payload,
                        offset: node.offset,
                    };
                    let id = c.exguid()?;
                    roots.insert(u32::from_le_bytes(c.read()?), id);
                }
                0x1c => break,
                _ => return Err(wrong("Unexpected node in a revision manifest")),
            }
        }
        let expected: BTreeSet<ExGuid> = if commit.checkpoint {
            live.revision.objects.keys().copied().collect()
        } else {
            commit.changed.clone()
        };
        if declared != expected
            || (commit.checkpoint && roots != live.revision.roots)
            || (!commit.checkpoint && !roots.is_empty())
        {
            return Err(wrong(
                "A revision declares other objects or roots than it changed",
            ));
        }
    }

    if after.root.nodes != before.root.nodes {
        let bytes = gained(Some(&before.root), &after.root)?;
        let added = nodes(
            &bytes,
            after.root.last,
            after.root.id,
            before.root.fragments,
            after.root.nodes - before.root.nodes,
        )?;
        let created = revisions
            .iter()
            .filter(|written| !before.spaces.contains_key(&written.space))
            .count();
        let spaces = added.iter().filter(|node| node.id == 8).count();
        if spaces != created || added.iter().any(|node| !matches!(node.id, 8 | 0x90)) {
            return Err(wrong(
                "The root list gains other nodes than new spaces declare",
            ));
        }
    }
    if !payloads.is_empty() {
        let files = after
            .files
            .as_ref()
            .ok_or(wrong("Embedded payloads have no store"))?;
        let bytes = gained(before.files.as_ref(), files)?;
        let declared = nodes(
            &bytes,
            files.last,
            files.id,
            before.files.map_or(0, |files| files.fragments),
            payloads.len(),
        )?;
        for ((guid, payload), node) in payloads.iter().zip(&declared) {
            let Some(Reference::Data(at)) = node.reference else {
                return Err(wrong("A payload declaration lacks its data"));
            };
            let data = read(at)?;
            let blob = crate::files::payload(&data, at.offset as usize);
            if node.id != 0x94 || node.payload != guid || blob.ok() != Some(*payload) {
                return Err(wrong("An embedded payload differs from its declaration"));
            }
        }
    }
    Ok(())
}
