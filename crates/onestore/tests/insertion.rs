#[path = "support/checkpoint.rs"]
mod checkpoint;
#[path = "support/current.rs"]
mod current;
#[path = "support/disk.rs"]
mod disk;

use onestore::{
    ExGuid, Insertion, PreparedEdit, RevisionIndex, Store,
    document::{Document, Kind},
};

fn targets(source: &[u8]) -> (ExGuid, ExGuid, ExGuid, ExGuid, ExGuid) {
    let store = Store::parse(source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let (sid, page) = document.pages().unwrap()[0];
    let space = &document.spaces[&sid];
    let view = &space.revisions[&space.contexts[&ExGuid::default()]];
    let outline = view.nodes[&page]
        .children
        .iter()
        .copied()
        .find(|id| matches!(view.nodes[id].kind, Kind::Outline { .. }))
        .unwrap();
    let paragraph = view.nodes[&outline].children[0];
    let text = view.nodes[&paragraph].content[0];
    (sid, page, outline, paragraph, text)
}

#[test]
fn paragraph_and_outline_insertions_publish_metadata_and_references_together() {
    let source = onestore::create_section("insertion.one", "Original", "Original author").unwrap();
    let (sid, page, outline, paragraph, original_text) = targets(&source);
    for (intent, expected_parent, expected_x, expected_y) in [
        (
            Insertion::paragraph(outline, Some(paragraph), "First 🦀\rSecond", "New author")
                .unwrap(),
            outline,
            None,
            None,
        ),
        (
            Insertion::outline(page, 144.0, 18.0, "First 🦀\rSecond", "New author").unwrap(),
            page,
            Some(144.0),
            Some(18.0),
        ),
    ] {
        let prepared = PreparedEdit::insert(&source, sid, &intent).unwrap();
        let store = Store::parse(prepared.as_bytes()).unwrap();
        assert_eq!(
            store.header.transaction_count,
            Store::parse(&source).unwrap().header.transaction_count + 1
        );
        let index = RevisionIndex::parse(&store).unwrap();
        index.validate_current().unwrap();
        let document = Document::parse(&index).unwrap();
        let space = &document.spaces[&sid];
        let view = &space.revisions[&space.contexts[&ExGuid::default()]];
        assert!(
            view.nodes[&expected_parent]
                .children
                .contains(&intent.object())
        );
        assert_eq!(view.nodes[&intent.object()].layout.x, expected_x);
        assert_eq!(view.nodes[&intent.object()].layout.y, expected_y);
        assert!(
            matches!(&view.nodes[&original_text].kind, Kind::RichText { text, .. } if text == "Original")
        );
        assert!(
            matches!(&view.nodes[&intent.text_object()].kind, Kind::RichText { text, .. } if text == "First 🦀\rSecond")
        );
        assert!(
            matches!(&view.nodes[&page].kind, Kind::Page { alternate_title, .. } if alternate_title.as_deref() == Some("First 🦀"))
        );
        assert!(
            matches!(&view.nodes[&view.roots[&2]].kind, Kind::Metadata { title, .. } if title.as_deref() == Some("First 🦀"))
        );
        assert!(
            view.nodes
                .values()
                .any(|node| matches!(&node.kind, Kind::Author { name } if name.as_deref() == Some("New author")))
        );
        let runs = view.text_runs(intent.text_object()).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].format.font.as_deref(), Some("Calibri"));
        assert_eq!(runs[0].format.font_size, Some(11.0));
        let previous_store = Store::parse(&source).unwrap();
        let previous = RevisionIndex::parse(&previous_store).unwrap();
        for (old_sid, old_space) in &previous.spaces {
            for rid in old_space.revisions.keys() {
                assert_eq!(
                    format!("{:?}", previous.resolve(*old_sid, *rid).unwrap()),
                    format!("{:?}", index.resolve(*old_sid, *rid).unwrap())
                );
            }
        }
    }
}

#[test]
fn repeated_insertions_and_formatting_share_immutable_objects() {
    let mut source = onestore::create_section("shared.one", "Original", "Same author").unwrap();
    let (sid, _, outline, _, _) = targets(&source);
    let mut texts = Vec::new();
    for _ in 0..12 {
        let insertion =
            Insertion::paragraph(outline, None, "Repeated paragraph", "Same author").unwrap();
        source = PreparedEdit::insert(&source, sid, &insertion)
            .unwrap()
            .as_bytes()
            .to_vec();
        source = PreparedEdit::format(
            &source,
            sid,
            insertion.text_object(),
            1..8,
            &[
                onestore::TextAttribute::Bold(true),
                onestore::TextAttribute::FontSize(20.0),
            ],
        )
        .unwrap()
        .as_bytes()
        .to_vec();
        texts.push(insertion.text_object());
    }
    let store = Store::parse(&source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    index.validate_current().unwrap();
    let raw = index
        .resolve(sid, index.spaces[&sid].labels[&(ExGuid::default(), 1)])
        .unwrap();
    let mut unique = std::collections::BTreeSet::new();
    let mut counts = Vec::new();
    for id in raw.reachable().unwrap() {
        let object = &raw.objects[&id];
        if object.jcid & 0x100000 == 0 {
            continue;
        }
        let onestore::ObjectData::Properties(bytes) = object.data else {
            panic!()
        };
        assert!(
            unique.insert((object.jcid, bytes.to_vec())),
            "Duplicate immutable object"
        );
        counts.push((object.jcid, object.reference_count));
    }
    counts.sort();
    assert_eq!(counts, [(0x120001, 26), (0x12004d, 12), (0x12004d, 25)]);
    let document = Document::parse(&index).unwrap();
    let space = &document.spaces[&sid];
    let view = &space.revisions[&space.contexts[&ExGuid::default()]];
    for text in texts {
        let runs = view.text_runs(text).unwrap();
        assert_eq!(runs.len(), 3);
        assert_eq!(runs[1].format.bold, Some(true));
        assert_eq!(runs[1].format.font_size, Some(20.0));
    }
}

#[test]
fn serialized_insertions_rebase_with_the_same_objects_and_preserve_remote_edits() {
    let source = onestore::create_section("rebase.one", "Original", "Author").unwrap();
    let (sid, _, outline, paragraph, text) = targets(&source);
    let intent = Insertion::paragraph(outline, Some(paragraph), "Inserted", "Author").unwrap();
    let encoded = serde_json::to_vec(&intent).unwrap();
    let restored: Insertion = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(intent.object(), restored.object());
    assert_eq!(intent.text_object(), restored.text_object());
    let original_preparation = PreparedEdit::insert(&source, sid, &intent).unwrap();
    let remote = PreparedEdit::text(&source, sid, text, 0..0, "Remote ").unwrap();
    let updated = PreparedEdit::insert(remote.as_bytes(), sid, &restored).unwrap();
    let store = Store::parse(updated.as_bytes()).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let space = &document.spaces[&sid];
    let view = &space.revisions[&space.contexts[&ExGuid::default()]];
    assert_eq!(
        view.nodes[&outline].children,
        [restored.object(), paragraph]
    );
    assert!(
        matches!(&view.nodes[&text].kind, Kind::RichText { text, .. } if text == "Remote Original")
    );
    assert!(
        matches!(&view.nodes[&restored.text_object()].kind, Kind::RichText { text, .. } if text == "Inserted")
    );
    assert!(PreparedEdit::insert(original_preparation.as_bytes(), sid, &restored).is_err());
    assert!(PreparedEdit::insert(updated.as_bytes(), sid, &restored).is_err());
}

#[test]
fn repositioning_preserves_intent_identity_and_kind() {
    let source = onestore::create_section("placement.one", "Original", "Author").unwrap();
    let (sid, page, outline, paragraph, _) = targets(&source);
    let p = Insertion::paragraph(outline, Some(paragraph), "Inserted", "Author").unwrap();
    let o = Insertion::outline(page, 144.0, 144.0, "Inserted", "Author").unwrap();
    assert!(p.reposition_outline(page, 72.0, 72.0).is_err());
    assert!(o.reposition_paragraph(outline, None).is_err());
    assert!(p.reposition_paragraph(ExGuid::default(), None).is_err());
    assert!(o.reposition_outline(page, f32::NAN, 72.0).is_err());
    for (original, moved) in [
        (&p, p.reposition_paragraph(outline, None).unwrap()),
        (&o, o.reposition_outline(page, 288.0, 360.0).unwrap()),
    ] {
        let mut before = serde_json::to_value(original).unwrap();
        let mut after = serde_json::to_value(&moved).unwrap();
        for name in ["parent", "placement"] {
            before.as_object_mut().unwrap().remove(name);
            after.as_object_mut().unwrap().remove(name);
        }
        assert_eq!(before, after);
        assert_eq!(original.object(), moved.object());
        assert_eq!(original.text_object(), moved.text_object());
        let restored: Insertion =
            serde_json::from_value(serde_json::to_value(&moved).unwrap()).unwrap();
        let prepared = PreparedEdit::insert(&source, sid, &restored).unwrap();
        let store = Store::parse(prepared.as_bytes()).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        index.validate_current().unwrap();
        let doc = Document::parse(&index).unwrap();
        let space = &doc.spaces[&sid];
        let view = &space.revisions[&space.contexts[&ExGuid::default()]];
        if original == &p {
            assert_eq!(view.nodes[&outline].children, [paragraph, moved.object()]);
        } else {
            assert_eq!(
                (
                    view.nodes[&moved.object()].layout.x,
                    view.nodes[&moved.object()].layout.y
                ),
                (Some(288.0), Some(360.0))
            );
        }
    }
}

#[test]
#[cfg(any(unix, windows))]
fn filesystem_insertion_uses_the_prepared_identity_and_rejects_stale_replay() {
    use std::{fs, io::Write};
    let source = onestore::create_section("file.one", "Original", "Author").unwrap();
    let (sid, _, outline, _, _) = targets(&source);
    let intent = Insertion::paragraph(outline, None, "File insertion", "Author").unwrap();
    let path = std::env::temp_dir().join(format!("onestore-insertion-{}.one", intent.object()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    file.write_all(&source).unwrap();
    file.sync_all().unwrap();
    drop(file);
    let edit = PreparedEdit::insert(&source, sid, &intent).unwrap();
    let result = edit.commit_file(&path);
    let written = onestore::read_file(&path).unwrap();
    let repeated = edit.commit_file(&path).unwrap_err();
    fs::remove_file(path).unwrap();
    result.unwrap();
    assert_eq!(written, edit.as_bytes());
    assert_eq!(repeated.state, onestore::CommitState::NotCommitted);
    assert_eq!(repeated.error.kind(), std::io::ErrorKind::ResourceBusy);
}

#[test]
fn anchors_targets_serialized_identities_and_text_are_validated_before_publication() {
    let source = onestore::create_section("invalid.one", "Original", "Author").unwrap();
    let (sid, page, outline, paragraph, text) = targets(&source);
    assert!(Insertion::outline(page, f32::NAN, 0.0, "Text", "Author").is_err());
    assert!(Insertion::outline(page, 0.0, f32::INFINITY, "Text", "Author").is_err());
    for content in ["a\0b", "a\nb", "a\u{fffc}b", "a\u{fddf}b"] {
        assert!(Insertion::paragraph(outline, None, content, "Author").is_err());
    }
    assert!(Insertion::paragraph(outline, None, "Text", "a\0b").is_err());
    for intent in [
        Insertion::paragraph(outline, Some(text), "Text", "Author").unwrap(),
        Insertion::paragraph(text, None, "Text", "Author").unwrap(),
        Insertion::paragraph(page, None, "Text", "Author").unwrap(),
        Insertion::outline(paragraph, 0.0, 0.0, "Text", "Author").unwrap(),
    ] {
        assert!(PreparedEdit::insert(&source, sid, &intent).is_err());
    }
    let intent = Insertion::paragraph(outline, None, "Text", "Author").unwrap();
    let created = PreparedEdit::insert(&source, sid, &intent).unwrap();
    let before_missing =
        Insertion::paragraph(outline, Some(intent.object()), "Anchored", "Author").unwrap();
    assert!(PreparedEdit::insert(&source, sid, &before_missing).is_err());
    assert!(PreparedEdit::insert(created.as_bytes(), sid, &before_missing).is_ok());
    let mut encoded = serde_json::to_value(&intent).unwrap();
    encoded["parent"] = serde_json::to_value(intent.object()).unwrap();
    let collision: Insertion = serde_json::from_value(encoded).unwrap();
    assert!(PreparedEdit::insert(created.as_bytes(), sid, &collision).is_err());
}

#[test]
fn insertions_into_nested_paragraphs_and_native_table_cells_preserve_structure() {
    let source = onestore::create_section("nested.one", "Original", "Author").unwrap();
    let (sid, _, outline, paragraph, _) = targets(&source);
    let child = Insertion::paragraph(paragraph, None, "Nested", "Author").unwrap();
    let changed = PreparedEdit::insert(&source, sid, &child).unwrap();
    let store = Store::parse(changed.as_bytes()).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let space = &document.spaces[&sid];
    let view = &space.revisions[&space.contexts[&ExGuid::default()]];
    assert_eq!(view.nodes[&outline].children, [paragraph]);
    assert_eq!(view.nodes[&paragraph].children, [child.object()]);
    let source = include_bytes!(
        "../../../corpus/native/20260905-05/snapshots/07-table/notebook/synthetic.one"
    );
    let store = Store::parse(source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let (sid, cell) = document
        .spaces
        .iter()
        .find_map(|(sid, space)| {
            let view = &space.revisions[&space.contexts[&ExGuid::default()]];
            view.nodes.iter().find_map(|(id, node)| {
                matches!(node.kind, Kind::Cell { .. }).then_some((*sid, *id))
            })
        })
        .unwrap();
    let insertion = Insertion::paragraph(cell, None, "Added to cell", "Author").unwrap();
    let changed = PreparedEdit::insert(source, sid, &insertion).unwrap();
    current::current(changed.as_bytes());
}

#[test]
fn insertion_publication_faults_expose_only_complete_graphs_and_title_caches() {
    let original = onestore::create_section("atomic.one", "Original", "Author").unwrap();
    let (sid, _, _, _, text) = targets(&original);
    let checkpoint = checkpoint::pending(&original, sid, text, 0x14001d7a);
    for source in [
        original.as_slice(),
        include_bytes!("../../../corpus/append/round-01/tx-255/notebook/synthetic.one"),
        &checkpoint,
    ] {
        let (sid, page, outline, paragraph, _) = targets(source);
        for intent in [
            Insertion::paragraph(outline, Some(paragraph), "First 🦀", "New author").unwrap(),
            Insertion::outline(page, 0.0, 0.0, "First 🦀", "New author").unwrap(),
        ] {
            let edit = PreparedEdit::insert(source, sid, &intent).unwrap();
            let before = current::current(source);
            let after = current::current(edit.as_bytes());
            for write_limit in [17, 4096] {
                let disk = |fail_at| disk::Disk {
                    visible: source.to_vec(),
                    durable: source.to_vec(),
                    operation: 0,
                    fail_at,
                    write_limit,
                    random: 945,
                };
                let mut successful = disk(None);
                edit.commit(&mut successful).unwrap();
                assert_eq!(successful.durable, edit.as_bytes());
                for at in 1..=successful.operation {
                    let mut interrupted = disk(Some(at));
                    let failure = edit.commit(&mut interrupted).unwrap_err();
                    let state = current::current(&interrupted.durable);
                    assert!(state == before || state == after, "interruption {at}");
                    if failure.state == onestore::CommitState::NotCommitted {
                        assert_eq!(state, before);
                    }
                    if failure.state == onestore::CommitState::Committed {
                        assert_eq!(state, after);
                    }
                }
            }
        }
    }
}
