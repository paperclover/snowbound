use onestore::{
    ExGuid, ObjectData, PropertySets, RevisionIndex, Store, Value, replace_property_bytes,
};
use std::fs;

#[test]
fn scalar_edit_appends_a_revision_and_preserves_every_prior_object() {
    let source =
        fs::read("../../corpus/native/20260905-05/snapshots/02-text/notebook/synthetic.one")
            .unwrap();
    let store = Store::parse(&source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let mut selected = None;
    for (osid, space) in &index.spaces {
        let revision = index
            .resolve(*osid, space.labels[&(ExGuid::default(), 1)])
            .unwrap();
        for oid in revision.reachable().unwrap() {
            let object = &revision.objects[&oid];
            if let ObjectData::Properties(bytes) = object.data {
                for property in &PropertySets::parse(bytes).unwrap().sets[0] {
                    if property.value == Value::Bytes(b"Fictitious plain text.") {
                        assert!(selected.replace((*osid, oid, property.id)).is_none());
                    }
                }
            }
        }
    }
    let (osid, oid, property) = selected.unwrap();
    assert_eq!(
        replace_property_bytes(&source, osid, oid, property, b"Fictitious plain text.").unwrap(),
        source
    );
    for length in 0..=40 {
        let value = vec![b'x'; length];
        let written = replace_property_bytes(&source, osid, oid, property, &value).unwrap();
        assert_preserved_committed_bytes(&source, &written);
        let parsed = Store::parse(&written).unwrap();
        assert!(parsed.checksum_mismatches.is_empty());
        assert_eq!(
            parsed.header.transaction_count,
            store.header.transaction_count + 1
        );
        assert_eq!(parsed.header.generation, store.header.generation + 1);
        assert_ne!(parsed.header.deny_read_id, store.header.deny_read_id);
        let after = RevisionIndex::parse(&parsed).unwrap();
        after.validate_current().unwrap();
        for (space_id, space) in &index.spaces {
            for rid in space.revisions.keys() {
                let before = index.resolve(*space_id, *rid).unwrap();
                let unchanged = after.resolve(*space_id, *rid).unwrap();
                assert_eq!(before.roots, unchanged.roots);
                assert_eq!(before.objects.len(), unchanged.objects.len());
                for (id, object) in before.objects {
                    let same = &unchanged.objects[&id];
                    assert_eq!(object.jcid, same.jcid);
                    assert_eq!(object.reference_count, same.reference_count);
                    assert_eq!(object.global_ids, same.global_ids);
                    assert_eq!(object.data, same.data);
                }
            }
        }
        let space = &after.spaces[&osid];
        let active = after
            .resolve(osid, space.labels[&(ExGuid::default(), 1)])
            .unwrap();
        let ObjectData::Properties(data) = active.objects[&oid].data else {
            panic!()
        };
        let properties = PropertySets::parse(data).unwrap();
        assert_eq!(
            properties.sets[0]
                .iter()
                .find(|candidate| candidate.id == property)
                .unwrap()
                .value,
            Value::Bytes(&value)
        );
    }
}

#[test]
fn toc_color_edit_uses_its_native_revision_encoding_and_crc() {
    let source = fs::read(
        "../../corpus/native/20260905-05/snapshots/02-text/notebook/Open Notebook.onetoc2",
    )
    .unwrap();
    let store = Store::parse(&source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let rid = index.spaces[&index.root].labels[&(ExGuid::default(), 1)];
    let revision = index.resolve(index.root, rid).unwrap();
    let oid = revision.roots[&1];
    let written =
        replace_property_bytes(&source, index.root, oid, 0x14001cbe, &[0x33, 0x66, 0x99, 0])
            .unwrap();
    let after = Store::parse(&written).unwrap();
    assert!(after.checksum_mismatches.is_empty());
    assert_preserved_committed_bytes(&source, &written);
    let current = RevisionIndex::parse(&after).unwrap();
    current.validate_current().unwrap();
    assert_eq!(
        current.resolve(index.root, rid).unwrap().objects[&oid].data,
        revision.objects[&oid].data
    );
    let active = current
        .resolve(
            index.root,
            current.spaces[&index.root].labels[&(ExGuid::default(), 1)],
        )
        .unwrap();
    let ObjectData::Properties(blob) = active.objects[&oid].data else {
        panic!()
    };
    assert!(
        PropertySets::parse(blob).unwrap().sets[0]
            .iter()
            .any(|p| p.id == 0x14001cbe && p.value == Value::Bytes(&[0x33, 0x66, 0x99, 0]))
    );
}

#[test]
fn fresh_section_has_unique_identities_and_exact_utf16_text() {
    let text = "Created in Rust: café 東京 🦀";
    let first = onestore::create_section("synthetic.one", text, "Fixture Author").unwrap();
    let second = onestore::create_section("synthetic.one", text, "Fixture Author").unwrap();
    let a = Store::parse(&first).unwrap();
    let b = Store::parse(&second).unwrap();
    assert_ne!(a.header.file_id, b.header.file_id);
    assert!(a.checksum_mismatches.is_empty());
    let index = RevisionIndex::parse(&a).unwrap();
    index.validate_current().unwrap();
    assert_eq!(index.spaces.len(), 2);
    let expected: Vec<_> = text
        .encode_utf16()
        .chain([0])
        .flat_map(u16::to_le_bytes)
        .collect();
    let mut found = 0;
    for (osid, space) in &index.spaces {
        let revision = index
            .resolve(*osid, space.labels[&(ExGuid::default(), 1)])
            .unwrap();
        for oid in revision.reachable().unwrap() {
            let object = &revision.objects[&oid];
            if object.jcid == 0x6000e {
                let ObjectData::Properties(data) = object.data else {
                    panic!()
                };
                assert!(
                    PropertySets::parse(data).unwrap().sets[0]
                        .iter()
                        .any(|p| p.id == 0x1c001c22 && p.value == Value::Bytes(&expected))
                );
                found += 1;
            }
        }
    }
    assert_eq!(found, 1);
    for text in ["line\nfeed", "embedded\0nul"] {
        assert!(onestore::create_section("a.one", text, "").is_err());
    }
    for name in ["a.onetoc2", "../a.one", "a\\b.one", "a\0.one"] {
        assert!(onestore::create_section(name, "text", "").is_err());
    }
}

#[test]
fn malformed_creation_sequences_are_rejected_before_object_resolution() {
    let source = onestore::create_section("a.one", "text", "").unwrap();
    let store = Store::parse(&source).unwrap();
    let node = store
        .lists
        .values()
        .flat_map(|list| &list.nodes)
        .find(|node| node.id == 0x84)
        .unwrap();
    let at = node.offset;
    let mut damaged = source.clone();
    let header = u32::from_le_bytes(damaged[at..at + 4].try_into().unwrap());
    damaged[at..at + 4].copy_from_slice(&((header & !0x3ff) | 0x5a).to_le_bytes());
    let store = Store::parse(&damaged).unwrap();
    assert_eq!(
        RevisionIndex::parse(&store).unwrap_err().message,
        "Object group lacks its dependency overrides"
    );
    let store = Store::parse(&source).unwrap();
    let root = store
        .lists
        .values()
        .find(|list| list.nodes.iter().any(|node| node.id == 4))
        .unwrap();
    let selector = root.nodes.iter().find(|node| node.id == 4).unwrap();
    let start = root.nodes[0].offset;
    let end = selector.offset + 24;
    let mut damaged = source.clone();
    damaged[start..end].rotate_right(24);
    let store = Store::parse(&damaged).unwrap();
    assert_eq!(
        RevisionIndex::parse(&store).unwrap_err().message,
        "Root object space is referenced before its declaration"
    );
}

#[test]
fn created_toc_preserves_section_order_names_and_identities() {
    let entries = [("second.one", [2; 16]), ("first.one", [1; 16])];
    let bytes = onestore::create_table_of_contents("Open Notebook.onetoc2", &entries).unwrap();
    let store = Store::parse(&bytes).unwrap();
    assert!(store.checksum_mismatches.is_empty());
    let index = RevisionIndex::parse(&store).unwrap();
    index.validate_current().unwrap();
    let revision = index
        .resolve(
            index.root,
            index.spaces[&index.root].labels[&(ExGuid::default(), 1)],
        )
        .unwrap();
    let root = &revision.objects[&revision.roots[&1]];
    for (oid, (name, identity)) in root.references().unwrap().objects.iter().zip(entries) {
        let ObjectData::Properties(data) = revision.objects[oid].data else {
            panic!()
        };
        let props = PropertySets::parse(data).unwrap();
        let expected: Vec<_> = name
            .encode_utf16()
            .chain([0])
            .flat_map(u16::to_le_bytes)
            .collect();
        assert!(
            props.sets[0]
                .iter()
                .any(|p| p.id == 0x1c001d94 && p.value == Value::Bytes(&identity))
        );
        assert!(
            props.sets[0]
                .iter()
                .any(|p| p.id == 0x1c001d6b && p.value == Value::Bytes(&expected))
        );
    }
    assert_eq!(root.references().unwrap().objects.len(), entries.len());
    assert!(
        onestore::create_table_of_contents(
            "Open Notebook.onetoc2",
            &[("same.one", [1; 16]), ("SAME.one", [2; 16])]
        )
        .is_err()
    );
    assert!(
        onestore::create_table_of_contents(
            "Open Notebook.onetoc2",
            &[("a.one", [1; 16]), ("b.one", [1; 16])]
        )
        .is_err()
    );
    let empty = onestore::create_table_of_contents("Open Notebook.onetoc2", &[]).unwrap();
    let store = Store::parse(&empty).unwrap();
    RevisionIndex::parse(&store)
        .unwrap()
        .validate_current()
        .unwrap();
}

fn assert_preserved_committed_bytes(source: &[u8], written: &[u8]) {
    let before = Store::parse(source).unwrap();
    let after = Store::parse(written).unwrap();
    for node in before.lists.values().flat_map(|list| &list.nodes) {
        let header = u32::from_le_bytes(source[node.offset..node.offset + 4].try_into().unwrap());
        let size = ((header >> 10) & 0x1fff) as usize;
        assert!(
            source[node.offset..node.offset + size] == written[node.offset..node.offset + size],
            "Committed node changed at {}",
            node.offset
        );
        if let Some(onestore::Reference::Data(chunk)) = node.reference
            && chunk.length > 0
        {
            assert_eq!(
                before.chunk_data(chunk).unwrap(),
                after.chunk_data(chunk).unwrap()
            );
        }
    }
    let mut uncommitted = written.to_vec();
    uncommitted[..1024].copy_from_slice(&source[..1024]);
    let old = Store::parse(&uncommitted).unwrap();
    assert!(old.checksum_mismatches.is_empty());
    RevisionIndex::parse(&old)
        .unwrap()
        .validate_current()
        .unwrap();
}

#[test]
fn toc_writes_cross_fragment_and_counter_boundaries_without_crc_drift() {
    let mut bytes =
        fs::read("../../corpus/m6/native-structure-01/notebook/Open Notebook.onetoc2").unwrap();
    for i in 0_u32..260 {
        let store = Store::parse(&bytes).unwrap();
        assert!(store.checksum_mismatches.is_empty(), "edit {i}");
        let index = RevisionIndex::parse(&store).unwrap();
        let root = index.root;
        let revision = index
            .resolve(root, index.spaces[&root].labels[&(ExGuid::default(), 1)])
            .unwrap();
        let oid = revision.roots[&1];
        let written =
            replace_property_bytes(&bytes, root, oid, 0x14001cbe, &i.to_le_bytes()).unwrap();
        assert_preserved_committed_bytes(&bytes, &written);
        bytes = written;
    }
    let store = Store::parse(&bytes).unwrap();
    assert!(store.checksum_mismatches.is_empty());
    assert!(store.header.transaction_count >= 262);
}

#[test]
fn checkpoints_bound_dependencies_and_preserve_native_objects_and_history() {
    for (path, section) in [
        (
            "../../corpus/native/20260905-05/snapshots/06-attachment/notebook/synthetic.one",
            true,
        ),
        (
            "../../corpus/m6/native-structure-01/notebook/Open Notebook.onetoc2",
            false,
        ),
    ] {
        let mut bytes = fs::read(path).unwrap();
        let initial = Store::parse(&bytes).unwrap();
        let index = RevisionIndex::parse(&initial).unwrap();
        let mut candidates = Vec::new();
        for (sid, space) in &index.spaces {
            let revision = index
                .resolve(*sid, space.labels[&(ExGuid::default(), 1)])
                .unwrap();
            for oid in revision.reachable().unwrap() {
                let object = &revision.objects[&oid];
                if object.jcid != if section { 0x6000e } else { 0x20001 } {
                    continue;
                }
                let ObjectData::Properties(blob) = object.data else {
                    panic!()
                };
                let property = if section { 0x14001d7a } else { 0x14001cbe };
                if PropertySets::parse(blob).unwrap().sets[0]
                    .iter()
                    .any(|p| p.id == property)
                {
                    candidates.push((*sid, oid, property));
                }
            }
        }
        assert!(!candidates.is_empty());
        let sid = candidates[0].0;
        candidates.retain(|candidate| candidate.0 == sid);
        if section {
            let revision = index
                .resolve(sid, index.spaces[&sid].labels[&(ExGuid::default(), 1)])
                .unwrap();
            assert!(
                revision
                    .objects
                    .values()
                    .any(|o| matches!(o.data, ObjectData::File { .. }))
            );
            assert!(revision.objects.values().any(|o| o.jcid & 0x100000 != 0));
        }
        let mut checkpoints = 0;
        for operation in 0_u32..1025 {
            let (_, oid, property) = candidates[operation as usize % candidates.len()];
            let store = Store::parse(&bytes).unwrap();
            let before = RevisionIndex::parse(&store).unwrap();
            let rid = before.spaces[&sid].labels[&(ExGuid::default(), 1)];
            let value = (1_000_000 + operation).to_le_bytes();
            let written = replace_property_bytes(&bytes, sid, oid, property, &value).unwrap();
            let updated = Store::parse(&written).unwrap();
            assert!(updated.checksum_mismatches.is_empty());
            assert_eq!(
                updated.header.transaction_count,
                store.header.transaction_count + 1
            );
            let after = RevisionIndex::parse(&updated).unwrap();
            after.validate_current().unwrap();
            let next = after.spaces[&sid].labels[&(ExGuid::default(), 1)];
            let depth =
                std::iter::successors(Some(next), |id| after.spaces[&sid].revisions[id].dependency)
                    .count();
            assert!(depth <= 512);
            if after.spaces[&sid].revisions[&next].dependency.is_none() {
                checkpoints += 1;
                assert_preserved_committed_bytes(&bytes, &written);
                let old = before.resolve(sid, rid).unwrap();
                let current = after.resolve(sid, next).unwrap();
                assert_eq!(old.roots, current.roots);
                assert_eq!(old.objects.len(), current.objects.len());
                for (id, object) in &old.objects {
                    let same = &current.objects[id];
                    assert_eq!(object.jcid, same.jcid);
                    assert_eq!(object.reference_count, same.reference_count);
                    let a = object.references().unwrap();
                    let b = same.references().unwrap();
                    assert_eq!(a.objects, b.objects);
                    assert_eq!(a.object_spaces, b.object_spaces);
                    assert_eq!(a.contexts, b.contexts);
                    if section {
                        assert_eq!(object.global_ids, same.global_ids);
                        if *id != oid {
                            assert_eq!(object.data, same.data);
                        }
                    }
                    if let (ObjectData::Properties(a), ObjectData::Properties(b)) =
                        (object.data, same.data)
                    {
                        let a = PropertySets::parse(a).unwrap();
                        let b = PropertySets::parse(b).unwrap();
                        assert_eq!(a.sets.len(), b.sets.len());
                        for (a, b) in a.sets.iter().zip(&b.sets) {
                            assert_eq!(a.len(), b.len());
                            for (a, b) in a.iter().zip(b) {
                                assert_eq!(a.id, b.id);
                                if *id == oid && a.id == property {
                                    assert_eq!(b.value, Value::Bytes(&value));
                                } else if !matches!(a.value, Value::References { .. }) {
                                    assert_eq!(a.value, b.value);
                                }
                            }
                        }
                    }
                }
                for (space_id, space) in &before.spaces {
                    for previous in space.revisions.keys() {
                        let old = before.resolve(*space_id, *previous).unwrap();
                        let preserved = after.resolve(*space_id, *previous).unwrap();
                        assert_eq!(old.roots, preserved.roots);
                        assert_eq!(old.objects.len(), preserved.objects.len());
                        for (id, object) in old.objects {
                            let same = &preserved.objects[&id];
                            assert_eq!(object.jcid, same.jcid);
                            assert_eq!(object.reference_count, same.reference_count);
                            assert_eq!(object.global_ids, same.global_ids);
                            assert_eq!(object.data, same.data);
                        }
                    }
                }
            }
            bytes = written;
        }
        assert_eq!(checkpoints, 2, "{path}");
    }
}
