#[path = "support/checkpoint.rs"]
mod checkpoint;
#[path = "support/current.rs"]
mod current;
#[path = "support/disk.rs"]
mod disk;
#[path = "support/ops.rs"]
mod ops;

use onestore::{
    ExGuid, RevisionIndex, Store, TextAttribute as A,
    document::{Document, Kind},
    op::{Edit, Op, PageOp},
};
use std::ops::Range;

/// Where an insertion goes: a paragraph in a container before a child of it, or last; or a
/// new outline at a point on the page.
#[derive(Clone, Copy)]
enum Place {
    In(ExGuid, Option<ExGuid>),
    Outline(f32, f32),
}

/// The ops inserting `text` at `place` with `formats` set over UTF-16 ranges of it, and the
/// identities of the new paragraph (or outline) and text.
fn insertion(
    space: ExGuid,
    place: Place,
    text: &str,
    formats: &[(Range<u32>, Vec<A>)],
) -> (Vec<Op>, ExGuid, ExGuid) {
    let paragraph = ops::paragraph(text);
    let text = paragraph.text().unwrap().id;
    let (op, object) = match place {
        Place::In(container, before) => {
            let id = paragraph.id;
            let paragraphs = vec![paragraph];
            (
                PageOp::Insert {
                    container,
                    before,
                    paragraphs,
                },
                id,
            )
        }
        Place::Outline(x, y) => {
            let object = ops::outline(x, y, vec![paragraph]);
            let id = object.id();
            (PageOp::Add { object, before: None }, id)
        }
    };
    let mut result = vec![Op::Page { space, op }];
    for (range, set) in formats {
        let op = PageOp::Format {
            text,
            range: range.clone(),
            set: set.clone(),
            clear: Vec::new(),
        };
        result.push(Op::Page { space, op });
    }
    (result, object, text)
}

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
fn atomic_formatted_insertions_match_a_character_model_and_preserve_history() {
    use serde_json::json;
    let source = onestore::create_section("rich.one", "Original", "Author").unwrap();
    let (sid, page, outline, _, _) = targets(&source);
    let text = "ab🦀 e\u{301} 東京\rEnd\t!";
    let (plain, _, plain_text) = insertion(sid, Place::In(outline, None), text, &[]);
    let base = ops::edited(&source, plain).unwrap();
    let base_store = Store::parse(&base).unwrap();
    let base_index = RevisionIndex::parse(&base_store).unwrap();
    let base_doc = Document::parse(&base_index).unwrap();
    let base_space = &base_doc.spaces[&sid];
    let base_view = &base_space.revisions[&base_space.contexts[&ExGuid::default()]];
    let default =
        serde_json::to_value(&base_view.text_runs(plain_text).unwrap()[0].format).unwrap();
    let offsets: Vec<u32> = std::iter::once(0)
        .chain(text.chars().scan(0, |n, c| {
            *n += c.len_utf16() as u32;
            Some(*n)
        }))
        .collect();
    let old_store = Store::parse(&source).unwrap();
    let old_index = RevisionIndex::parse(&old_store).unwrap();
    for seed in 1..=48_u64 {
        let mut random = seed;
        let mut expected: Vec<_> = text.chars().map(|c| (c, default.clone())).collect();
        let mut formats = Vec::new();
        // Descending construction also exercises the order formats apply in.
        for i in (0..expected.len()).rev() {
            random ^= random << 13;
            random ^= random >> 7;
            random ^= random << 17;
            let (attributes, values) = match random % 8 {
                0 => continue,
                1 => (
                    vec![A::Bold(true), A::Underline(true)],
                    json!({"bold":true,"underline":true}),
                ),
                2 => (
                    vec![A::Italic(true), A::Strike(true)],
                    json!({"italic":true,"strike":true}),
                ),
                3 => (
                    vec![A::Font("Arial".into()), A::FontSize(13.5)],
                    json!({"font":"Arial","font_size":13.5}),
                ),
                4 => (
                    vec![
                        A::Color(Some([12, 34, 56])),
                        A::Highlight(Some([255, 255, 0])),
                    ],
                    json!({"color":0x38220c,"highlight":0x00ffff}),
                ),
                5 => (
                    vec![A::Superscript(true)],
                    json!({"superscript":true,"subscript":false}),
                ),
                6 => (
                    vec![A::Subscript(true)],
                    json!({"subscript":true,"superscript":false}),
                ),
                _ => (
                    vec![A::Bold(false), A::Color(None), A::Highlight(None)],
                    json!({"bold":false,"color":0xff000000_u32,"highlight":0xff000000_u32}),
                ),
            };
            formats.push((offsets[i]..offsets[i + 1], attributes));
            for (key, value) in values.as_object().unwrap() {
                expected[i].1[key] = value.clone();
            }
        }
        let place = if seed % 2 == 0 {
            Place::Outline(72.0, 144.0)
        } else {
            Place::In(outline, None)
        };
        let (ops, _, inserted) = insertion(sid, place, text, &formats);
        let edit = Edit { at: ops::now(), ops };
        let restored: Edit = serde_json::from_slice(&serde_json::to_vec(&edit).unwrap()).unwrap();
        assert_eq!(restored, edit);
        let edited = ops::edited(&source, restored.ops).unwrap();
        let store = Store::parse(&edited).unwrap();
        assert_eq!(
            store.header.transaction_count,
            old_store.header.transaction_count + 1
        );
        let index = RevisionIndex::parse(&store).unwrap();
        index.validate_current().unwrap();
        assert_eq!(
            index.spaces[&sid].revisions.len(),
            old_index.spaces[&sid].revisions.len() + 1
        );
        for (space, old) in &old_index.spaces {
            for revision in old.revisions.keys() {
                assert_eq!(
                    format!("{:?}", old_index.resolve(*space, *revision).unwrap()),
                    format!("{:?}", index.resolve(*space, *revision).unwrap())
                );
            }
        }
        let document = Document::parse(&index).unwrap();
        let space = &document.spaces[&sid];
        let view = &space.revisions[&space.contexts[&ExGuid::default()]];
        if let Place::Outline(..) = place {
            assert!(view.nodes[&page].children.len() > 1);
        }
        let actual: Vec<_> = view
            .text_runs(inserted)
            .unwrap()
            .into_iter()
            .flat_map(|run| {
                let format = serde_json::to_value(run.format).unwrap();
                run.text.chars().map(move |c| (c, format.clone()))
            })
            .collect();
        assert_eq!(actual, expected, "seed {seed}");
    }
}

#[test]
fn formatted_empty_insertions_preserve_the_insertion_style_for_later_typing() {
    let source = onestore::create_section("empty-rich.one", "Original", "Author").unwrap();
    let (sid, _, outline, _, _) = targets(&source);
    for place in [Place::Outline(72.0, 144.0), Place::In(outline, None)] {
        let style = [(0..0, vec![A::Italic(true), A::FontSize(18.0)])];
        let (ops, _, text) = insertion(sid, place, "", &style);
        let inserted = ops::edited(&source, ops).unwrap();
        let typed = ops::page_edited(
            &inserted,
            sid,
            vec![PageOp::Text {
                text,
                range: 0..0,
                with: "Typed 🦀".into(),
            }],
        )
        .unwrap();
        for (bytes, expected) in [(&inserted, ""), (&typed, "Typed 🦀")] {
            let store = Store::parse(bytes).unwrap();
            let index = RevisionIndex::parse(&store).unwrap();
            index.validate_current().unwrap();
            let document = Document::parse(&index).unwrap();
            let space = &document.spaces[&sid];
            let view = &space.revisions[&space.contexts[&ExGuid::default()]];
            let runs = view.text_runs(text).unwrap();
            assert_eq!(runs.len(), 1);
            assert_eq!(runs[0].text, expected);
            assert_eq!(runs[0].format.italic, Some(true));
            assert_eq!(runs[0].format.font_size, Some(18.0));
        }
    }
}

#[test]
#[ignore = "exports formatted insertion candidates for cold native validation"]
fn export_native_formatted_insertions() {
    use std::{fs, path::PathBuf};
    let output = PathBuf::from(std::env::var_os("ONESTORE_INSERT_OUTPUT").unwrap());
    assert!(output.is_absolute());
    fs::create_dir(&output).unwrap();
    let generated = onestore::create_section("rich.one", "Original", "Author").unwrap();
    let unicode = include_bytes!(
        "../../../corpus/native/20260905-05/snapshots/03-format-unicode/notebook/synthetic.one"
    );
    let table = include_bytes!(
        "../../../corpus/native/20260905-05/snapshots/07-table/notebook/synthetic.one"
    );
    let text = "Bold 🦀 italic e\u{301} color 東京\rEnd";
    let mut manifest = Vec::new();
    for (name, source, empty) in [
        ("outline", generated.as_slice(), false),
        ("native-paragraph", unicode.as_slice(), false),
        ("native-cell", table.as_slice(), false),
        ("empty", generated.as_slice(), true),
        ("empty-typed", generated.as_slice(), true),
    ] {
        let (sid, _, outline, paragraph, _) = targets(source);
        let place = match name {
            "native-paragraph" => Place::In(outline, Some(paragraph)),
            "native-cell" => {
                let store = Store::parse(source).unwrap();
                let index = RevisionIndex::parse(&store).unwrap();
                let document = Document::parse(&index).unwrap();
                let space = &document.spaces[&sid];
                let view = &space.revisions[&space.contexts[&ExGuid::default()]];
                let cell = *view
                    .nodes
                    .iter()
                    .find(|(_, node)| matches!(node.kind, Kind::Cell { .. }))
                    .unwrap()
                    .0;
                Place::In(cell, None)
            }
            _ => Place::Outline(72.0, 144.0),
        };
        let formats = if empty {
            vec![(0..0, vec![A::Italic(true), A::FontSize(18.0)])]
        } else {
            [
                ("Bold", vec![A::Bold(true), A::Underline(true)]),
                ("🦀", vec![A::Font("Arial".into()), A::FontSize(13.5)]),
                ("italic", vec![A::Italic(true), A::Strike(true)]),
                ("e\u{301}", vec![A::Superscript(true)]),
                (
                    "color",
                    vec![
                        A::Color(Some([12, 34, 56])),
                        A::Highlight(Some([255, 255, 0])),
                    ],
                ),
                ("東京", vec![A::Subscript(true)]),
                (
                    "End",
                    vec![
                        A::Bold(false),
                        A::Italic(false),
                        A::Color(None),
                        A::Highlight(None),
                    ],
                ),
            ]
            .into_iter()
            .map(|(label, attributes)| {
                let start = text[..text.find(label).unwrap()].encode_utf16().count() as u32;
                (start..start + label.encode_utf16().count() as u32, attributes)
            })
            .collect()
        };
        let (mut ops, object, inserted) =
            insertion(sid, place, if empty { "" } else { text }, &formats);
        if name == "empty-typed" {
            let op = PageOp::Text {
                text: inserted,
                range: 0..0,
                with: "Typed café 🦀".into(),
            };
            ops.push(Op::Page { space: sid, op });
        }
        let edit = Edit { at: ops::now(), ops };
        let bytes = ops::edited(source, edit.ops.clone()).unwrap();
        current::current(&bytes);
        let notebook = output.join(name);
        fs::create_dir(&notebook).unwrap();
        fs::write(notebook.join("synthetic.one"), &bytes).unwrap();
        manifest.push(serde_json::json!({
            "name": name, "space": sid, "object": object, "text": inserted, "edit": edit
        }));
    }
    fs::write(
        output.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();
}

#[test]
fn paragraph_and_outline_insertions_publish_metadata_and_references_together() {
    let source = onestore::create_section("insertion.one", "Original", "Original author").unwrap();
    let (sid, page, outline, paragraph, original_text) = targets(&source);
    for (place, expected_parent, expected_x, expected_y) in [
        (Place::In(outline, Some(paragraph)), outline, None, None),
        (Place::Outline(144.0, 18.0), page, Some(144.0), Some(18.0)),
    ] {
        let (ops, object, text) = insertion(sid, place, "First 🦀\rSecond", &[]);
        let transaction = ops::transaction(&source, "New author", ops).unwrap().unwrap();
        let mut written = source.clone();
        transaction.apply(&mut written).unwrap();
        let store = Store::parse(&written).unwrap();
        assert_eq!(
            store.header.transaction_count,
            Store::parse(&source).unwrap().header.transaction_count + 1
        );
        let index = RevisionIndex::parse(&store).unwrap();
        index.validate_current().unwrap();
        let document = Document::parse(&index).unwrap();
        let space = &document.spaces[&sid];
        let view = &space.revisions[&space.contexts[&ExGuid::default()]];
        assert!(view.nodes[&expected_parent].children.contains(&object));
        assert_eq!(view.nodes[&object].layout.x, expected_x);
        assert_eq!(view.nodes[&object].layout.y, expected_y);
        assert!(
            matches!(&view.nodes[&original_text].kind, Kind::RichText { text, .. } if text == "Original")
        );
        assert!(
            matches!(&view.nodes[&text].kind, Kind::RichText { text, .. } if text == "First 🦀\rSecond")
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
        let runs = view.text_runs(text).unwrap();
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
        let (ops, _, text) = insertion(sid, Place::In(outline, None), "Repeated paragraph", &[]);
        let transaction = ops::transaction(&source, "Same author", ops).unwrap().unwrap();
        transaction.apply(&mut source).unwrap();
        let bold = PageOp::Format {
            text,
            range: 1..8,
            set: vec![A::Bold(true), A::FontSize(20.0)],
            clear: Vec::new(),
        };
        let transaction = ops::transaction(&source, "Same author", vec![Op::Page { space: sid, op: bold }])
            .unwrap()
            .unwrap();
        transaction.apply(&mut source).unwrap();
        texts.push(text);
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
    let formats = [(0..3, vec![A::Bold(true), A::Color(Some([12, 34, 56]))])];
    let (ops, object, inserted) =
        insertion(sid, Place::In(outline, Some(paragraph)), "Inserted", &formats);
    let encoded = serde_json::to_vec(&ops).unwrap();
    let restored: Vec<Op> = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(ops, restored);
    let original = ops::edited(&source, ops).unwrap();
    let remote = ops::page_edited(
        &source,
        sid,
        vec![PageOp::Text {
            text,
            range: 0..0,
            with: "Remote ".into(),
        }],
    )
    .unwrap();
    let updated = ops::edited(&remote, restored.clone()).unwrap();
    let store = Store::parse(&updated).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let space = &document.spaces[&sid];
    let view = &space.revisions[&space.contexts[&ExGuid::default()]];
    assert_eq!(view.nodes[&outline].children, [object, paragraph]);
    assert!(
        matches!(&view.nodes[&text].kind, Kind::RichText { text, .. } if text == "Remote Original")
    );
    assert!(
        matches!(&view.nodes[&inserted].kind, Kind::RichText { text, .. } if text == "Inserted")
    );
    let runs = view.text_runs(inserted).unwrap();
    assert_eq!(runs[0].text, "Ins");
    assert_eq!(runs[0].format.bold, Some(true));
    assert_eq!(runs[0].format.color, Some(0x38220c));
    assert_eq!(runs[1].text, "erted");
    assert_ne!(runs[1].format.bold, Some(true));
    assert!(ops::edited(&original, restored.clone()).is_err());
    assert!(ops::edited(&updated, restored).is_err());
}

#[test]
#[cfg(any(unix, windows))]
fn filesystem_insertion_commits_once_and_rejects_stale_replay() {
    use std::{fs, io::Write};
    let source = onestore::create_section("file.one", "Original", "Author").unwrap();
    let (sid, _, outline, _, _) = targets(&source);
    let (ops, object, _) = insertion(sid, Place::In(outline, None), "File insertion", &[]);
    let path = std::env::temp_dir().join(format!("onestore-insertion-{object}.one"));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    file.write_all(&source).unwrap();
    file.sync_all().unwrap();
    drop(file);
    let transaction = ops::transaction(&source, "Author", ops).unwrap().unwrap();
    let result = transaction.commit_file(&path);
    let written = onestore::read_file(&path).unwrap();
    let repeated = transaction.commit_file(&path).unwrap_err();
    fs::remove_file(path).unwrap();
    result.unwrap();
    let mut expected = source.clone();
    transaction.apply(&mut expected).unwrap();
    assert_eq!(written, expected);
    assert_eq!(repeated.state, onestore::CommitState::NotCommitted);
    assert_eq!(repeated.error.kind(), std::io::ErrorKind::ResourceBusy);
}

#[test]
fn anchors_targets_identities_and_text_are_validated_before_publication() {
    let source = onestore::create_section("invalid.one", "Original", "Author").unwrap();
    let (sid, page, outline, _, text) = targets(&source);
    let refused = |source: &[u8], place, content: &str| {
        ops::edited(source, insertion(sid, place, content, &[]).0).is_err()
    };
    assert!(refused(&source, Place::Outline(f32::NAN, 0.0), "Text"));
    assert!(refused(&source, Place::Outline(0.0, f32::INFINITY), "Text"));
    for content in ["a\0b", "a\nb", "a\u{fffc}b"] {
        assert!(refused(&source, Place::In(outline, None), content));
    }
    assert!(
        ops::transaction(&source, "a\0b", insertion(sid, Place::In(outline, None), "Text", &[]).0)
            .is_err()
    );
    for place in [
        Place::In(outline, Some(text)),
        Place::In(text, None),
        Place::In(page, None),
    ] {
        assert!(refused(&source, place, "Text"));
    }
    let (ops, object, _) = insertion(sid, Place::In(outline, None), "Text", &[]);
    let created = ops::edited(&source, ops.clone()).unwrap();
    let before_created = Place::In(outline, Some(object));
    assert!(refused(&source, before_created, "Anchored"));
    assert!(!refused(&created, before_created, "Anchored"));
    // A new object's identity may not already be on the page.
    assert!(ops::edited(&created, ops).is_err());
}

#[test]
fn insertions_into_nested_paragraphs_and_native_table_cells_preserve_structure() {
    let source = onestore::create_section("nested.one", "Original", "Author").unwrap();
    let (sid, _, outline, paragraph, _) = targets(&source);
    let mut nested = ops::paragraph("Nested");
    nested.level = 2;
    let child = nested.id;
    let insert = PageOp::Insert {
        container: paragraph,
        before: None,
        paragraphs: vec![nested],
    };
    let changed = ops::page_edited(&source, sid, vec![insert]).unwrap();
    let store = Store::parse(&changed).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let space = &document.spaces[&sid];
    let view = &space.revisions[&space.contexts[&ExGuid::default()]];
    assert_eq!(view.nodes[&outline].children, [paragraph]);
    assert_eq!(view.nodes[&paragraph].children, [child]);
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
    let (ops, _, _) = insertion(sid, Place::In(cell, None), "Added to cell", &[]);
    current::current(&ops::edited(source, ops).unwrap());
}

#[test]
fn insertion_publication_faults_expose_only_complete_graphs_and_title_caches() {
    let original = onestore::create_section("atomic.one", "Original", "Author").unwrap();
    let (sid, _, _, _, text) = targets(&original);
    let checkpoint = checkpoint::pending(&original, sid, text);
    for source in [
        original.as_slice(),
        include_bytes!("../../../corpus/append/round-01/tx-255/notebook/synthetic.one"),
        &checkpoint,
    ] {
        let (sid, _, outline, paragraph, _) = targets(source);
        for place in [Place::In(outline, Some(paragraph)), Place::Outline(0.0, 0.0)] {
            let formats = [
                (0..3, vec![A::Bold(true), A::FontSize(18.0)]),
                (6..8, vec![A::Italic(true), A::Highlight(Some([255, 255, 0]))]),
            ];
            let (ops, _, _) = insertion(sid, place, "First 🦀", &formats);
            let edit = ops::transaction(source, "New author", ops).unwrap().unwrap();
            let mut written = source.to_vec();
            edit.apply(&mut written).unwrap();
            let before = current::current(source);
            let after = current::current(&written);
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
                assert_eq!(successful.durable, written);
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
