use super::*;
use onestore::{FileDataReference, IdStream, ObjectData, PropertySets, ResolvedRevision, Value};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
enum Content {
    Legacy([u8; 32]),
    Semantic { sha256: [u8; 32] },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Observed {
    path: Vec<(ExGuid, u8)>,
    siblings: BTreeMap<ExGuid, bool>,
    destination: Vec<(ExGuid, u8)>,
    content: Option<Content>,
}

/// A subtree intent with observed placement, deletion content and replacement identities.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TreeEdit {
    pub intent: onestore::TreeEdit,
    observed: Observed,
    created: BTreeSet<ExGuid>,
}

fn fingerprint(
    store: &Store<'_>,
    raw: &ResolvedRevision<'_>,
    root: ExGuid,
    legacy: bool,
) -> Result<[u8; 32]> {
    let mut objects = BTreeMap::new();
    let mut visiting = BTreeSet::new();
    let mut pending = vec![(root, false)];
    while let Some((id, complete)) = pending.pop() {
        if objects.contains_key(&id) {
            continue;
        }
        let object = &raw.objects[&id];
        let references = object.references()?;
        if !complete {
            if !visiting.insert(id) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Subtree property references form a cycle",
                )
                .into());
            }
            pending.push((id, true));
            pending.extend(references.objects.iter().map(|id| (*id, false)));
            continue;
        }
        visiting.remove(&id);
        let mut hash = Sha256::new();
        hash.update(object.jcid.to_le_bytes());
        match object.data {
            ObjectData::Properties(bytes) => {
                hash.update([0]);
                let properties = PropertySets::parse(bytes)?;
                let mut object_ids = references.objects.into_iter();
                let mut spaces = references.object_spaces.into_iter();
                let mut contexts = references.contexts.into_iter();
                let mut fields = Vec::new();
                for set in &properties.sets {
                    let mut values = BTreeMap::new();
                    for property in set {
                        let mut value = Sha256::new();
                        match &property.value {
                            Value::NoData => {}
                            Value::Bytes(bytes) => value.update(bytes),
                            Value::References {
                                stream,
                                compact_ids,
                            } => {
                                let references = match stream {
                                    IdStream::Objects => &mut object_ids,
                                    IdStream::ObjectSpaces => &mut spaces,
                                    IdStream::Contexts => &mut contexts,
                                };
                                for reference in references.take(compact_ids.len() / 4) {
                                    if !legacy && *stream == IdStream::Objects {
                                        let immutable =
                                            raw.objects[&reference].jcid & 0x100000 != 0;
                                        value.update([u8::from(immutable)]);
                                        if !immutable {
                                            value.update(reference.guid);
                                            value.update(reference.n.to_le_bytes());
                                        }
                                        value.update(objects[&reference]);
                                    } else {
                                        value.update(reference.guid);
                                        value.update(reference.n.to_le_bytes());
                                    }
                                }
                            }
                            Value::Sets(_) => {}
                        }
                        if property.id == 0x14001d7a
                            || (!legacy
                                && property.id == 0x24001c20
                                && matches!(&property.value, Value::References { compact_ids, .. } if compact_ids.is_empty()))
                        {
                            continue;
                        }
                        values.insert(property.id, value);
                    }
                    fields.push(values);
                }
                // Arena indices and CompactIDs can change without changing property content.
                let mut sets = vec![[0; 32]; properties.sets.len()];
                for at in (0..properties.sets.len()).rev() {
                    for property in &properties.sets[at] {
                        if let Value::Sets(children) = &property.value {
                            let value = fields[at].get_mut(&property.id).unwrap();
                            for child in children.clone() {
                                value.update(sets[child]);
                            }
                        }
                    }
                    let mut set = Sha256::new();
                    for (id, value) in &fields[at] {
                        set.update(id.to_le_bytes());
                        set.update(value.clone().finalize());
                    }
                    sets[at] = set.finalize().into();
                }
                hash.update(sets[0]);
            }
            ObjectData::File {
                reference,
                extension,
            } => {
                hash.update([1]);
                hash.update(Sha256::digest(reference));
                hash.update(Sha256::digest(extension));
                if let Some(FileDataReference::Internal(guid)) = object.file_reference()? {
                    hash.update(Sha256::digest(store.file_data(guid)?));
                }
            }
            ObjectData::Encrypted(_) => unreachable!("references rejected encrypted content"),
        }
        objects.insert(id, <[u8; 32]>::from(hash.finalize()));
    }
    if !legacy {
        return Ok(objects[&root]);
    }
    let mut hash = Sha256::new();
    for (id, value) in objects {
        hash.update(id.guid);
        hash.update(id.n.to_le_bytes());
        hash.update(value);
    }
    Ok(hash.finalize().into())
}

fn observe(
    source: &[u8],
    space: ExGuid,
    intent: &onestore::TreeEdit,
    legacy: bool,
) -> Result<Option<Observed>> {
    let store = Store::parse(source)?;
    let index = RevisionIndex::parse(&store)?;
    let document = Document::parse(&index)?;
    let Some(section) = document.spaces.get(&space) else {
        return Ok(None);
    };
    let Some(rid) = section.contexts.get(&ExGuid::default()) else {
        return Ok(None);
    };
    let view = &section.revisions[rid];
    let pages: Vec<_> = document
        .pages()?
        .into_iter()
        .filter_map(|(sid, page)| (sid == space).then_some(page))
        .collect();
    if pages.len() != 1 {
        return Ok(None);
    }
    let object = intent.object();
    let mut targets = vec![object];
    if let Some((parent, _)) = intent.destination() {
        targets.push(parent);
    }
    let Some(paths) = active_paths(view, &pages, &targets) else {
        return Ok(None);
    };
    let Some(parent) = paths[0].first() else {
        return Ok(None);
    };
    let children = &view.nodes[parent].children;
    let Some(position) = children.iter().position(|id| *id == object) else {
        return Ok(None);
    };
    let levels = |path: &[ExGuid]| {
        path.iter()
            .map(|id| (*id, view.nodes[id].child_level.unwrap_or(1)))
            .collect::<Vec<_>>()
    };
    let destination = if let Some((parent, _)) = intent.destination() {
        levels(&[&[parent], paths[1].as_slice()].concat())
    } else {
        Vec::new()
    };
    Ok(Some(Observed {
        path: levels(&paths[0]),
        siblings: children
            .iter()
            .enumerate()
            .filter_map(|(at, id)| (*id != object).then_some((*id, at < position)))
            .collect(),
        destination,
        content: if intent.destination().is_none() {
            let sha256 = fingerprint(&store, &index.resolve(space, *rid)?, object, legacy)?;
            Some(if legacy {
                Content::Legacy(sha256)
            } else {
                Content::Semantic { sha256 }
            })
        } else {
            None
        },
    }))
}

fn mutable_objects(source: &[u8], space: ExGuid) -> Result<BTreeSet<ExGuid>> {
    let store = Store::parse(source)?;
    let index = RevisionIndex::parse(&store)?;
    let raw = index.resolve(space, index.spaces[&space].labels[&(ExGuid::default(), 1)])?;
    Ok(raw
        .reachable()?
        .into_iter()
        .filter(|id| raw.objects[id].jcid & 0x100000 == 0)
        .collect())
}

impl Replica {
    /// Queues a move or deletion, retaining content changes for explicit deletion review.
    pub fn tree(
        &self,
        source: &[u8],
        space: ExGuid,
        intent: &onestore::TreeEdit,
    ) -> Result<Option<u64>> {
        let (edit, prepared) = TreeEdit::capture(source, space, intent)?;
        self.record(source, space, Operation::Tree(edit), &prepared)
    }
}

impl TreeEdit {
    pub(crate) fn capture<'a>(
        source: &'a [u8],
        space: ExGuid,
        intent: &onestore::TreeEdit,
    ) -> Result<(Self, PreparedEdit<'a>)> {
        let prepared = PreparedEdit::tree(source, space, intent)?;
        let observed = observe(source, space, intent, false)?.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "Prepared subtree edit has no active target",
            )
        })?;
        let before = mutable_objects(source, space)?;
        let after = mutable_objects(prepared.as_bytes(), space)?;
        Ok((
            Self {
                intent: intent.clone(),
                observed,
                created: &after - &before,
            },
            prepared,
        ))
    }

    pub(crate) fn prepare<'a>(
        &self,
        source: &'a [u8],
        space: ExGuid,
    ) -> Result<std::result::Result<PreparedEdit<'a>, ConflictKind>> {
        let Some(current) = observe(
            source,
            space,
            &self.intent,
            matches!(self.observed.content, Some(Content::Legacy(_))),
        )?
        else {
            return Ok(Err(ConflictKind::TargetUnavailable));
        };
        let prepared = match PreparedEdit::tree(source, space, &self.intent) {
            Ok(prepared) => prepared,
            Err(_) => return Ok(Err(ConflictKind::UnsupportedEdit)),
        };
        let after = mutable_objects(prepared.as_bytes(), space)?;
        if prepared.as_bytes() == source {
            return Ok(if self.created.is_subset(&after) {
                Ok(prepared)
            } else {
                Err(ConflictKind::StructureChanged)
            });
        }
        if current.path != self.observed.path
            || current.destination != self.observed.destination
            || current.siblings.iter().any(|(id, before)| {
                self.observed
                    .siblings
                    .get(id)
                    .is_some_and(|was_before| before != was_before)
            })
            || &after - &mutable_objects(source, space)? != self.created
        {
            return Ok(Err(ConflictKind::StructureChanged));
        }
        if current.content != self.observed.content {
            return Ok(Err(ConflictKind::ContentChanged));
        }
        Ok(Ok(prepared))
    }

    pub(crate) fn review(&self, source: &[u8], space: ExGuid) -> Result<Self> {
        let (reviewed, _) = Self::capture(source, space, &self.intent)?;
        if reviewed.created != self.created {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "The subtree edit would change its replacement paragraph identities",
            )
            .into());
        }
        Ok(reviewed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use onestore::Object;
    use std::sync::Arc;

    fn set(fields: &[(u32, Vec<u8>)]) -> Vec<u8> {
        let mut bytes = u16::try_from(fields.len()).unwrap().to_le_bytes().to_vec();
        for (id, _) in fields {
            bytes.extend(id.to_le_bytes());
        }
        for (_, value) in fields {
            bytes.extend(value);
        }
        bytes
    }

    #[test]
    fn deletion_fingerprint_resolves_references_and_nested_sets_without_storage_order() {
        let source = onestore::create_section("fingerprint.one", "Original", "Author").unwrap();
        let store = Store::parse(&source).unwrap();
        let root = ExGuid {
            guid: [1; 16],
            n: 1,
        };
        let child = ExGuid {
            guid: [2; 16],
            n: 7,
        };
        let hash = |reverse: bool, global: u32, timestamp: u32, change: usize, immutable: bool| {
            let mut fields = vec![
                (0x14001d7a, timestamp.to_le_bytes().to_vec()),
                (0x20000001, vec![]),
                (
                    0x44000002,
                    set(&[(0x14000003, (u32::from(change == 1)).to_le_bytes().to_vec())]),
                ),
                (
                    0x44000004,
                    set(&[(if change == 2 { 0x08000005 } else { 0x88000005 }, vec![])]),
                ),
            ];
            if change != 5 {
                fields.push((0x24001c20, 0_u32.to_le_bytes().to_vec()));
            }
            if change == 6 {
                fields.push((0x24000007, 0_u32.to_le_bytes().to_vec()));
            }
            if reverse {
                fields.reverse();
            }
            let mut bytes = 0x80000001_u32.to_le_bytes().to_vec();
            bytes.extend(((global << 8) | child.n).to_le_bytes());
            bytes.extend(set(&fields));
            let mut child_bytes = 0x80000000_u32.to_le_bytes().to_vec();
            child_bytes.extend(set(&[(
                0x14000006,
                (u32::from(change == 3)).to_le_bytes().to_vec(),
            )]));
            let ids = Arc::new(BTreeMap::from([(
                global,
                if change == 4 { [3; 16] } else { child.guid },
            )]));
            let target = ExGuid {
                guid: ids[&global],
                ..child
            };
            let raw = ResolvedRevision {
                roots: BTreeMap::from([(1, root)]),
                objects: BTreeMap::from([
                    (
                        root,
                        Object {
                            jcid: 0x6000d,
                            reference_count: 1,
                            data: ObjectData::Properties(&bytes),
                            global_ids: ids.clone(),
                        },
                    ),
                    (
                        target,
                        Object {
                            jcid: if immutable { 0x12004d } else { 0x6000e },
                            reference_count: 1,
                            data: ObjectData::Properties(&child_bytes),
                            global_ids: ids,
                        },
                    ),
                ]),
            };
            fingerprint(&store, &raw, root, false).unwrap()
        };
        let original = hash(false, 0, 1, 0, false);
        assert_eq!(original, hash(true, 137, 2, 0, false));
        assert_eq!(original, hash(true, 137, 2, 5, false));
        assert_ne!(original, hash(true, 137, 2, 6, false));
        for change in 1..=4 {
            assert_ne!(original, hash(true, 137, 2, change, false));
        }
        let immutable = hash(false, 0, 1, 0, true);
        assert_eq!(immutable, hash(true, 137, 2, 4, true));
        for change in 1..=3 {
            assert_ne!(immutable, hash(true, 137, 2, change, true));
        }
    }

    #[test]
    fn version_eight_deletion_observations_survive_migration_and_upgrade_only_after_review() {
        struct Server(Vec<u8>);
        impl Remote for Server {
            fn read(&mut self) -> io::Result<Vec<u8>> {
                Ok(self.0.clone())
            }
            fn publish(
                &mut self,
                edit: &PreparedEdit<'_>,
            ) -> std::result::Result<(), onestore::CommitError> {
                self.0 = edit.as_bytes().to_vec();
                Ok(())
            }
            fn confirm(
                &mut self,
                snapshot: &[u8],
            ) -> std::result::Result<(), onestore::CommitError> {
                assert!(self.0 == snapshot);
                Ok(())
            }
        }
        let source = onestore::create_section("legacy.one", "AlphaOmega", "Author").unwrap();
        let store = Store::parse(&source).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let document = Document::parse(&index).unwrap();
        let (sid, _) = document.pages().unwrap()[0];
        let space = &document.spaces[&sid];
        let view = &space.revisions[&space.contexts[&ExGuid::default()]];
        let text = *view.nodes.iter().find(|(_, node)| matches!(&node.kind, Kind::RichText { text, .. } if text == "AlphaOmega")).unwrap().0;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("legacy.sqlite");
        let cache = Replica::create(&path, &source).unwrap();
        cache
            .format(
                &source,
                sid,
                text,
                0..10,
                &[onestore::TextAttribute::Bold(true)],
            )
            .unwrap();
        let split = onestore::ParagraphSplit::new(text, 5, "Author").unwrap();
        cache
            .split(&cache.snapshot().unwrap(), sid, &split)
            .unwrap();
        let before = cache.snapshot().unwrap();
        let intent = onestore::TreeEdit::delete(split.object(), "Author").unwrap();
        let deletion = cache.tree(&before, sid, &intent).unwrap().unwrap();
        let dependent = cache
            .edit_text(&cache.snapshot().unwrap(), sid, text, 0..0, "Local ")
            .unwrap()
            .unwrap();
        let mut queue = cache.pending().unwrap();
        let Operation::Tree(edit) = &mut queue[2].operation else {
            panic!()
        };
        edit.observed = observe(&before, sid, &intent, true).unwrap().unwrap();
        let serialized = serde_json::to_string(&queue[2].operation).unwrap();
        let local = cache.snapshot().unwrap();
        drop(cache);
        let db = Connection::open(&path).unwrap();
        db.execute(
            "UPDATE edits SET operation=?1 WHERE id=?2",
            params![serialized, i64::try_from(deletion).unwrap()],
        )
        .unwrap();
        db.pragma_update(None, "user_version", 8).unwrap();
        drop(db);
        let cache = Replica::open(&path).unwrap();
        assert_eq!(cache.pending().unwrap(), queue);
        assert!(cache.snapshot().unwrap() == local);
        let mut server = Server(source);
        for _ in 0..2 {
            assert!(matches!(
                cache.sync_once(&mut server).unwrap(),
                Some((_, EditStatus::Published { .. }))
            ));
        }
        assert_eq!(
            cache.sync_once(&mut server).unwrap(),
            Some((deletion, EditStatus::Conflict(ConflictKind::ContentChanged)))
        );
        cache
            .rebase_tree_conflict(deletion, &local, &server.0)
            .unwrap();
        let queue = cache.pending().unwrap();
        let Operation::Tree(edit) = &queue[0].operation else {
            panic!()
        };
        assert!(matches!(
            edit.observed.content,
            Some(Content::Semantic { .. })
        ));
        for id in [deletion, dependent] {
            assert!(
                matches!(cache.sync_once(&mut server).unwrap(), Some((actual, EditStatus::Published { .. })) if actual == id)
            );
        }
        assert_eq!(
            paragraph(&server.0, sid, text).unwrap().as_deref(),
            Some("Local Alpha")
        );
    }
}
