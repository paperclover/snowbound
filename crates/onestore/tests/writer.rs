#[path = "support/ops.rs"]
mod ops;

use onestore::{
    ExGuid, ObjectData, PropertySets, RevisionIndex, Store, TocEdit, Value,
    document::{Document, Kind},
    op::{Edit, Op, PageOp},
};
use std::fs;

/// The text object holding `text` in `source` and its page space.
fn text_object(source: &[u8], text: &str) -> (ExGuid, ExGuid) {
    let store = Store::parse(source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let mut found = document.spaces.iter().flat_map(|(space, view)| {
        view.active()
            .unwrap()
            .nodes
            .iter()
            .filter(|(_, node)| matches!(&node.kind, Kind::RichText { text: stored, .. } if stored == text))
            .map(|(id, _)| (*space, *id))
            .collect::<Vec<_>>()
    });
    let target = found.next().unwrap();
    assert!(found.next().is_none());
    target
}

/// `source`, a table of contents, with the notebook recoloured.
fn recolored(source: &[u8], color: u32) -> Vec<u8> {
    let mut image = source.to_vec();
    let edit = TocEdit::Color(color);
    if let Some(transaction) = onestore::edit_table_of_contents(source, &[edit]).unwrap() {
        transaction.apply(&mut image).unwrap();
    }
    image
}

#[test]
fn text_edit_appends_a_revision_and_preserves_every_prior_object() {
    let source =
        fs::read("../../corpus/native/20260905-05/snapshots/02-text/notebook/synthetic.one")
            .unwrap();
    let store = Store::parse(&source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let (osid, oid) = text_object(&source, "Fictitious plain text.");
    let whole = 0.."Fictitious plain text.".encode_utf16().count() as u32;
    let typed = |with: &str| {
        let op = PageOp::Text {
            text: oid,
            range: whole.clone(),
            with: with.into(),
        };
        ops::page_edited(&source, osid, vec![op]).unwrap()
    };
    assert_eq!(typed("Fictitious plain text."), source);
    for length in 0..=40 {
        let value = "x".repeat(length);
        let written = typed(&value);
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
                    assert!(
                        same.global_ids
                            .iter()
                            .all(|(entry, guid)| object.global_ids.get(entry) == Some(guid))
                    );
                    assert_eq!(object.data, same.data);
                }
            }
        }
        let document = Document::parse(&after).unwrap();
        let Kind::RichText { text, .. } = &document.active(osid).unwrap().nodes[&oid].kind else {
            panic!()
        };
        assert_eq!(*text, value);
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
    let written = recolored(&source, 0x996633);
    let after = Store::parse(&written).unwrap();
    assert!(after.checksum_mismatches.is_empty());
    assert_preserved_committed_bytes(&source, &written);
    let current = RevisionIndex::parse(&after).unwrap();
    current.validate_current().unwrap();
    let active = current
        .resolve(
            index.root,
            current.spaces[&index.root].labels[&(ExGuid::default(), 1)],
        )
        .unwrap();
    let recoloured: Vec<_> = active
        .objects
        .iter()
        .filter(|(id, object)| {
            revision
                .objects
                .get(id)
                .is_none_or(|old| old.data != object.data)
        })
        .map(|(_, object)| object)
        .collect();
    let [entry] = recoloured[..] else {
        panic!("{} objects changed", recoloured.len())
    };
    let ObjectData::Properties(blob) = entry.data else {
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
        index.validate_current().unwrap();
        let written = recolored(&bytes, i);
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
        // A section retypes text objects of one page; a table of contents recolours the notebook.
        let document = Document::parse(&index).unwrap();
        let mut candidates = Vec::new();
        for (sid, view) in &document.spaces {
            for (oid, node) in &view.active().unwrap().nodes {
                match &node.kind {
                    Kind::RichText {
                        text,
                        boilerplate: false,
                        ..
                    } if section && !text.is_empty() => {
                        candidates.push((*sid, *oid));
                    }
                    Kind::Toc { entries, .. } if !section && !entries.is_empty() => {
                        candidates.push((*sid, *oid));
                    }
                    _ => {}
                }
            }
        }
        assert!(!candidates.is_empty());
        let sid = candidates[0].0;
        candidates.retain(|candidate| candidate.0 == sid);
        // What each edit rewrites: modification times, or the notebook's colour.
        let property = if section { 0x14001d7a } else { 0x14001cbe };
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
            let (_, oid) = candidates[operation as usize % candidates.len()];
            let store = Store::parse(&bytes).unwrap();
            let before = RevisionIndex::parse(&store).unwrap();
            let rid = before.spaces[&sid].labels[&(ExGuid::default(), 1)];
            let written = if section {
                let typed = |range, with: &str| Op::Page {
                    space: sid,
                    op: PageOp::Text {
                        text: oid,
                        range,
                        with: with.into(),
                    },
                };
                // Modification times count seconds: each edit is a second later.
                let at = 134_000_000_000_000_000 + u64::from(operation) * 10_000_000;
                let ops = vec![typed(0..0, "x"), typed(0..1, "")];
                let arena = onestore::Arena::default();
                let mut section = onestore::Section::open(&arena, bytes.clone()).unwrap();
                section.apply("Author", &Edit { at, ops }).unwrap();
                section.seal().unwrap().unwrap();
                section.image()
            } else {
                recolored(&bytes, 1_000_000 + operation)
            };
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
                        assert!(
                            same.global_ids
                                .iter()
                                .all(|(entry, guid)| object.global_ids.get(entry) == Some(guid))
                        );
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
                                if a.id != property && !matches!(a.value, Value::References { .. })
                                {
                                    assert_eq!(a.value, b.value, "{id} {:#x}", a.id);
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
                            assert!(
                                same.global_ids
                                    .iter()
                                    .all(|(entry, guid)| object.global_ids.get(entry) == Some(guid))
                            );
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
