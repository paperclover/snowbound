#[path = "support/checkpoint.rs"]
mod checkpoint;
#[path = "support/current.rs"]
mod current;
#[path = "support/disk.rs"]
mod disk;

use onestore::{
    ExGuid, Insertion, PreparedEdit, RevisionIndex, Store, TextAttribute as A,
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
fn atomic_formatted_insertions_match_a_character_model_and_preserve_history() {
    use serde_json::json;
    let source = onestore::create_section("rich.one", "Original", "Author").unwrap();
    let (sid, page, outline, _, _) = targets(&source);
    let text = "ab🦀 e\u{301} 東京\rEnd\t!";
    let plain = Insertion::paragraph(outline, None, text, "Author").unwrap();
    assert!(
        serde_json::to_value(&plain)
            .unwrap()
            .get("formats")
            .is_none()
    );
    let restored: Insertion = serde_json::from_slice(&serde_json::to_vec(&plain).unwrap()).unwrap();
    assert_eq!(plain, restored);
    let base = PreparedEdit::insert(&source, sid, &plain).unwrap();
    let base_store = Store::parse(base.as_bytes()).unwrap();
    let base_index = RevisionIndex::parse(&base_store).unwrap();
    let base_doc = Document::parse(&base_index).unwrap();
    let base_space = &base_doc.spaces[&sid];
    let base_view = &base_space.revisions[&base_space.contexts[&ExGuid::default()]];
    let default =
        serde_json::to_value(&base_view.text_runs(plain.text_object()).unwrap()[0].format).unwrap();
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
        let mut intent = if seed % 2 == 0 {
            Insertion::outline(page, 72.0, 144.0, text, "Author").unwrap()
        } else {
            plain.clone()
        };
        // Descending construction also exercises persisted ordering independently of call order.
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
            intent = intent
                .with_formatting(offsets[i]..offsets[i + 1], &attributes)
                .unwrap();
            for (key, value) in values.as_object().unwrap() {
                expected[i].1[key] = value.clone();
            }
        }
        let restored: Insertion =
            serde_json::from_slice(&serde_json::to_vec(&intent).unwrap()).unwrap();
        assert_eq!(restored, intent);
        let edited = PreparedEdit::insert(&source, sid, &restored).unwrap();
        let store = Store::parse(edited.as_bytes()).unwrap();
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
        let actual: Vec<_> = view
            .text_runs(intent.text_object())
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
fn invalid_or_forged_insertion_formats_fail_before_source_parsing() {
    use serde_json::json;
    let source = onestore::create_section("invalid-rich.one", "Original", "Author").unwrap();
    let (sid, _, outline, _, _) = targets(&source);
    let plain = Insertion::paragraph(outline, None, "a🦀e\u{301}b", "Author").unwrap();
    for (start, end) in [(0, 0), (2, 3), (1, 2), (4, 3), (0, 7)] {
        let range = start..end;
        assert!(plain.with_formatting(range, &[A::Bold(true)]).is_err());
    }
    for attributes in [
        vec![],
        vec![A::Bold(true), A::Bold(false)],
        vec![A::FontSize(f32::NAN)],
        vec![A::FontSize(5.5)],
        vec![A::Font("a\0b".into())],
        vec![A::Superscript(true), A::Subscript(true)],
    ] {
        assert!(plain.with_formatting(1..3, &attributes).is_err());
    }
    let formatted = plain.with_formatting(1..3, &[A::Bold(true)]).unwrap();
    for range in [0..3, 1..3, 1..4] {
        assert!(
            formatted
                .with_formatting(range, &[A::Italic(true)])
                .is_err()
        );
    }
    let encoded = serde_json::to_value(&formatted).unwrap();
    for mutate in 0..6 {
        let mut value = encoded.clone();
        match mutate {
            0 => value["formats"][0]["guid"] = serde_json::to_value([0; 16]).unwrap(),
            1 => value["formats"][0]["guid"] = value["guid"].clone(),
            2 => {
                let duplicate = value["formats"][0].clone();
                value["formats"].as_array_mut().unwrap().push(duplicate);
            }
            3 => value["formats"][0]["range"] = json!({"start":2,"end":3}),
            4 => value["formats"][0]["attributes"] = json!([]),
            _ => value["formats"][0]["range"] = json!({"start":0,"end":u32::MAX}),
        }
        let forged: Insertion = serde_json::from_value(value).unwrap();
        let error = PreparedEdit::insert(&[], sid, &forged).err().unwrap();
        assert_eq!(
            error,
            PreparedEdit::insert(&source, sid, &forged).err().unwrap()
        );
    }
}

#[test]
fn formatted_empty_insertions_preserve_the_insertion_style_for_later_typing() {
    let source = onestore::create_section("empty-rich.one", "Original", "Author").unwrap();
    let (sid, page, outline, _, _) = targets(&source);
    for plain in [
        Insertion::outline(page, 72.0, 144.0, "", "Author").unwrap(),
        Insertion::paragraph(outline, None, "", "Author").unwrap(),
    ] {
        let intent = plain
            .with_formatting(0..0, &[A::Italic(true), A::FontSize(18.0)])
            .unwrap();
        assert!(intent.with_formatting(0..0, &[A::Bold(true)]).is_err());
        let inserted = PreparedEdit::insert(&source, sid, &intent).unwrap();
        let typed = PreparedEdit::text(
            inserted.as_bytes(),
            sid,
            intent.text_object(),
            0..0,
            "Typed 🦀",
        )
        .unwrap();
        for (bytes, text) in [(inserted.as_bytes(), ""), (typed.as_bytes(), "Typed 🦀")] {
            let store = Store::parse(bytes).unwrap();
            let index = RevisionIndex::parse(&store).unwrap();
            index.validate_current().unwrap();
            let document = Document::parse(&index).unwrap();
            let space = &document.spaces[&sid];
            let view = &space.revisions[&space.contexts[&ExGuid::default()]];
            let runs = view.text_runs(intent.text_object()).unwrap();
            assert_eq!(runs.len(), 1);
            assert_eq!(runs[0].text, text);
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
        let (sid, page, outline, paragraph, _) = targets(source);
        let insertion = match name {
            "native-paragraph" => {
                Insertion::paragraph(outline, Some(paragraph), text, "Rich author")
            }
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
                Insertion::paragraph(cell, None, text, "Rich author")
            }
            _ => Insertion::outline(
                page,
                72.0,
                144.0,
                if empty { "" } else { text },
                "Rich author",
            ),
        }
        .unwrap();
        let mut intent = insertion;
        if empty {
            intent = intent
                .with_formatting(0..0, &[A::Italic(true), A::FontSize(18.0)])
                .unwrap();
        } else {
            for (label, attributes) in [
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
            ] {
                let start = text[..text.find(label).unwrap()].encode_utf16().count() as u32;
                intent = intent
                    .with_formatting(
                        start..start + label.encode_utf16().count() as u32,
                        &attributes,
                    )
                    .unwrap();
            }
        }
        let inserted = PreparedEdit::insert(source, sid, &intent).unwrap();
        let bytes = if name == "empty-typed" {
            PreparedEdit::text(
                inserted.as_bytes(),
                sid,
                intent.text_object(),
                0..0,
                "Typed café 🦀",
            )
            .unwrap()
            .as_bytes()
            .to_vec()
        } else {
            inserted.as_bytes().to_vec()
        };
        current::current(&bytes);
        let notebook = output.join(name);
        fs::create_dir(&notebook).unwrap();
        fs::write(notebook.join("synthetic.one"), &bytes).unwrap();
        manifest.push(serde_json::json!({"name":name,"space":sid,"intent":intent}));
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
    let intent = Insertion::paragraph(outline, Some(paragraph), "Inserted", "Author")
        .unwrap()
        .with_formatting(0..3, &[A::Bold(true), A::Color(Some([12, 34, 56]))])
        .unwrap();
    let encoded = serde_json::to_vec(&intent).unwrap();
    let restored: Insertion = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(intent.object(), restored.object());
    assert_eq!(intent.text_object(), restored.text_object());
    assert_eq!(intent, restored);
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
    let runs = view.text_runs(restored.text_object()).unwrap();
    assert_eq!(runs[0].text, "Ins");
    assert_eq!(runs[0].format.bold, Some(true));
    assert_eq!(runs[0].format.color, Some(0x38220c));
    assert_eq!(runs[1].text, "erted");
    assert_ne!(runs[1].format.bold, Some(true));
    assert!(PreparedEdit::insert(original_preparation.as_bytes(), sid, &restored).is_err());
    assert!(PreparedEdit::insert(updated.as_bytes(), sid, &restored).is_err());
}

#[test]
fn repositioning_preserves_intent_identity_and_kind() {
    let source = onestore::create_section("placement.one", "Original", "Author").unwrap();
    let (sid, page, outline, paragraph, _) = targets(&source);
    let p = Insertion::paragraph(outline, Some(paragraph), "Inserted", "Author")
        .unwrap()
        .with_formatting(1..4, &[A::Italic(true)])
        .unwrap();
    let o = Insertion::outline(page, 144.0, 144.0, "Inserted", "Author")
        .unwrap()
        .with_formatting(0..8, &[A::FontSize(18.0)])
        .unwrap();
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
            let intent = intent
                .with_formatting(0..3, &[A::Bold(true), A::FontSize(18.0)])
                .unwrap()
                .with_formatting(6..8, &[A::Italic(true), A::Highlight(Some([255, 255, 0]))])
                .unwrap();
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
