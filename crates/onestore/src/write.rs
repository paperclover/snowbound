use crate::{
    Chunk, Error, ExGuid, FileType, ObjectData, PropertySets, Reference, RevisionIndex, Store,
    Value,
    store::{crc, transaction_crc},
};

use std::collections::{BTreeMap, BTreeSet};

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

fn patch_properties(
    blob: &[u8],
    updates: &[(u32, &[u8])],
    insert: Option<(u32, &[u8])>,
) -> Result<Vec<u8>> {
    let properties = PropertySets::parse(blob)?;
    let mut patches = Vec::new();
    for (i, &(property, value)) in updates.iter().chain(insert.iter()).enumerate() {
        if updates[..i.min(updates.len())]
            .iter()
            .any(|(id, _)| *id == property)
        {
            return Err(Error {
                offset: 0,
                message: "Duplicate property update",
            });
        }
        let kind = (property >> 26) & 0x1f;
        if !(3..=7).contains(&kind) {
            return Err(Error {
                offset: 0,
                message: "Property does not contain scalar bytes",
            });
        }
        if (kind == 7 && value.len() >= 0x40000000) || (kind != 7 && value.len() != 1 << (kind - 3))
        {
            return Err(Error {
                offset: 0,
                message: "Replacement has an invalid property length",
            });
        }
        let matches: Vec<_> = properties.sets[0]
            .iter()
            .filter(|candidate| candidate.id == property)
            .collect();
        let adding = i == updates.len();
        if matches.len() != usize::from(!adding) {
            return Err(Error {
                offset: 0,
                message: "Property is missing or duplicated",
            });
        }
        let previous = if adding {
            None
        } else {
            let Value::Bytes(previous) = matches[0].value else {
                return Err(Error {
                    offset: 0,
                    message: "Property does not contain scalar bytes",
                });
            };
            if previous == value {
                continue;
            }
            Some(previous)
        };
        let mut encoded = Vec::new();
        if kind == 7 {
            encoded.extend_from_slice(&u32::try_from(value.len()).unwrap().to_le_bytes());
        }
        encoded.extend_from_slice(value);
        if let Some(previous) = previous {
            let start = previous.as_ptr().addr() - blob.as_ptr().addr();
            patches.push((
                start - if kind == 7 { 4 } else { 0 },
                start + previous.len(),
                encoded,
            ));
        } else {
            let count = u16::try_from(properties.sets[0].len() + 1).map_err(|_| Error {
                offset: 0,
                message: "Root property count exceeds the format limit",
            })?;
            let ids = properties.root_ids.as_ptr().addr() - blob.as_ptr().addr();
            patches.push((ids - 2, ids, count.to_le_bytes().to_vec()));
            let end = ids + properties.root_ids.len();
            patches.push((end, end, property.to_le_bytes().to_vec()));
            let end = blob.len() - properties.padding.len();
            patches.push((end, end, encoded));
        }
    }
    if patches.is_empty() {
        return Ok(blob.to_vec());
    }
    patches.sort_by_key(|(start, end, _)| (*start, *end));
    let mut changed = Vec::new();
    let mut cursor = 0;
    for (start, end, value) in patches {
        changed.extend_from_slice(&blob[cursor..start]);
        changed.extend_from_slice(&value);
        cursor = end;
    }
    changed.extend_from_slice(&blob[cursor..blob.len() - properties.padding.len()]);
    changed.resize(changed.len().next_multiple_of(8), 0);
    PropertySets::parse(&changed)?;

    Ok(changed)
}

pub(crate) struct ObjectEdit<'a> {
    pub object: ExGuid,
    pub updates: &'a [(u32, &'a [u8])],
    pub insert: Option<(u32, &'a [u8])>,
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
    replace_objects(
        source,
        space,
        &[ObjectEdit {
            object: object_id,
            updates: &[(property, value)],
            insert: None,
        }],
    )
}

pub(crate) fn replace_objects(
    source: &[u8],
    space: ExGuid,
    edits: &[ObjectEdit<'_>],
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
    let rid = *index
        .spaces
        .get(&space)
        .and_then(|space| space.labels.get(&(ExGuid::default(), 1)))
        .ok_or(Error {
            offset: 0,
            message: "Object space has no active default revision",
        })?;
    let revision = index.resolve(space, rid)?;
    let reachable = revision.reachable()?;
    let mut changed = BTreeMap::new();
    for (i, edit) in edits.iter().enumerate() {
        if edits[..i]
            .iter()
            .any(|previous| previous.object == edit.object)
        {
            return Err(Error {
                offset: 0,
                message: "Duplicate object edit",
            });
        }
        if !reachable.contains(&edit.object) {
            return Err(Error {
                offset: 0,
                message: "Object is not reachable in the active revision",
            });
        }
        let object = &revision.objects[&edit.object];
        if object.jcid & 0x100000 != 0 {
            return Err(Error {
                offset: 0,
                message: "Read-only object requires a new identity",
            });
        }
        let ObjectData::Properties(blob) = object.data else {
            return Err(Error {
                offset: 0,
                message: "Object does not contain editable properties",
            });
        };
        let bytes = patch_properties(blob, edit.updates, edit.insert)?;
        if bytes != blob {
            changed.insert(edit.object, bytes);
        }
    }
    if changed.is_empty() {
        return Ok(source.to_vec());
    }

    // Native cold-open fails on long dependency chains; cap their depth at 512.
    let checkpoint = std::iter::successors(Some(rid), |id| {
        index.spaces[&space].revisions[id].dependency
    })
    .nth(511)
    .is_some();
    let selected: Vec<_> = revision
        .objects
        .iter()
        .filter(|(id, _)| checkpoint || changed.contains_key(id))
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
    let mut output = source.to_vec();
    let maximum = store
        .transaction_fragments
        .iter()
        .flat_map(|fragment| fragment.entries.chunks_exact(8))
        .map(|entry| u32::from_le_bytes(entry[..4].try_into().unwrap()))
        .max()
        .unwrap();
    let new_rid = ExGuid {
        guid: fresh_guid()?,
        n: 1,
    };
    let mut start = Vec::new();
    new_rid.encode(&mut start);
    if checkpoint { ExGuid::default() } else { rid }.encode(&mut start);
    if !is_section {
        start.extend_from_slice(&0_u64.to_le_bytes());
    }
    start.extend_from_slice(&1_u32.to_le_bytes());
    start.extend_from_slice(&0_u16.to_le_bytes());
    let mut manifest = vec![node(if is_section { 0x1e } else { 0x1b }, None, &start)?];
    let mut group_counts = Vec::new();
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
                ObjectData::Properties(previous) => {
                    let bytes = changed.get(&id).map(Vec::as_slice).unwrap_or(previous);
                    let references = object.references()?;
                    let flags = u8::from(!references.objects.is_empty())
                        | (u8::from(
                            !references.object_spaces.is_empty() || !references.contexts.is_empty(),
                        ) << 1);
                    let data = if toc_table.is_some() {
                        let mut mapped = bytes.to_vec();
                        for property in PropertySets::parse(bytes)?.sets.iter().flatten() {
                            if let Value::References { compact_ids, .. } = property.value {
                                let offset = compact_ids.as_ptr().addr() - bytes.as_ptr().addr();
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
                    } else if changed.contains_key(&id) {
                        append(&mut output, bytes)?
                    } else {
                        Chunk {
                            offset: u64::try_from(bytes.as_ptr().addr() - source.as_ptr().addr())
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
            let group_id = u32::try_from(group_counts.len())
                .ok()
                .and_then(|n| n.checked_add(1))
                .and_then(|n| maximum.checked_add(n))
                .ok_or(Error {
                    offset: 0,
                    message: "File-node list identities are exhausted",
                })?;
            let chunk = append_list(&mut output, group_id, &group)?;
            group_counts.push((group_id, group.len()));
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
    let last_fragment = *revision_list.fragments.last().unwrap();
    let list_start = usize::try_from(last_fragment.offset).unwrap();
    let list_id = u32::from_le_bytes(source[list_start + 8..list_start + 12].try_into().unwrap());
    let manifest_chunk = append_list(&mut output, list_id, &manifest)?;
    let manifest_start = usize::try_from(manifest_chunk.offset).unwrap();
    let sequence = u32::try_from(revision_list.fragments.len()).map_err(|_| Error {
        offset: list_start,
        message: "File-node fragment sequences are exhausted",
    })?;
    output[manifest_start + 12..manifest_start + 16].copy_from_slice(&sequence.to_le_bytes());
    let last_node = revision_list.nodes.last().unwrap();
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
    output[tail..tail + 8].copy_from_slice(&manifest_chunk.offset.to_le_bytes());
    output[tail + 8..tail + 12].copy_from_slice(&(manifest_chunk.length as u32).to_le_bytes());

    let transactions = store.header.transaction_count.checked_add(1).ok_or(Error {
        offset: 96,
        message: "Transaction counter is exhausted",
    })?;
    let changed_bytes = store.header.transaction_count ^ transactions;
    let commit_byte = (31 - changed_bytes.leading_zeros()) / 8;
    let ceiling = transactions | ((1_u32 << (commit_byte * 8)) - 1);
    let mut entries = Vec::new();
    for (id, count) in group_counts
        .into_iter()
        .chain([(list_id, revision_list.nodes.len() + manifest.len())])
    {
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
    written_index.resolve(space, new_rid)?.reachable()?;
    Ok(output)
}
