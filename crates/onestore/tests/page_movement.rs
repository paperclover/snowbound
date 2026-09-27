#[path = "support/ops.rs"]
mod ops;

use onestore::{
    ExGuid, PageEdit, RevisionIndex, Store,
    document::{Document, Kind},
    op::{PageOp, SectionOp},
};

#[path = "support/current.rs"]
mod current;
#[path = "support/disk.rs"]
mod disk;
#[path = "support/sweep.rs"]
mod sweep;

const SOURCE: &[u8] =
    include_bytes!("../../../corpus/page-lifecycle/04-nested/notebook/Lifecycle.one");

fn pages(source: &[u8]) -> Vec<ExGuid> {
    let store = Store::parse(source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    Document::parse(&index)
        .unwrap()
        .pages()
        .unwrap()
        .into_iter()
        .map(|(sid, _)| sid)
        .collect()
}

fn export(name: &str, bytes: &[u8]) {
    if let Some(output) = std::env::var_os("ONESTORE_PAGE_MOVEMENT_OUTPUT") {
        assert!(
            std::path::Path::new(&output).is_absolute(),
            "Use an absolute fixture output directory"
        );
        let path = std::path::Path::new(&output).join(name);
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("Lifecycle.one"), bytes).unwrap();
    }
}

fn verify(before: &[u8], after: &[u8], native: &[u8]) {
    let stores = [before, after, native].map(|bytes| Store::parse(bytes).unwrap());
    let indexes = stores
        .each_ref()
        .map(|store| RevisionIndex::parse(store).unwrap());
    indexes[1].validate_current().unwrap();
    let documents = indexes
        .each_ref()
        .map(|index| Document::parse(index).unwrap());
    let expected = documents[2].pages().unwrap();
    assert_eq!(documents[1].pages().unwrap(), expected);
    for (sid, _) in &expected {
        let levels = [&documents[1], &documents[2]].map(|document| {
            let space = &document.spaces[sid];
            let view = &space.revisions[&space.contexts[&ExGuid::default()]];
            let Kind::Metadata { level, .. } = view.nodes[&view.roots[&2]].kind else {
                panic!()
            };
            level
        });
        assert_eq!(levels[0], levels[1]);
    }
    let groups = [&documents[1], &documents[2]].map(|document| {
        let space = &document.spaces[&document.root];
        let view = &space.revisions[&space.contexts[&ExGuid::default()]];
        view.nodes[&view.roots[&1]]
            .children
            .iter()
            .map(|id| view.nodes[id].spaces.clone())
            .collect::<Vec<_>>()
    });
    assert_eq!(groups[0], groups[1]);
    assert_eq!(
        stores[1].header.transaction_count,
        stores[0].header.transaction_count + 1
    );
    for (sid, space) in &indexes[0].spaces {
        for rid in space.revisions.keys() {
            assert_eq!(
                format!("{:?}", indexes[0].resolve(*sid, *rid).unwrap()),
                format!("{:?}", indexes[1].resolve(*sid, *rid).unwrap())
            );
        }
        if *sid != indexes[0].root {
            let old = indexes[0]
                .resolve(*sid, space.labels[&(ExGuid::default(), 1)])
                .unwrap();
            let new = indexes[1]
                .resolve(*sid, indexes[1].spaces[sid].labels[&(ExGuid::default(), 1)])
                .unwrap();
            assert_eq!(old.roots, new.roots);
            assert_eq!(
                old.objects.keys().collect::<Vec<_>>(),
                new.objects.keys().collect::<Vec<_>>()
            );
            for (id, object) in &old.objects {
                if *id != old.roots[&2] {
                    assert_eq!(format!("{object:?}"), format!("{:?}", new.objects[id]));
                }
            }
        }
    }
    let mut previous = after.to_vec();
    previous[96..100].copy_from_slice(&before[96..100]);
    assert_eq!(current::current(&previous), current::current(before));
}

#[test]
fn explicit_page_edits_match_native_selection_and_indentation() {
    let ids = pages(SOURCE);
    let level = |i, n| PageEdit::set_level(ids[i], n).unwrap();
    let move_to =
        |i, before: Option<usize>, n| PageEdit::move_to(ids[i], before.map(|i| ids[i]), n).unwrap();
    let cases: [(&str, Vec<PageEdit>, &[u8]); 8] = [
        (
            "01-demoted-parent",
            vec![level(3, 2)],
            include_bytes!(
                "../../../corpus/page-lifecycle/movement/01-demoted-parent/notebook/Lifecycle.one"
            ),
        ),
        (
            "02-promoted-parent",
            vec![level(3, 1)],
            include_bytes!(
                "../../../corpus/page-lifecycle/movement/02-promoted-parent/notebook/Lifecycle.one"
            ),
        ),
        (
            "03-selected-group-move",
            vec![
                move_to(3, None, 1),
                move_to(4, None, 2),
                move_to(5, None, 3),
            ],
            include_bytes!(
                "../../../corpus/page-lifecycle/movement/03-selected-group-move/notebook/Lifecycle.one"
            ),
        ),
        (
            "04-subpage-move",
            vec![move_to(4, Some(6), 1)],
            include_bytes!(
                "../../../corpus/page-lifecycle/movement/04-subpage-move/notebook/Lifecycle.one"
            ),
        ),
        (
            "05-insert-before-subpage",
            vec![move_to(4, Some(5), 3)],
            include_bytes!(
                "../../../corpus/page-lifecycle/movement/05-insert-before-subpage/notebook/Lifecycle.one"
            ),
        ),
        (
            "06-promoted-level-two",
            vec![level(4, 2)],
            include_bytes!(
                "../../../corpus/page-lifecycle/movement/06-promoted-level-two/notebook/Lifecycle.one"
            ),
        ),
        (
            "07-promoted-root",
            vec![level(4, 1)],
            include_bytes!(
                "../../../corpus/page-lifecycle/movement/07-promoted-root/notebook/Lifecycle.one"
            ),
        ),
        (
            "08-collapsed-group-move",
            vec![move_to(4, Some(0), 1), move_to(5, Some(0), 3)],
            include_bytes!(
                "../../../corpus/page-lifecycle/movement/08-collapsed-group-move/notebook/Lifecycle.one"
            ),
        ),
    ];
    let mut source = SOURCE.to_vec();
    for (name, edits, native) in cases {
        let serialized = serde_json::to_vec(&edits).unwrap();
        let retained: Vec<PageEdit> = serde_json::from_slice(&serialized).unwrap();
        assert_eq!(edits, retained);
        let prepared = ops::section_op(&source, SectionOp::Pages(retained.to_vec())).unwrap();
        verify(&source, prepared.as_bytes(), native);
        assert_eq!(
            ops::section_op(prepared.as_bytes(), SectionOp::Pages(retained.to_vec()))
                .unwrap()
                .as_bytes(),
            prepared.as_bytes()
        );
        export(name, prepared.as_bytes());
        source = prepared.as_bytes().to_vec();
    }
    let edit = PageEdit::move_to(ids[3], None, 1).unwrap();
    let single = ops::section_op(SOURCE, SectionOp::Pages([edit].to_vec())).unwrap();
    verify(
        SOURCE,
        single.as_bytes(),
        include_bytes!(
            "../../../corpus/page-lifecycle/movement/single-page/notebook/Lifecycle.one"
        ),
    );
    export("single-page", single.as_bytes());
}

#[test]
fn invalid_and_empty_page_edits_do_not_publish() {
    let ids = pages(SOURCE);
    for level in [0, 4, u32::MAX] {
        assert!(PageEdit::set_level(ids[0], level).is_err());
    }
    assert!(PageEdit::move_to(ids[0], Some(ids[0]), 1).is_err());
    assert!(PageEdit::set_level(ExGuid::default(), 1).is_err());
    assert_eq!(ops::section_op(SOURCE, SectionOp::Pages([].to_vec())).unwrap().as_bytes(), SOURCE);
    assert_eq!(
        ops::section_op(SOURCE, SectionOp::Pages([PageEdit::set_level(ids[0], 1).unwrap()].to_vec()))
            .unwrap()
            .as_bytes(),
        SOURCE
    );
    assert!(ops::section_op(SOURCE, SectionOp::Pages([PageEdit::set_level(ids[0], 2).unwrap()].to_vec())).is_err());
    let duplicate = PageEdit::set_level(ids[3], 2).unwrap();
    assert!(ops::section_op(SOURCE, SectionOp::Pages([duplicate.clone(), duplicate].to_vec())).is_err());
    let missing = ExGuid {
        guid: [0x77; 16],
        n: 1,
    };
    assert!(ops::section_op(SOURCE, SectionOp::Pages([PageEdit::set_level(missing, 1).unwrap()].to_vec())).is_err());
    assert!(
        ops::section_op(SOURCE, SectionOp::Pages([PageEdit::move_to(ids[3], Some(missing), 1).unwrap()].to_vec()))
        .is_err()
    );
    let mut value = serde_json::to_value(PageEdit::set_level(ids[3], 2).unwrap()).unwrap();
    value["level"] = 4.into();
    let invalid: PageEdit = serde_json::from_value(value.clone()).unwrap();
    assert!(ops::section_op(SOURCE, SectionOp::Pages([invalid].to_vec())).is_err());
    value["unknown"] = true.into();
    assert!(serde_json::from_value::<PageEdit>(value).is_err());
}

#[test]
fn moving_pages_preserves_ink_file_data_and_native_feature_objects() {
    for source in [
        include_bytes!("../../../corpus/native-ink/20260905-ui/notebook/synthetic.one").as_slice(),
        include_bytes!("../../../corpus/native-external-assets/notebook/synthetic.one").as_slice(),
        include_bytes!("../../../corpus/m6/native-features-01/notebook/Features.one").as_slice(),
    ] {
        let first = pages(source)[0];
        let created =
            onestore::PageCreation::new(Some(first), Some("Movement anchor"), "Author").unwrap();
        let source = ops::section_op(source, SectionOp::Create(created.clone()))
            .unwrap()
            .as_bytes()
            .to_vec();
        let moved =
            ops::section_op(&source, SectionOp::Pages([PageEdit::move_to(first, None, 1).unwrap()].to_vec())).unwrap();
        assert_eq!(pages(moved.as_bytes()).last(), Some(&first));
        let stores = [&source, moved.as_bytes()].map(|bytes| Store::parse(bytes).unwrap());
        let indexes = stores
            .each_ref()
            .map(|store| RevisionIndex::parse(store).unwrap());
        for (sid, space) in &indexes[0].spaces {
            for rid in space.revisions.keys() {
                assert_eq!(
                    format!("{:?}", indexes[0].resolve(*sid, *rid).unwrap()),
                    format!("{:?}", indexes[1].resolve(*sid, *rid).unwrap())
                );
            }
            if *sid != indexes[0].root {
                assert_eq!(space.labels, indexes[1].spaces[sid].labels);
                let revision = indexes[0]
                    .resolve(*sid, space.labels[&(ExGuid::default(), 1)])
                    .unwrap();
                for object in revision.objects.values() {
                    if let Some(onestore::FileDataReference::Internal(guid)) =
                        object.file_reference().unwrap()
                    {
                        assert_eq!(
                            stores[0].file_data(guid).unwrap(),
                            stores[1].file_data(guid).unwrap()
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn section_reordering_refreshes_stale_native_metadata_levels() {
    let source = include_bytes!(
        "../../../corpus/page-lifecycle/movement/02-promoted-parent/notebook/Lifecycle.one"
    );
    let ids = pages(source);
    let edits: Vec<_> = ids[3..6]
        .iter()
        .enumerate()
        .map(|(i, sid)| PageEdit::move_to(*sid, None, u32::try_from(i + 1).unwrap()).unwrap())
        .collect();
    let prepared = ops::section_op(source, SectionOp::Pages(edits.to_vec())).unwrap();
    let stores = [source.as_slice(), prepared.as_bytes()].map(|b| Store::parse(b).unwrap());
    let indexes = stores.each_ref().map(|s| RevisionIndex::parse(s).unwrap());
    let documents = indexes.each_ref().map(|i| Document::parse(i).unwrap());
    let levels = documents.each_ref().map(|document| {
        let section = &document.spaces[&document.root];
        let view = &section.revisions[&section.contexts[&ExGuid::default()]];
        let copies: Vec<_> = view
            .nodes
            .iter()
            .filter_map(|(id, node)| match &node.kind {
                Kind::Metadata {
                    title: Some(title),
                    level,
                } if title == "Same title" => Some((*id, *level)),
                _ => None,
            })
            .collect();
        assert_eq!(copies.len(), 1);
        copies[0]
    });
    assert_eq!(levels[0].0, levels[1].0);
    assert_eq!(levels[0].1, Some(2));
    assert_eq!(levels[1].1, Some(1));
    for (sid, space) in &indexes[0].spaces {
        for rid in space.revisions.keys() {
            assert_eq!(
                format!("{:?}", indexes[0].resolve(*sid, *rid).unwrap()),
                format!("{:?}", indexes[1].resolve(*sid, *rid).unwrap())
            );
        }
    }
}

#[test]
fn metadata_copy_order_does_not_change_page_identity() {
    let source = include_bytes!(
        "../../../corpus/page-lifecycle/page-edits/optional-cache/source-cold/notebook/Lifecycle.one"
    );
    let sid = pages(source)[3];
    let edited = ops::section_op(source, SectionOp::Pages([PageEdit::set_level(sid, 2).unwrap()].to_vec())).unwrap();
    let expected = include_bytes!(
        "../../../corpus/page-lifecycle/page-edits/optional-cache/candidate/Lifecycle.one"
    );
    export("reordered-metadata", edited.as_bytes());
    verify(source, edited.as_bytes(), expected);
    let stores = [edited.as_bytes(), expected.as_slice()].map(|b| Store::parse(b).unwrap());
    let indexes = stores.each_ref().map(|s| RevisionIndex::parse(s).unwrap());
    let documents = indexes.each_ref().map(|i| Document::parse(i).unwrap());
    for sid in documents[0].spaces.keys() {
        let nodes = documents.each_ref().map(|d| {
            let space = &d.spaces[sid];
            serde_json::to_value(&space.revisions[&space.contexts[&ExGuid::default()]].nodes)
                .unwrap()
        });
        assert_eq!(nodes[0], nodes[1], "{sid}");
    }
}

#[test]
fn native_edits_on_moved_pages_support_more_rust_changes() {
    let source = include_bytes!(
        "../../../corpus/page-lifecycle/page-edits/native/cold/notebook/Lifecycle.one"
    );
    let store = Store::parse(source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let (sid, _) = *document.pages().unwrap().last().unwrap();
    let space = &document.spaces[&sid];
    let view = &space.revisions[&space.contexts[&ExGuid::default()]];
    assert!(
        matches!(&view.nodes[&view.roots[&2]].kind, Kind::Metadata { title: Some(title), level: Some(1) } if title == "Native moved 🦋 é")
    );
    let bodies: Vec<_> = view.nodes.iter().filter_map(|(id, node)|
        matches!(&node.kind, Kind::RichText { text, .. } if text == "Native body after a Rust page move.").then_some(*id)).collect();
    let [body] = bodies.as_slice() else { panic!() };
    let moved = ops::section_op(source, SectionOp::Pages([PageEdit::set_level(sid, 2).unwrap()].to_vec())).unwrap();
    let edited = ops::page_op(moved.as_bytes(), sid, PageOp::Text { text: *body, range: 0..0, with: "Rust + ".into() }).unwrap();
    let after_store = Store::parse(edited.as_bytes()).unwrap();
    let after_index = RevisionIndex::parse(&after_store).unwrap();
    let after = Document::parse(&after_index).unwrap();
    assert_eq!(after.pages().unwrap(), document.pages().unwrap());
    let space = &after.spaces[&sid];
    let view = &space.revisions[&space.contexts[&ExGuid::default()]];
    assert!(
        matches!(&view.nodes[&view.roots[&2]].kind, Kind::Metadata { title: Some(title), level: Some(2) } if title == "Native moved 🦋 é")
    );
    assert!(
        matches!(&view.nodes[body].kind, Kind::RichText { text, .. } if text == "Rust + Native body after a Rust page move.")
    );
    for (sid, space) in &index.spaces {
        for rid in space.revisions.keys() {
            assert_eq!(
                format!("{:?}", index.resolve(*sid, *rid).unwrap()),
                format!("{:?}", after_index.resolve(*sid, *rid).unwrap())
            );
        }
    }
    export("rust-followup", edited.as_bytes());
}

#[test]
fn grouped_page_levels_and_membership_survive_each_storage_interruption() {
    let mut source = onestore::create_section("movement.one", "Original", "Author").unwrap();
    for _ in 0..2 {
        let page = onestore::PageCreation::new(None, Some("Same title"), "Author").unwrap();
        source = ops::section_op(&source, SectionOp::Create(page.clone()))
            .unwrap()
            .as_bytes()
            .to_vec();
    }
    for (source, first) in [
        (source.as_slice(), 1),
        (
            include_bytes!(
                "../../../corpus/page-lifecycle/page-edits/optional-cache/source/Lifecycle.one"
            )
            .as_slice(),
            3,
        ),
    ] {
        let ids = pages(source);
        let edits = [
            PageEdit::set_level(ids[first], 2).unwrap(),
            PageEdit::set_level(ids[first + 1], 3).unwrap(),
        ];
        let prepared = ops::section_op(source, SectionOp::Pages(edits.to_vec())).unwrap();
        let old = current::current(source);
        let new = current::current(prepared.as_bytes());
        assert_eq!(new.len(), old.len());
        // Byte-at-a-time writes tear the commit at every byte, a full sweep's matrix.
        let limits: &[usize] = if sweep::full().is_some() {
            &[1, 17, 4096]
        } else {
            &[17, 4096]
        };
        for &write_limit in limits {
            let mut complete = disk::Disk {
                visible: source.to_vec(),
                durable: source.to_vec(),
                operation: 0,
                fail_at: None,
                write_limit,
                random: 1957,
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
                    random: 1957 + fail_at as u64,
                };
                prepared.commit(&mut interrupted).unwrap_err();
                let observed = current::current(&interrupted.durable);
                assert!(
                    observed == old || observed == new,
                    "{write_limit}:{fail_at}"
                );
            }
        }
    }
}

#[test]
fn repeated_nesting_retains_series_history_across_revision_checkpoints() {
    let mut source = onestore::create_section("movement.one", "Original", "Author").unwrap();
    let page = onestore::PageCreation::new(None, Some("Child"), "Author").unwrap();
    source = ops::section_op(&source, SectionOp::Create(page.clone()))
        .unwrap()
        .as_bytes()
        .to_vec();
    let initial = source.clone();
    for step in 0..520 {
        let level = if step % 2 == 0 { 2 } else { 1 };
        let edit = PageEdit::set_level(page.space(), level).unwrap();
        let written = ops::section_op(&source, SectionOp::Pages([edit].to_vec()))
            .unwrap()
            .as_bytes()
            .to_vec();
        let store = Store::parse(&written).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        index.validate_current().unwrap();
        let document = Document::parse(&index).unwrap();
        assert_eq!(document.pages().unwrap().len(), 2);
        let space = &document.spaces[&page.space()];
        let view = &space.revisions[&space.contexts[&ExGuid::default()]];
        assert!(
            matches!(view.nodes[&view.roots[&2]].kind, Kind::Metadata { level: Some(n), .. } if n == level)
        );
        if step % 64 == 0 || matches!(step, 255 | 511 | 519) {
            let mut old = written.clone();
            old[96..100].copy_from_slice(&source[96..100]);
            assert_eq!(current::current(&old), current::current(&source));
        }
        source = written;
    }
    let stores = [&initial, &source].map(|bytes| Store::parse(bytes).unwrap());
    let indexes = stores
        .each_ref()
        .map(|store| RevisionIndex::parse(store).unwrap());
    assert_eq!(
        stores[1].header.transaction_count,
        stores[0].header.transaction_count + 520
    );
    for (sid, space) in &indexes[0].spaces {
        for rid in space.revisions.keys() {
            assert_eq!(
                format!("{:?}", indexes[0].resolve(*sid, *rid).unwrap()),
                format!("{:?}", indexes[1].resolve(*sid, *rid).unwrap())
            );
        }
        let current = &indexes[1].spaces[sid];
        let rid = current.labels[&(ExGuid::default(), 1)];
        assert!(
            std::iter::successors(Some(rid), |rid| current.revisions[rid].dependency).count() < 20
        );
    }
    if let Some(output) = std::env::var_os("ONESTORE_PAGE_MOVEMENT_STRESS_OUTPUT") {
        assert!(
            std::path::Path::new(&output).is_absolute(),
            "Use an absolute fixture output directory"
        );
        std::fs::create_dir(&output).unwrap();
        std::fs::write(std::path::Path::new(&output).join("movement.one"), &source).unwrap();
    }
}

/// A move on a section OneNote merged, which holds empty page series, rewrites the series
/// without them and gives the moved page one of its own.
#[test]
fn a_move_after_a_native_merge_drops_its_empty_series() {
    let merged = include_bytes!(
        "../../../corpus/conflict-page/native-pages/merged/notebook/synthetic.one"
    );
    let series = |bytes: &[u8]| {
        let store = Store::parse(bytes).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let document = Document::parse(&index).unwrap();
        let space = &document.spaces[&document.root];
        let view = &space.revisions[&space.contexts[&ExGuid::default()]];
        view.nodes[&view.roots[&1]]
            .children
            .iter()
            .map(|id| (*id, view.nodes[id].spaces.clone()))
            .collect::<Vec<_>>()
    };
    let before = series(merged);
    assert!(before.iter().any(|(_, spaces)| spaces.is_empty()));
    let ids = pages(merged);
    let moved = ops::section_op(
        merged,
        SectionOp::Pages(vec![PageEdit::move_to(ids[1], None, 1).unwrap()]),
    )
    .unwrap();
    let after = series(moved.as_bytes());
    assert!(after.iter().all(|(_, spaces)| !spaces.is_empty()));
    let (id, spaces) = after.last().unwrap();
    assert_eq!(spaces, &vec![ids[1]]);
    assert!(before.iter().all(|(old, _)| old != id));
    assert_eq!(pages(moved.as_bytes()), [&ids[..1], &ids[2..], &ids[1..2]].concat());
    if let Some(output) = std::env::var_os("ONESTORE_EMPTY_SERIES_EXPORT") {
        let output = std::path::Path::new(&output);
        std::fs::create_dir(output).unwrap();
        std::fs::write(output.join("synthetic.one"), moved.as_bytes()).unwrap();
    }
}
