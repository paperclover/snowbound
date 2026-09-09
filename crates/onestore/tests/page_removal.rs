use onestore::{
    ExGuid, FileDataReference, IdStream, ObjectData, PropertySets, RevisionIndex, Store, Value,
    document::Document,
};
use std::collections::{BTreeMap, BTreeSet};

fn compare_copy(left: &Store<'_>, right: &Store<'_>, source: ExGuid, copied: ExGuid) {
    let indexes = [left, right].map(|store| RevisionIndex::parse(store).unwrap());
    let mut contexts = vec![(source, ExGuid::default(), copied, ExGuid::default())];
    let mut mapped_contexts = BTreeMap::new();
    let mut inverse_contexts = BTreeMap::new();
    while let Some((ls, lc, rs, rc)) = contexts.pop() {
        if let Some(previous) = mapped_contexts.insert((ls, lc), (rs, rc)) {
            assert_eq!(previous, (rs, rc));
            continue;
        }
        assert_eq!(inverse_contexts.insert((rs, rc), (ls, lc)), None);
        let revisions =
            [(ls, lc, &indexes[0]), (rs, rc, &indexes[1])].map(|(sid, context, index)| {
                index
                    .resolve(sid, index.spaces[&sid].labels[&(context, 1)])
                    .unwrap()
            });
        assert_eq!(
            revisions[0].roots.keys().collect::<Vec<_>>(),
            revisions[1].roots.keys().collect::<Vec<_>>()
        );
        let manifest = &revisions[1].objects[&revisions[1].roots[&1]];
        let copied_time = if manifest.jcid == 0x60037 {
            let pages = manifest.references().unwrap().objects;
            assert_eq!(pages.len(), 1);
            let ObjectData::Properties(bytes) = revisions[1].objects[&pages[0]].data else {
                panic!()
            };
            let properties = PropertySets::parse(bytes).unwrap();
            let Value::Bytes(time) = properties.sets[0]
                .iter()
                .find(|p| p.id == 0x14001d7a)
                .unwrap()
                .value
            else {
                panic!()
            };
            Some(time)
        } else {
            None
        };
        let mut pending: Vec<_> = revisions[0]
            .roots
            .iter()
            .map(|(role, id)| (*id, revisions[1].roots[role]))
            .collect();
        let mut mapped = BTreeMap::new();
        let mut inverse = BTreeMap::new();
        while let Some((a, b)) = pending.pop() {
            if let Some(previous) = mapped.insert(a, b) {
                assert_eq!(previous, b);
                continue;
            }
            if let Some(previous) = inverse.insert(b, a) {
                let originals = [&revisions[0].objects[&previous], &revisions[0].objects[&a]];
                assert!(originals.iter().all(|object| object.jcid == 0x120001));
                assert_eq!(originals[0].data, originals[1].data);
                for object in originals {
                    let references = object.references().unwrap();
                    assert!(references.objects.is_empty());
                    assert!(references.object_spaces.is_empty());
                    assert!(references.contexts.is_empty());
                }
            }
            let objects = [&revisions[0].objects[&a], &revisions[1].objects[&b]];
            assert_eq!(objects[0].jcid, objects[1].jcid);
            match (objects[0].data, objects[1].data) {
                (ObjectData::Properties(a), ObjectData::Properties(b)) => {
                    let sets = [a, b].map(|bytes| PropertySets::parse(bytes).unwrap());
                    let mut nested = vec![(0, 0)];
                    while let Some((a, b)) = nested.pop() {
                        let fields = [(0, a), (1, b)].map(|(side, set)| {
                            let properties = &sets[side].sets[set];
                            let fields = properties
                                .iter()
                                .map(|p| (p.id, &p.value))
                                .collect::<BTreeMap<_, _>>();
                            assert_eq!(fields.len(), properties.len());
                            fields
                        });
                        assert_eq!(
                            fields[0].keys().collect::<Vec<_>>(),
                            fields[1].keys().collect::<Vec<_>>()
                        );
                        for (id, value) in &fields[0] {
                            let other = fields[1][id];
                            match (value, other) {
                                (
                                    Value::References {
                                        stream,
                                        compact_ids,
                                    },
                                    Value::References {
                                        stream: other_stream,
                                        compact_ids: other_ids,
                                    },
                                ) => {
                                    assert_eq!(stream, other_stream);
                                    assert_eq!(compact_ids.len(), other_ids.len());
                                    for (a, b) in
                                        compact_ids.chunks_exact(4).zip(other_ids.chunks_exact(4))
                                    {
                                        let ids = [a, b].map(|bytes| {
                                            u32::from_le_bytes(bytes.try_into().unwrap())
                                        });
                                        let [a, b] = [0, 1].map(|side| ExGuid {
                                            guid: objects[side].global_ids[&(ids[side] >> 8)],
                                            n: ids[side] & 255,
                                        });
                                        match stream {
                                            IdStream::Objects => pending.push((a, b)),
                                            IdStream::Contexts => contexts.push((ls, a, rs, b)),
                                            IdStream::ObjectSpaces => contexts.push((
                                                a,
                                                ExGuid::default(),
                                                b,
                                                ExGuid::default(),
                                            )),
                                        }
                                    }
                                }
                                (Value::Sets(a), Value::Sets(b)) => {
                                    assert_eq!(a.len(), b.len());
                                    nested.extend(a.clone().zip(b.clone()));
                                }
                                (Value::Bytes(_), Value::Bytes(level))
                                    if objects[0].jcid == 0x20030
                                        && *id == 0x14001dff
                                        && lc == ExGuid::default() =>
                                {
                                    assert_eq!(*level, 1_u32.to_le_bytes());
                                }
                                (Value::Bytes(_), Value::Bytes(time))
                                    if matches!(
                                        objects[0].jcid,
                                        0x6000b
                                            | 0x6000c
                                            | 0x6002c
                                            | 0x60022
                                            | 0x60012
                                            | 0x60023
                                            | 0x60024
                                    ) && *id == 0x14001d7a
                                        && lc == ExGuid::default() =>
                                {
                                    if value != &other {
                                        assert_eq!(Some(*time), copied_time);
                                    }
                                }
                                _ => assert_eq!(
                                    value, &other,
                                    "property {id:#x}, jcid {:#x}, context {lc}, source set {a}, copied set {b}",
                                    objects[0].jcid
                                ),
                            }
                        }
                    }
                }
                (ObjectData::File { extension: a, .. }, ObjectData::File { extension: b, .. }) => {
                    assert_eq!(a, b);
                    let references =
                        objects.map(|object| object.file_reference().unwrap().unwrap());
                    let [
                        FileDataReference::Internal(a),
                        FileDataReference::Internal(b),
                    ] = references
                    else {
                        panic!("The copy fixture requires its external payloads");
                    };
                    assert_eq!(left.file_data(a).unwrap(), right.file_data(b).unwrap());
                }
                _ => panic!("Copied object representations differ"),
            }
        }
    }
}

#[test]
fn native_parent_removal_retains_history_and_copies_the_complete_page() {
    let bytes = [
        include_bytes!("../../../corpus/page-lifecycle/05-reordered/notebook/Lifecycle.one").as_slice(),
        include_bytes!("../../../corpus/page-lifecycle/06-deleted-parent/notebook/Lifecycle.one").as_slice(),
        include_bytes!("../../../corpus/page-lifecycle/06-deleted-parent/notebook/OneNote_RecycleBin/OneNote_DeletedPages.one").as_slice(),
    ];
    let stores = bytes.map(|b| Store::parse(b).unwrap());
    let indexes = stores.each_ref().map(|s| RevisionIndex::parse(s).unwrap());
    let documents = indexes.each_ref().map(|i| Document::parse(i).unwrap());
    let sid = documents[0].pages().unwrap()[4].0;
    assert!(!documents[1].spaces.contains_key(&sid));
    assert_eq!(
        indexes[0].spaces.keys().collect::<Vec<_>>(),
        indexes[1].spaces.keys().collect::<Vec<_>>()
    );
    for (space, revisions) in &indexes[0].spaces {
        for rid in revisions.revisions.keys() {
            assert_eq!(
                format!("{:?}", indexes[0].resolve(*space, *rid).unwrap()),
                format!("{:?}", indexes[1].resolve(*space, *rid).unwrap())
            );
        }
    }
    let current = indexes[1]
        .resolve(sid, indexes[1].spaces[&sid].labels[&(ExGuid::default(), 1)])
        .unwrap();
    let ObjectData::Properties(manifest) = current.objects[&current.roots[&1]].data else {
        panic!()
    };
    assert!(PropertySets::parse(manifest).unwrap().sets[0].is_empty());
    let ObjectData::Properties(metadata) = current.objects[&current.roots[&2]].data else {
        panic!()
    };
    assert!(
        PropertySets::parse(metadata).unwrap().sets[0]
            .iter()
            .any(|p| p.id == 0x88001de9)
    );
    let copied = documents[2].pages().unwrap();
    assert_eq!(copied.len(), 1);
    compare_copy(&stores[0], &stores[2], sid, copied[0].0);
}

fn compare_labels(before: &RevisionIndex<'_>, after: &RevisionIndex<'_>) {
    assert_eq!(
        before.spaces.keys().collect::<Vec<_>>(),
        after.spaces.keys().collect::<Vec<_>>()
    );
    for (sid, space) in &before.spaces {
        assert_eq!(
            space.labels.keys().collect::<Vec<_>>(),
            after.spaces[sid].labels.keys().collect::<Vec<_>>()
        );
        for (label, rid) in &space.labels {
            assert_eq!(
                format!("{:?}", before.resolve(*sid, *rid).unwrap()),
                format!(
                    "{:?}",
                    after
                        .resolve(*sid, after.spaces[sid].labels[label])
                        .unwrap()
                ),
                "cold context {sid}/{label:?}"
            );
        }
    }
}

fn native_case(name: &str) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus/page-lifecycle/removal")
        .join(name);
    let request: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("removal.json")).unwrap()).unwrap();
    let selected: BTreeSet<usize> = serde_json::from_value(request["selected"].clone()).unwrap();
    let bytes = ["native/before", "native/after", "cold"]
        .map(|phase| std::fs::read(root.join(phase).join("notebook/Lifecycle.one")).unwrap());
    let stores = bytes.each_ref().map(|bytes| Store::parse(bytes).unwrap());
    let indexes = stores.each_ref().map(|store| {
        assert!(store.checksum_mismatches.is_empty());
        let index = RevisionIndex::parse(store).unwrap();
        index.validate_current().unwrap();
        index
    });
    let documents = indexes
        .each_ref()
        .map(|index| Document::parse(index).unwrap());
    let before = documents[0].pages().unwrap();
    let retained: Vec<_> = before
        .iter()
        .enumerate()
        .filter(|(index, _)| !selected.contains(index))
        .map(|(_, page)| *page)
        .collect();
    assert_eq!(documents[1].pages().unwrap(), retained);
    assert_eq!(documents[2].pages().unwrap(), retained);
    let groups = documents.each_ref().map(|document| {
        let space = &document.spaces[&document.root];
        let revision = &space.revisions[&space.contexts[&ExGuid::default()]];
        revision.nodes[&revision.roots[&1]]
            .children
            .iter()
            .map(|id| (*id, revision.nodes[id].spaces.clone()))
            .collect::<Vec<_>>()
    });
    let mut expected: Vec<(ExGuid, Vec<ExGuid>)> = Vec::new();
    for (sid, _) in &retained {
        let space = &documents[0].spaces[sid];
        let revision = &space.revisions[&space.contexts[&ExGuid::default()]];
        let onestore::document::Kind::Metadata { level, .. } =
            revision.nodes[&revision.roots[&2]].kind
        else {
            panic!()
        };
        if expected.is_empty() || level == Some(1) {
            let (series, _) = groups[0]
                .iter()
                .find(|(_, pages)| pages.contains(sid))
                .unwrap();
            expected.push((*series, Vec::new()));
        }
        expected.last_mut().unwrap().1.push(*sid);
    }
    assert_eq!(groups[1], expected);
    assert_eq!(groups[2], expected);
    for (phase, index) in indexes[1..].iter().enumerate() {
        assert_eq!(
            indexes[0].spaces.keys().collect::<Vec<_>>(),
            index.spaces.keys().collect::<Vec<_>>()
        );
        let mut discarded = 0;
        for (sid, space) in &indexes[0].spaces {
            for rid in space.revisions.keys() {
                if !index.spaces[sid].revisions.contains_key(rid) {
                    discarded += 1;
                    continue;
                }
                assert_eq!(
                    format!("{:?}", indexes[0].resolve(*sid, *rid).unwrap()),
                    format!("{:?}", index.resolve(*sid, *rid).unwrap()),
                    "historical revision {sid}/{rid}"
                );
            }
            for ((context, role), rid) in &space.labels {
                if *context == ExGuid::default()
                    || selected.iter().any(|ordinal| before[*ordinal].0 == *sid)
                {
                    continue;
                }
                let retained = index.spaces[sid].labels[&(*context, *role)];
                assert_eq!(
                    format!("{:?}", indexes[0].resolve(*sid, *rid).unwrap()),
                    format!("{:?}", index.resolve(*sid, retained).unwrap()),
                    "labeled history {sid}/{context}/{role}"
                );
            }
        }
        assert_eq!(
            discarded,
            match name {
                "ink" => 9,
                "features" => 8,
                _ => 0,
            },
            "native compaction phase {phase}"
        );
    }
    compare_labels(&indexes[1], &indexes[2]);
    let active = |side: usize, sid| {
        indexes[side]
            .resolve(
                sid,
                indexes[side].spaces[&sid].labels[&(ExGuid::default(), 1)],
            )
            .unwrap()
    };
    for (ordinal, (sid, _)) in before.iter().enumerate() {
        let old = active(0, *sid);
        let after = active(1, *sid);
        assert_eq!(old.roots, after.roots);
        if selected.contains(&ordinal) {
            let ObjectData::Properties(manifest) = after.objects[&after.roots[&1]].data else {
                panic!()
            };
            assert!(PropertySets::parse(manifest).unwrap().sets[0].is_empty());
            let ObjectData::Properties(metadata) = after.objects[&after.roots[&2]].data else {
                panic!()
            };
            assert!(
                PropertySets::parse(metadata).unwrap().sets[0]
                    .iter()
                    .any(|p| p.id == 0x88001de9)
            );
        } else {
            assert_eq!(
                old.objects.keys().collect::<Vec<_>>(),
                after.objects.keys().collect::<Vec<_>>()
            );
            for (id, object) in &old.objects {
                if *id != old.roots[&2] {
                    assert_eq!(
                        format!("{object:?}"),
                        format!("{:?}", after.objects[id]),
                        "survivor {sid}/{id}"
                    );
                } else {
                    let fields = [object, &after.objects[id]].map(|object| {
                        let ObjectData::Properties(bytes) = object.data else {
                            panic!()
                        };
                        PropertySets::parse(bytes).unwrap().sets[0]
                            .iter()
                            .filter(|p| p.id != 0x14001dff)
                            .map(|p| (p.id, format!("{:?}", p.value)))
                            .collect::<BTreeMap<_, _>>()
                    });
                    assert_eq!(fields[0], fields[1]);
                    use onestore::document::Kind;
                    let levels = [0, 1].map(|side| {
                        let space = &documents[side].spaces[sid];
                        let view = &space.revisions[&space.contexts[&ExGuid::default()]];
                        let Kind::Metadata { level, .. } = view.nodes[id].kind else {
                            panic!()
                        };
                        level
                    });
                    assert_eq!(
                        levels[1],
                        if retained.first().map(|p| p.0) == Some(*sid) {
                            Some(1)
                        } else {
                            levels[0]
                        }
                    );
                }
            }
        }
    }
    if request["permanent"].as_bool().unwrap() {
        assert!(
            !root
                .join("native/after/notebook/OneNote_RecycleBin")
                .exists()
        );
        return;
    }
    let recycle_bytes = std::fs::read(
        root.join("native/after/notebook/OneNote_RecycleBin/OneNote_DeletedPages.one"),
    )
    .unwrap();
    let recycle = Store::parse(&recycle_bytes).unwrap();
    let recycled_index = RevisionIndex::parse(&recycle).unwrap();
    recycled_index.validate_current().unwrap();
    let recycled_document = Document::parse(&recycled_index).unwrap();
    let cold_recycle_bytes =
        std::fs::read(root.join("cold/notebook/OneNote_RecycleBin/OneNote_DeletedPages.one"))
            .unwrap();
    let cold_recycle = Store::parse(&cold_recycle_bytes).unwrap();
    let cold_index = RevisionIndex::parse(&cold_recycle).unwrap();
    cold_index.validate_current().unwrap();
    compare_labels(&recycled_index, &cold_index);
    let pages = recycled_document.pages().unwrap();
    assert_eq!(pages.len(), selected.len());
    let identity = |index: &RevisionIndex<'_>, sid| {
        let revision = index
            .resolve(sid, index.spaces[&sid].labels[&(ExGuid::default(), 1)])
            .unwrap();
        let ObjectData::Properties(bytes) = revision.objects[&revision.roots[&2]].data else {
            panic!()
        };
        let sets = PropertySets::parse(bytes).unwrap();
        let Value::Bytes(guid) = sets.sets[0]
            .iter()
            .find(|p| p.id == 0x1c001c30)
            .unwrap()
            .value
        else {
            panic!()
        };
        guid.to_vec()
    };
    let mut copies = BTreeMap::new();
    for (sid, _) in pages {
        assert!(copies.insert(identity(&recycled_index, sid), sid).is_none());
    }
    for ordinal in selected {
        let sid = before[ordinal].0;
        let copied = copies.remove(&identity(&indexes[0], sid)).unwrap();
        compare_copy(&stores[0], &recycle, sid, copied);
        compare_copy(&stores[0], &cold_recycle, sid, copied);
    }
    assert!(copies.is_empty());
}

macro_rules! native_cases {
    ($($name:ident => $case:literal),* $(,)?) => {$(
        #[test]
        fn $name() { native_case($case); }
    )*};
}

native_cases! {
    trailing => "trailing", parent => "parent", child => "child", grandchild => "grandchild",
    group => "group", all => "all", permanent_parent => "permanent-parent",
    leading_parent => "leading-parent", leading_group => "leading-group",
    features => "features", ink => "ink",
}

#[test]
fn copy_comparison_rejects_text_and_payload_changes() {
    let original = include_bytes!(
        "../../../corpus/page-lifecycle/removal/ink/native/before/notebook/Lifecycle.one"
    );
    let copied = include_bytes!(
        "../../../corpus/page-lifecycle/removal/ink/native/after/notebook/OneNote_RecycleBin/OneNote_DeletedPages.one"
    );
    let stores = [original.as_slice(), copied.as_slice()].map(|bytes| Store::parse(bytes).unwrap());
    let indexes = stores
        .each_ref()
        .map(|store| RevisionIndex::parse(store).unwrap());
    let documents = indexes
        .each_ref()
        .map(|index| Document::parse(index).unwrap());
    let [source_sid, copy_sid] = documents
        .each_ref()
        .map(|document| document.pages().unwrap()[0].0);
    compare_copy(&stores[0], &stores[1], source_sid, copy_sid);
    let space = &documents[1].spaces[&copy_sid];
    let revision = &space.revisions[&space.contexts[&ExGuid::default()]];
    let text = revision
        .nodes
        .iter()
        .find_map(|(oid, node)| {
            matches!(
                node.kind,
                onestore::document::Kind::RichText {
                    boilerplate: false,
                    ..
                }
            )
            .then_some(*oid)
        })
        .unwrap();
    let changed = onestore::replace_text(copied, copy_sid, text, 0..0, "Changed ").unwrap();
    let changed_store = Store::parse(&changed).unwrap();
    RevisionIndex::parse(&changed_store)
        .unwrap()
        .validate_current()
        .unwrap();
    assert!(
        std::panic::catch_unwind(|| compare_copy(&stores[0], &changed_store, source_sid, copy_sid))
            .is_err()
    );

    let raw = indexes[1]
        .resolve(
            copy_sid,
            indexes[1].spaces[&copy_sid].labels[&(ExGuid::default(), 1)],
        )
        .unwrap();
    let attachment = revision
        .nodes
        .values()
        .find_map(|node| match node.kind {
            onestore::document::Kind::Attachment { container, .. } => container,
            _ => None,
        })
        .unwrap();
    let Some(FileDataReference::Internal(guid)) =
        raw.objects[&attachment].file_reference().unwrap()
    else {
        panic!()
    };
    let payload = stores[1].file_data(guid).unwrap();
    assert!(!payload.is_empty());
    let offset = payload.as_ptr().addr() - copied.as_ptr().addr();
    let mut changed = copied.to_vec();
    changed[offset] ^= 1;
    let changed_store = Store::parse(&changed).unwrap();
    assert!(changed_store.checksum_mismatches.is_empty());
    RevisionIndex::parse(&changed_store)
        .unwrap()
        .validate_current()
        .unwrap();
    assert_ne!(changed_store.file_data(guid).unwrap(), payload);
    assert!(
        std::panic::catch_unwind(|| compare_copy(&stores[0], &changed_store, source_sid, copy_sid))
            .is_err()
    );
}
