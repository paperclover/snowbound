use onestore::{
    ExGuid, FileDataReference, ObjectData, PreparedEdit, PropertySets, RevisionIndex, Store,
    document::{Document, Kind},
};
use std::collections::BTreeSet;

const SOURCE: &[u8] =
    include_bytes!("../../../corpus/page-lifecycle/04-nested/notebook/Lifecycle.one");

fn verify(original: &[u8], written: &[u8], native: &[u8], removed: &[ExGuid]) {
    let stores = [original, written, native].map(|bytes| Store::parse(bytes).unwrap());
    let indexes = stores.each_ref().map(|store| {
        assert!(store.checksum_mismatches.is_empty());
        let index = RevisionIndex::parse(store).unwrap();
        index.validate_current().unwrap();
        index
    });
    let documents = indexes
        .each_ref()
        .map(|index| Document::parse(index).unwrap());
    assert_eq!(stores[0].header.file_id, stores[1].header.file_id);
    assert_eq!(
        stores[1].header.transaction_count,
        stores[0].header.transaction_count + 1
    );
    assert_eq!(
        indexes[0].spaces.keys().collect::<Vec<_>>(),
        indexes[1].spaces.keys().collect::<Vec<_>>()
    );
    let expected: Vec<_> = documents[0]
        .pages()
        .unwrap()
        .into_iter()
        .filter(|(sid, _)| !removed.contains(sid))
        .collect();
    assert_eq!(documents[1].pages().unwrap(), expected);
    assert_eq!(documents[2].pages().unwrap(), expected);
    assert_eq!(
        documents[1].spaces.keys().collect::<Vec<_>>(),
        documents[2].spaces.keys().collect::<Vec<_>>()
    );
    for (sid, space) in &documents[1].spaces {
        let expected = &documents[2].spaces[sid];
        assert_eq!(
            space.contexts.keys().collect::<Vec<_>>(),
            expected.contexts.keys().collect::<Vec<_>>()
        );
        for (context, rid) in &space.contexts {
            let actual = &space.revisions[rid];
            let expected = &expected.revisions[&expected.contexts[context]];
            assert_eq!(actual.roots, expected.roots);
            assert!(
                serde_json::to_value(&actual.nodes).unwrap()
                    == serde_json::to_value(&expected.nodes).unwrap(),
                "native page graph {sid}/{context}"
            );
        }
    }
    let mut payloads = BTreeSet::new();
    for (sid, space) in &indexes[0].spaces {
        for rid in space.revisions.keys() {
            let before = indexes[0].resolve(*sid, *rid).unwrap();
            let after = indexes[1].resolve(*sid, *rid).unwrap();
            assert_eq!(
                format!("{before:?}"),
                format!("{after:?}"),
                "prior revision {sid}/{rid}"
            );
            for object in before.objects.values() {
                if let Some(FileDataReference::Internal(guid)) = object.file_reference().unwrap() {
                    payloads.insert(guid);
                }
            }
        }
        if removed.contains(sid) {
            assert!(!documents[1].spaces.contains_key(sid));
            let before = indexes[0]
                .resolve(*sid, space.labels[&(ExGuid::default(), 1)])
                .unwrap();
            let after = indexes[1]
                .resolve(*sid, indexes[1].spaces[sid].labels[&(ExGuid::default(), 1)])
                .unwrap();
            assert_eq!(before.roots, after.roots);
            let ObjectData::Properties(bytes) = after.objects[&after.roots[&1]].data else {
                panic!()
            };
            assert!(PropertySets::parse(bytes).unwrap().sets[0].is_empty());
            let ObjectData::Properties(bytes) = after.objects[&after.roots[&2]].data else {
                panic!()
            };
            let metadata = PropertySets::parse(bytes).unwrap();
            assert!(metadata.sets[0].iter().any(|p| p.id == 0x88001de9));
            let native = indexes[2]
                .resolve(*sid, indexes[2].spaces[sid].labels[&(ExGuid::default(), 1)])
                .unwrap();
            assert_eq!(after.reachable().unwrap(), native.reachable().unwrap());
            for (label, rid) in &space.labels {
                if *label != (ExGuid::default(), 1) {
                    assert_eq!(indexes[1].spaces[sid].labels[label], *rid);
                }
            }
        }
    }
    for guid in payloads {
        assert_eq!(
            stores[0].file_data(guid).unwrap(),
            stores[1].file_data(guid).unwrap()
        );
    }
}

#[test]
fn removal_matches_all_native_cases_and_retains_every_old_revision() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus/page-lifecycle/removal");
    let cases: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("provenance.json")).unwrap()).unwrap();
    for (name, case) in cases.as_object().unwrap() {
        let original =
            std::fs::read(root.join(name).join("native/before/notebook/Lifecycle.one")).unwrap();
        let native =
            std::fs::read(root.join(name).join("native/after/notebook/Lifecycle.one")).unwrap();
        let store = Store::parse(&original).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let document = Document::parse(&index).unwrap();
        let pages = document.pages().unwrap();
        let selected: Vec<usize> =
            serde_json::from_value(case["request"]["selected"].clone()).unwrap();
        let removed: Vec<_> = selected
            .into_iter()
            .map(|ordinal| pages[ordinal].0)
            .collect();
        let prepared = PreparedEdit::delete_pages_permanently(&original, &removed).unwrap();
        if let Some(output) = std::env::var_os("ONESTORE_PAGE_REMOVAL_OUTPUT") {
            let output = std::path::Path::new(&output);
            assert!(output.is_absolute());
            std::fs::create_dir_all(output).unwrap();
            let case = output.join(name);
            std::fs::create_dir(&case).unwrap();
            std::fs::write(case.join("Lifecycle.one"), prepared.as_bytes()).unwrap();
        }
        verify(&original, prepared.as_bytes(), &native, &removed);
        assert!(PreparedEdit::delete_pages_permanently(prepared.as_bytes(), &removed).is_err());
    }
}

#[test]
fn removal_rejects_duplicate_missing_and_non_page_spaces_without_publication() {
    let store = Store::parse(SOURCE).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let pages = document.pages().unwrap();
    for selected in [
        vec![pages[0].0, pages[0].0],
        vec![index.root],
        vec![ExGuid::default()],
        vec![ExGuid {
            guid: [97; 16],
            n: 1,
        }],
        vec![pages[0].0, index.root],
    ] {
        assert!(PreparedEdit::delete_pages_permanently(SOURCE, &selected).is_err());
    }
    assert_eq!(
        PreparedEdit::delete_pages_permanently(SOURCE, &[])
            .unwrap()
            .as_bytes(),
        SOURCE
    );
    let written = PreparedEdit::delete_pages_permanently(SOURCE, &[pages[0].0]).unwrap();
    assert!(
        PreparedEdit::delete_pages_permanently(written.as_bytes(), &[pages[1].0, pages[0].0])
            .is_err()
    );
}

use page_schedule::{current, disk};

/// OneNote created a page in each section after Rust permanently removed pages from it;
/// the surviving pages keep their revisions through the native save and a cold reopen.
#[test]
fn native_pages_created_after_rust_removal_reopen_cold() {
    let root = std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../corpus/page-lifecycle/removal/rust-followup"
    ));
    for case in ["parent", "all", "features", "leading-parent"] {
        let read = |phase: &str| {
            std::fs::read(root.join(case).join(phase).join("notebook/Lifecycle.one")).unwrap()
        };
        let (candidate, followup, cold) =
            (read("candidate"), read("followup"), read("followup-cold"));
        let title =
            std::fs::read_to_string(root.join(case).join("followup/native-title.txt")).unwrap();
        let title = title.trim_start_matches('\u{feff}').trim_end();
        let stores = [&candidate, &followup, &cold].map(|bytes| Store::parse(bytes).unwrap());
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
        for (document, index) in documents[1..].iter().zip(&indexes[1..]) {
            let pages = document.pages().unwrap();
            assert_eq!(pages.len(), before.len() + 1, "{case}");
            assert_eq!(
                &pages[..before.len()],
                before.as_slice(),
                "{case}: surviving page order"
            );
            for (sid, _) in &before {
                assert_eq!(
                    index.active(*sid).unwrap(),
                    indexes[0].active(*sid).unwrap(),
                    "{case}: {sid}"
                );
            }
            let (native, page) = pages[before.len()];
            let revision = document.active(native).unwrap();
            let metadata = revision
                .roots
                .get(&2)
                .and_then(|id| revision.nodes.get(id))
                .unwrap();
            assert!(
                matches!(&metadata.kind, Kind::Metadata { title: Some(name), .. } if name == title),
                "{case}"
            );
            let body = revision
                .parents(&[page])
                .unwrap()
                .into_keys()
                .any(|id| matches!(&revision.nodes[&id].kind, Kind::RichText { text, .. } if text == "Native body after Rust removal."));
            assert!(body, "{case}: native body");
        }
        // The section root may gain OneNote's per-series navigation metadata copies on open.
        for sid in indexes[1]
            .spaces
            .keys()
            .filter(|sid| **sid != indexes[1].root)
        {
            assert_eq!(
                indexes[1].active(*sid).unwrap(),
                indexes[2].active(*sid).unwrap(),
                "{case}: cold reopen changed {sid}"
            );
        }
    }
}

#[test]
fn tombstones_and_first_page_promotion_survive_each_storage_interruption() {
    let leading = include_bytes!(
        "../../../corpus/page-lifecycle/removal/leading-parent/native/before/notebook/Lifecycle.one"
    );
    for (source, all) in [(leading.as_slice(), false), (SOURCE, true)] {
        let store = Store::parse(source).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let pages = Document::parse(&index).unwrap().pages().unwrap();
        let removed: Vec<_> = pages
            .iter()
            .take(if all { pages.len() } else { 1 })
            .map(|page| page.0)
            .collect();
        let prepared = PreparedEdit::delete_pages_permanently(source, &removed).unwrap();
        let old = current::current(source);
        let new = current::current(prepared.as_bytes());
        assert_ne!(old, new);
        assert_eq!(
            old.keys().collect::<Vec<_>>(),
            new.keys().collect::<Vec<_>>()
        );
        for write_limit in [1, 17, 4096] {
            let mut complete = disk::Disk {
                visible: source.to_vec(),
                durable: source.to_vec(),
                operation: 0,
                fail_at: None,
                write_limit,
                random: 1997,
            };
            prepared.commit(&mut complete).unwrap();
            assert_eq!(complete.durable, prepared.as_bytes());
            for fail_at in 1..=complete.operation {
                let mut interrupted = disk::Disk {
                    visible: source.to_vec(),
                    durable: source.to_vec(),
                    operation: 0,
                    fail_at: Some(fail_at),
                    write_limit,
                    random: 1997 + u64::try_from(fail_at).unwrap(),
                };
                let error = prepared.commit(&mut interrupted).unwrap_err();
                let observed = current::current(&interrupted.durable);
                assert!(
                    observed == old || observed == new,
                    "{all}:{write_limit}:{fail_at}"
                );
                match error.state {
                    onestore::CommitState::NotCommitted => assert_eq!(observed, old),
                    onestore::CommitState::Committed => assert_eq!(observed, new),
                    onestore::CommitState::Unknown => {}
                }
            }
        }
    }
}

#[test]
fn stale_removal_cannot_delete_a_newer_page_edit() {
    let store = Store::parse(SOURCE).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let sid = document.pages().unwrap()[3].0;
    let space = &document.spaces[&sid];
    let revision = &space.revisions[&space.contexts[&ExGuid::default()]];
    let text = revision
        .nodes
        .iter()
        .find_map(|(id, node)| match &node.kind {
            onestore::document::Kind::RichText {
                text,
                boilerplate: false,
                ..
            } if !text.is_empty() => Some(*id),
            _ => None,
        })
        .unwrap();
    let removal = PreparedEdit::delete_pages_permanently(SOURCE, &[sid]).unwrap();
    let remote = onestore::replace_text(SOURCE, sid, text, 0..0, "New remote content ").unwrap();
    let mut disk = disk::Disk {
        visible: remote.clone(),
        durable: remote.clone(),
        operation: 0,
        fail_at: None,
        write_limit: 17,
        random: 1999,
    };
    let error = removal.commit(&mut disk).unwrap_err();
    assert_eq!(error.state, onestore::CommitState::NotCommitted);
    assert_eq!(error.error.kind(), std::io::ErrorKind::ResourceBusy);
    assert_eq!(disk.visible, remote);
    assert_eq!(disk.durable, remote);
}

#[path = "support/page_schedule.rs"]
mod page_schedule;

#[test]
fn twelve_client_page_schedules_include_removal_empty_sections_and_stale_edits() {
    page_schedule::run(&[
        0, 192, 0, 128, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 2, 18, 0, 0, 0, 0, 0, 0, 3, 192, 0,
        128, 0, 0, 0, 0, 4, 0, 0, 0, 0, 0, 0, 0,
    ]);
    for seed in 1_u64..=32 {
        let mut random = seed * 2003;
        let mut input = [0; 192];
        for byte in &mut input {
            random ^= random << 13;
            random ^= random >> 7;
            random ^= random << 17;
            *byte = random.to_le_bytes()[0];
        }
        page_schedule::run(&input);
    }
}

#[test]
fn external_payloads_and_their_historical_references_survive_file_removal() {
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus/native-external-assets/notebook");
    let original = std::fs::read(fixture.join("synthetic.one")).unwrap();
    let store = Store::parse(&original).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let pages = Document::parse(&index).unwrap().pages().unwrap();
    assert_eq!(pages.len(), 3);
    let selected: Vec<_> = pages.iter().map(|page| page.0).collect();
    let prepared = PreparedEdit::delete_pages_permanently(&original, &selected).unwrap();
    let output = std::env::var_os("ONESTORE_PAGE_REMOVAL_EXTERNAL_OUTPUT");
    let root = output
        .as_ref()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::env::temp_dir().join(format!(
                "onestore-page-removal-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ))
        });
    assert!(root.is_absolute());
    std::fs::create_dir(&root).unwrap();
    let files = root.join("synthetic_onefiles");
    std::fs::create_dir(&files).unwrap();
    let mut payloads = std::collections::BTreeMap::new();
    for entry in std::fs::read_dir(fixture.join("synthetic_onefiles")).unwrap() {
        let entry = entry.unwrap();
        let bytes = std::fs::read(entry.path()).unwrap();
        std::fs::write(files.join(entry.file_name()), &bytes).unwrap();
        payloads.insert(entry.file_name(), bytes);
    }
    assert_eq!(payloads.len(), 3);
    let path = root.join("synthetic.one");
    std::fs::write(&path, &original).unwrap();
    prepared.commit_file(&path).unwrap();
    let written = onestore::read_file(&path).unwrap();
    assert_eq!(written, prepared.as_bytes());
    let current_store = Store::parse(&written).unwrap();
    let current_index = RevisionIndex::parse(&current_store).unwrap();
    current_index.validate_current().unwrap();
    assert!(
        Document::parse(&current_index)
            .unwrap()
            .pages()
            .unwrap()
            .is_empty()
    );
    let mut external = BTreeSet::new();
    for (sid, space) in &index.spaces {
        for rid in space.revisions.keys() {
            let before = index.resolve(*sid, *rid).unwrap();
            let after = current_index.resolve(*sid, *rid).unwrap();
            assert_eq!(format!("{before:?}"), format!("{after:?}"));
            for object in before.objects.values() {
                if let Some(FileDataReference::External(reference)) =
                    object.file_reference().unwrap()
                {
                    external.insert(reference);
                }
            }
        }
    }
    assert_eq!(external.len(), 3);
    for (name, bytes) in payloads {
        assert_eq!(std::fs::read(files.join(name)).unwrap(), bytes);
    }
    if output.is_none() {
        std::fs::remove_dir_all(root).unwrap();
    }
}
