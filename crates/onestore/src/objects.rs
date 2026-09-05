use crate::{
    Error, ExGuid, IdStream, Node, PropertySets, Reference, RevisionIndex, Value, bytes::Cursor,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

type Result<T> = std::result::Result<T, Error>;
type GlobalIds = BTreeMap<u32, [u8; 16]>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectData<'a> {
    Properties(&'a [u8]),
    Encrypted(&'a [u8]),
    File {
        reference: &'a [u8],
        extension: &'a [u8],
    },
}

#[derive(Debug)]
pub struct Object<'a> {
    pub jcid: u32,
    pub reference_count: u32,
    pub data: ObjectData<'a>,
    pub global_ids: Arc<GlobalIds>,
}

#[derive(Debug)]
pub struct ResolvedRevision<'a> {
    pub roots: BTreeMap<u32, ExGuid>,
    pub objects: BTreeMap<ExGuid, Object<'a>>,
}

/// Occurrences retain multiplicity for native reference counts.
#[derive(Debug, Default)]
pub struct ObjectReferences {
    pub objects: Vec<ExGuid>,
    pub object_spaces: Vec<ExGuid>,
    pub contexts: Vec<ExGuid>,
}

impl Object<'_> {
    pub fn references(&self) -> Result<ObjectReferences> {
        let mut result = ObjectReferences::default();
        let bytes = match self.data {
            ObjectData::File { .. } => return Ok(result),
            ObjectData::Properties(bytes) => bytes,
            ObjectData::Encrypted(_) => {
                return Err(Error {
                    offset: 0,
                    message: "Encrypted property references are unavailable",
                });
            }
        };
        let properties = PropertySets::parse(bytes)?;
        for property in properties.sets.iter().flatten() {
            if let Value::References {
                stream,
                compact_ids,
            } = property.value
            {
                let targets = match stream {
                    IdStream::Objects => &mut result.objects,
                    IdStream::ObjectSpaces => &mut result.object_spaces,
                    IdStream::Contexts => &mut result.contexts,
                };
                let mut c = Cursor {
                    bytes: compact_ids,
                    offset: 0,
                };
                while !c.bytes.is_empty() {
                    targets.push(c.compact(&self.global_ids)?);
                }
            }
        }
        Ok(result)
    }
}

impl ResolvedRevision<'_> {
    pub fn reachable(&self) -> Result<BTreeSet<ExGuid>> {
        let mut pending: Vec<_> = self.roots.values().copied().collect();
        let mut incoming = BTreeMap::<ExGuid, u32>::new();
        for id in &pending {
            if incoming.insert(*id, 1).is_some() {
                return Err(Error {
                    offset: 0,
                    message: "Object is the root of multiple roles",
                });
            }
        }
        let mut edges = BTreeMap::new();
        while let Some(id) = pending.pop() {
            if edges.contains_key(&id) {
                continue;
            }
            let object = self.objects.get(&id).ok_or(Error {
                offset: 0,
                message: "Reachable object has no declaration",
            })?;
            let references = object.references()?;
            for child in &references.objects {
                let count = incoming.entry(*child).or_default();
                *count = count.checked_add(1).ok_or(Error {
                    offset: 0,
                    message: "Object reference count overflows",
                })?;
                pending.push(*child);
            }
            edges.insert(id, references.objects);
        }
        let mut remaining = incoming.clone();
        for id in self.roots.values() {
            *remaining.get_mut(id).unwrap() -= 1;
        }
        pending.extend(
            remaining
                .iter()
                .filter_map(|(id, count)| (*count == 0).then_some(*id)),
        );
        let mut visited = 0;
        while let Some(id) = pending.pop() {
            visited += 1;
            for child in &edges[&id] {
                let count = remaining.get_mut(child).unwrap();
                *count -= 1;
                if *count == 0 {
                    pending.push(*child);
                }
            }
        }
        if visited != edges.len() {
            return Err(Error {
                offset: 0,
                message: "Object references form a cycle",
            });
        }
        for (id, count) in &incoming {
            if self.objects[id].reference_count != *count {
                return Err(Error {
                    offset: 0,
                    message: "Stored object reference count disagrees with the reachable graph",
                });
            }
        }
        Ok(edges.into_keys().collect())
    }
}

impl Cursor<'_> {
    pub(crate) fn compact(&mut self, table: &GlobalIds) -> Result<ExGuid> {
        let raw = u32::from_le_bytes(self.read()?);
        let guid = *table.get(&(raw >> 8)).ok_or(Error {
            offset: self.offset - 4,
            message: "Compact ID refers to a missing global ID",
        })?;
        Ok(ExGuid {
            guid,
            n: raw & 0xff,
        })
    }
}

impl<'a> Cursor<'a> {
    fn storage_string(&mut self) -> Result<&'a [u8]> {
        let count = u32::from_le_bytes(self.read()?);
        let size = usize::try_from(count)
            .ok()
            .and_then(|n| n.checked_mul(2))
            .ok_or(Error {
                offset: self.offset - 4,
                message: "String length exceeds address space",
            })?;
        self.take(size)
    }
}

impl<'a> RevisionIndex<'a> {
    pub fn validate_current(&self) -> Result<()> {
        let mut edges = BTreeMap::new();
        let mut incoming = BTreeMap::<_, usize>::new();
        for (osid, space) in &self.spaces {
            for rid in space.labels.values().copied().collect::<BTreeSet<_>>() {
                let revision = self.resolve(*osid, rid)?;
                let mut targets = BTreeSet::new();
                for oid in revision.reachable()? {
                    let references = revision.objects[&oid].references()?;
                    for target in references.object_spaces {
                        if target == *osid {
                            return Err(Error {
                                offset: 0,
                                message: "Object references its own object space",
                            });
                        }
                        let rid = self
                            .spaces
                            .get(&target)
                            .and_then(|space| space.labels.get(&(ExGuid::default(), 1)))
                            .ok_or(Error {
                                offset: 0,
                                message: "Object-space reference has no default revision",
                            })?;
                        targets.insert((target, *rid));
                    }
                    for context in references.contexts {
                        if !space.labels.contains_key(&(context, 1)) {
                            return Err(Error {
                                offset: 0,
                                message: "Context reference has no current revision",
                            });
                        }
                    }
                }
                for target in &targets {
                    *incoming.entry(*target).or_default() += 1;
                }
                incoming.entry((*osid, rid)).or_default();
                edges.insert((*osid, rid), targets);
            }
        }
        let mut pending: Vec<_> = incoming
            .iter()
            .filter_map(|(id, count)| (*count == 0).then_some(*id))
            .collect();
        let mut visited = 0;
        while let Some(id) = pending.pop() {
            visited += 1;
            for target in &edges[&id] {
                let count = incoming.get_mut(target).unwrap();
                *count -= 1;
                if *count == 0 {
                    pending.push(*target);
                }
            }
        }
        if visited != edges.len() {
            return Err(Error {
                offset: 0,
                message: "Object-space references form a cycle",
            });
        }
        Ok(())
    }

    pub fn resolve(&self, space: ExGuid, revision: ExGuid) -> Result<ResolvedRevision<'a>> {
        let space = self.spaces.get(&space).ok_or(Error {
            offset: 0,
            message: "Unknown object space",
        })?;
        let mut chain = Vec::new();
        let mut visited = BTreeSet::new();
        let mut next = Some(revision);
        while let Some(id) = next {
            let revision = space.revisions.get(&id).ok_or(Error {
                offset: 0,
                message: "Unknown revision dependency",
            })?;
            if !visited.insert(id) {
                return Err(Error {
                    offset: 0,
                    message: "Revision dependency cycle",
                });
            }
            chain.push(revision);
            next = revision.dependency;
        }
        let mut result = ResolvedRevision {
            roots: BTreeMap::new(),
            objects: BTreeMap::new(),
        };
        let mut table = Arc::new(GlobalIds::new());
        let mut signed_data = BTreeMap::new();
        for revision in chain.into_iter().rev() {
            let dependency_table = table;
            table = Arc::new(GlobalIds::new());
            let mut pending: Vec<&Node<'a>> = revision.nodes.iter().rev().collect();
            let mut groups = BTreeSet::new();
            let mut defining_table = false;
            let initial_crc = if self.store.header.file_type == crate::FileType::Section {
                u32::MAX
            } else {
                0
            };
            let mut override_crc = initial_crc;
            let mut in_group = false;
            let mut signature = ExGuid::default();
            while let Some(node) = pending.pop() {
                let mut c = node.fields(self.store);
                match node.id {
                    0xb0 => {
                        override_crc = initial_crc;
                        in_group = true;
                        let id = c.exguid()?;
                        if !groups.insert(id) {
                            return Err(Error {
                                offset: node.offset,
                                message: "Repeated object group",
                            });
                        }
                        let group = node.referenced_list(self.store)?;
                        let first = group.first().ok_or(Error {
                            offset: node.offset,
                            message: "Empty object group",
                        })?;
                        if first.id != 0xb4
                            || first.fields(self.store).exguid()? != id
                            || group.last().is_none_or(|node| node.id != 0xb8)
                        {
                            return Err(Error {
                                offset: node.offset,
                                message: "Object-group identity or boundaries do not match",
                            });
                        }
                        if group[1..group.len() - 1].iter().any(|node| {
                            !matches!(
                                node.id,
                                0x22 | 0x24 | 0x28 | 0x8c | 0xa4 | 0xa5 | 0xc4 | 0xc5 | 0x72 | 0x73
                            )
                        }) {
                            return Err(Error {
                                offset: node.offset,
                                message: "Unexpected node in an object group",
                            });
                        }
                        pending.extend(group[1..].iter().rev());
                    }
                    0x21 | 0x22 => {
                        if defining_table {
                            return Err(Error {
                                offset: node.offset,
                                message: "Unterminated global identification table",
                            });
                        }
                        table = Arc::new(GlobalIds::new());
                        defining_table = true;
                    }
                    0x24..=0x26 => {
                        if !defining_table {
                            return Err(Error {
                                offset: node.offset,
                                message: "Global ID entry outside a table",
                            });
                        }
                        let first = u32::from_le_bytes(c.read()?);
                        let entries: Vec<_> = match node.id {
                            0x24 => vec![(first, c.read()?)],
                            0x25 | 0x26 => {
                                let count = if node.id == 0x26 {
                                    u32::from_le_bytes(c.read()?)
                                } else {
                                    1
                                };
                                let to = u32::from_le_bytes(c.read()?);
                                let end = first.checked_add(count).ok_or(Error {
                                    offset: node.offset,
                                    message: "Global ID import range overflows",
                                })?;
                                if u64::from(count) > dependency_table.len() as u64
                                    || to.checked_add(count).is_none_or(|end| end > 0xffffff)
                                {
                                    return Err(Error {
                                        offset: node.offset,
                                        message: "Global ID import range exceeds its table",
                                    });
                                }
                                let entries: Vec<_> = dependency_table
                                    .range(first..end)
                                    .map(|(from, guid)| (to + from - first, *guid))
                                    .collect();
                                if entries.len() as u64 != u64::from(count) {
                                    return Err(Error {
                                        offset: node.offset,
                                        message: "Global ID import refers to a missing entry",
                                    });
                                }
                                entries
                            }
                            _ => unreachable!(),
                        };
                        for (index, guid) in entries {
                            if index >= 0xffffff
                                || guid == [0; 16]
                                || Arc::make_mut(&mut table).insert(index, guid).is_some()
                            {
                                return Err(Error {
                                    offset: node.offset,
                                    message: "Invalid or repeated global ID entry",
                                });
                            }
                        }
                    }
                    0x28 => {
                        if !defining_table {
                            return Err(Error {
                                offset: node.offset,
                                message: "Global ID table end without a start",
                            });
                        }
                        defining_table = false;
                        let unique: BTreeSet<_> = table.values().collect();
                        if unique.len() != table.len() {
                            return Err(Error {
                                offset: node.offset,
                                message: "Global ID table repeats a GUID",
                            });
                        }
                    }
                    0x59 | 0x5a => {
                        let id = if node.id == 0x5a {
                            c.exguid()?
                        } else {
                            c.compact(&table)?
                        };
                        let role = u32::from_le_bytes(c.read()?);
                        result.roots.insert(role, id);
                    }
                    0x2d | 0x2e | 0x41 | 0x42 | 0xa4 | 0xa5 | 0xc4 | 0xc5 | 0x72 | 0x73 => {
                        if defining_table {
                            return Err(Error {
                                offset: node.offset,
                                message: "Object declared inside a global ID table",
                            });
                        }
                        let id = c.compact(&table)?;
                        let jcid = match node.id {
                            0x2d | 0x2e => {
                                let bits = u16::from_le_bytes(c.read()?);
                                c.take(4)?;
                                if bits & 0x3fff != 1 {
                                    return Err(Error {
                                        offset: node.offset,
                                        message: "Invalid table-of-contents object type",
                                    });
                                }
                                0x20001
                            }
                            0x41 | 0x42 => {
                                result
                                    .objects
                                    .get(&id)
                                    .ok_or(Error {
                                        offset: node.offset,
                                        message: "Object revision has no previous declaration",
                                    })?
                                    .jcid
                            }
                            _ => u32::from_le_bytes(c.read()?),
                        };
                        let reference_count = match node.id {
                            0x41 => u32::from(c.read::<1>()?[0] >> 2),
                            0x42 => {
                                c.take(4)?;
                                u32::from_le_bytes(c.read()?)
                            }
                            0x2d | 0x72 => u32::from(c.read::<1>()?[0]),
                            0x2e | 0x73 => u32::from_le_bytes(c.read()?),
                            0xa4 | 0xc4 => {
                                c.take(1)?;
                                u32::from(c.read::<1>()?[0])
                            }
                            _ => {
                                c.take(1)?;
                                u32::from_le_bytes(c.read()?)
                            }
                        };
                        if in_group
                            || self.store.header.file_type == crate::FileType::TableOfContents
                        {
                            override_crc = crate::store::crc(
                                override_crc,
                                &reference_count.to_le_bytes(),
                                self.store.header.file_type,
                            );
                        }
                        let data = if matches!(node.id, 0x72 | 0x73) {
                            if jcid & 0x1fffff != 0x80000 | (jcid & 0xffff) {
                                return Err(Error {
                                    offset: node.offset,
                                    message: "Invalid file-data object flags",
                                });
                            }
                            ObjectData::File {
                                reference: c.storage_string()?,
                                extension: c.storage_string()?,
                            }
                        } else {
                            let Some(Reference::Data(chunk)) = node.reference else {
                                return Err(Error {
                                    offset: node.offset,
                                    message: "Object lacks a data reference",
                                });
                            };
                            let bytes = self.store.chunk_data(chunk)?;
                            if jcid & 0x20000 == 0
                                || (jcid & 0x100000 != 0) != matches!(node.id, 0xc4 | 0xc5)
                            {
                                return Err(Error {
                                    offset: node.offset,
                                    message: "Object declaration does not match its property-set flags",
                                });
                            }
                            if matches!(node.id, 0xc4 | 0xc5) {
                                let expected = c.read::<16>()?;
                                if !revision.encrypted && md5::compute(bytes).0 != expected {
                                    return Err(Error {
                                        offset: node.offset,
                                        message: "Read-only object checksum mismatch",
                                    });
                                }
                            }
                            if revision.encrypted {
                                ObjectData::Encrypted(bytes)
                            } else {
                                ObjectData::Properties(bytes)
                            }
                        };
                        if let Some(previous) = result.objects.get(&id) {
                            if previous.jcid & 0x1bffff != jcid & 0x1bffff {
                                return Err(Error {
                                    offset: node.offset,
                                    message: "Object revision changes its type",
                                });
                            }
                            if jcid & 0x180000 != 0 && previous.data != data {
                                return Err(Error {
                                    offset: node.offset,
                                    message: "Immutable object data changed",
                                });
                            }
                        }
                        if signature != ExGuid::default()
                            && signed_data
                                .insert((id, signature), data)
                                .is_some_and(|previous| previous != data)
                        {
                            return Err(Error {
                                offset: node.offset,
                                message: "Object data changed under the same data signature",
                            });
                        }
                        result.objects.insert(
                            id,
                            Object {
                                jcid,
                                reference_count,
                                data,
                                global_ids: Arc::clone(&table),
                            },
                        );
                    }
                    0x84 => {
                        let Some(Reference::Data(chunk)) = node.reference else {
                            return Err(Error {
                                offset: node.offset,
                                message: "Reference-count override lacks its data reference",
                            });
                        };
                        if !chunk.absent() {
                            c = Cursor {
                                bytes: self.store.chunk_data(chunk)?,
                                offset: usize::try_from(chunk.offset).unwrap(),
                            };
                        }
                        let small = u32::from_le_bytes(c.read()?);
                        let large = u32::from_le_bytes(c.read()?);
                        let expected = u32::from_le_bytes(c.read()?);
                        let start = c.bytes;
                        for (count, width) in [(small, 1), (large, 4)] {
                            for _ in 0..count {
                                let id = c.compact(&table)?;
                                let value = if width == 1 {
                                    u32::from(c.read::<1>()?[0])
                                } else {
                                    u32::from_le_bytes(c.read()?)
                                };
                                let object = result.objects.get_mut(&id).ok_or(Error { offset: node.offset, message: "Reference-count override targets an undeclared object" })?;
                                object.reference_count = value;
                            }
                        }
                        override_crc = crate::store::crc(
                            override_crc,
                            &start[..start.len() - c.bytes.len()],
                            self.store.header.file_type,
                        );
                        let actual = if self.store.header.file_type == crate::FileType::Section {
                            !override_crc
                        } else {
                            override_crc
                        };
                        if expected != actual {
                            return Err(Error {
                                offset: node.offset,
                                message: "Reference-count override checksum mismatch",
                            });
                        }
                        override_crc = initial_crc;
                    }
                    0x7c => {}
                    0x8c => {
                        signature = c.exguid()?;
                    }
                    0xb8 => {
                        in_group = false;
                        signature = ExGuid::default();
                    }
                    _ => {
                        return Err(Error {
                            offset: node.offset,
                            message: "Unsupported node in a revision manifest",
                        });
                    }
                }
            }
            if defining_table {
                return Err(Error {
                    offset: 0,
                    message: "Unterminated global identification table",
                });
            }
        }
        Ok(result)
    }
}
