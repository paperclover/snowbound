use onestore::{
    ExGuid, OutlineEdit as Edit, PreparedEdit, RevisionIndex, Store,
    document::{Document, Kind},
};
use std::collections::BTreeSet;

#[path = "support/current.rs"]
mod current;
#[path = "support/disk.rs"]
mod disk;

const SOURCE: &[u8] = include_bytes!("../../../corpus/outline-edit/before/notebook/synthetic.one");

fn cases(document: &Document<'_>) -> Vec<(String, ExGuid, ExGuid, Edit)> {
    let mut cases = Vec::new();
    for (sid, page) in document.pages().unwrap() {
        let space = &document.spaces[&sid];
        let view = &space.revisions[&space.contexts[&ExGuid::default()]];
        let Kind::Metadata {
            title: Some(name), ..
        } = &view.nodes[&view.roots[&2]].kind
        else {
            panic!()
        };
        let edit = match name.as_str() {
            "Move outline" => Edit::Position { x: 180.0, y: 216.0 },
            "Resize outline" => Edit::Width {
                points: 144.0,
                user_set: true,
            },
            "Automatic outline size" => Edit::Width {
                points: 360.0,
                user_set: false,
            },
            "Collapse subtree" => Edit::Collapsed(true),
            "Expand subtree" => Edit::Collapsed(false),
            _ => continue,
        };
        let mut object = *view.nodes[&page]
            .children
            .iter()
            .find(|id| matches!(view.nodes[id].kind, Kind::Outline { .. }))
            .unwrap();
        if matches!(edit, Edit::Collapsed(_)) {
            object = *view.nodes[&object].children.iter().find(|id| {
                view.nodes[id].content.first().is_some_and(|text| matches!(
                    &view.nodes[text].kind, Kind::RichText { text, .. } if text.starts_with("Target ")
                ))
            }).unwrap();
        }
        cases.push((name.clone(), sid, object, edit));
    }
    assert_eq!(cases.len(), 5);
    cases
}

#[test]
fn geometry_and_expansion_preserve_unrelated_properties_objects_and_history() {
    let store = Store::parse(SOURCE).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    for (name, sid, object, edit) in cases(&document) {
        let restored = serde_json::from_value(serde_json::to_value(edit).unwrap()).unwrap();
        assert_eq!(edit, restored);
        let prepared = PreparedEdit::outline(SOURCE, sid, object, restored).unwrap();
        let after_store = Store::parse(prepared.as_bytes()).unwrap();
        assert!(after_store.checksum_mismatches.is_empty());
        let after_index = RevisionIndex::parse(&after_store).unwrap();
        after_index.validate_current().unwrap();
        let after_document = Document::parse(&after_index).unwrap();
        let space = &document.spaces[&sid];
        let before = &space.revisions[&space.contexts[&ExGuid::default()]];
        let space = &after_document.spaces[&sid];
        let after = &space.revisions[&space.contexts[&ExGuid::default()]];
        let mut changed = BTreeSet::from([before.roots[&2]]);
        let mut pending = vec![object];
        while let Some(id) = pending.pop() {
            if !changed.insert(id) {
                continue;
            }
            pending.extend(before.nodes.iter().filter_map(|(parent, node)| {
                node.children
                    .iter()
                    .chain(&node.content)
                    .chain(&node.structure)
                    .any(|child| *child == id)
                    .then_some(*parent)
            }));
        }
        assert_eq!(
            before.nodes.keys().collect::<Vec<_>>(),
            after.nodes.keys().collect::<Vec<_>>()
        );
        for (id, node) in &before.nodes {
            let mut expected = serde_json::to_value(node).unwrap();
            let actual = serde_json::to_value(&after.nodes[id]).unwrap();
            if changed.contains(id) {
                expected["modified"] = actual["modified"].clone();
            }
            if *id == object {
                match edit {
                    Edit::Position { x, y } => {
                        expected["layout"]["x"] = x.into();
                        expected["layout"]["y"] = y.into();
                    }
                    Edit::Width { points, user_set } => {
                        expected["layout"]["max_width"] = points.into();
                        expected["layout"]["width_set_by_user"] = user_set.into();
                        let fields = expected["extra"][0].as_array_mut().unwrap();
                        fields.retain(|field| field["id"].as_u64() != Some(0x14001cdb));
                    }
                    Edit::Collapsed(value) => {
                        expected["kind"]["collapse_state"] = u8::from(value).into()
                    }
                }
            }
            let mut actual = actual;
            // Property order is not semantic; every property and nested value still compares exactly.
            expected["extra"][0]
                .as_array_mut()
                .unwrap()
                .sort_by_key(|field| field["id"].as_u64().unwrap());
            actual["extra"][0]
                .as_array_mut()
                .unwrap()
                .sort_by_key(|field| field["id"].as_u64().unwrap());
            assert_eq!(actual, expected, "{name}: {id}");
        }
        for (space_id, old_space) in &index.spaces {
            for revision in old_space.revisions.keys() {
                let old = index.resolve(*space_id, *revision).unwrap();
                let retained = after_index.resolve(*space_id, *revision).unwrap();
                assert_eq!(old.roots, retained.roots);
                for (id, value) in old.objects {
                    assert_eq!(value.data, retained.objects[&id].data);
                }
            }
            if *space_id != sid {
                assert_eq!(old_space.labels, after_index.spaces[space_id].labels);
            }
        }
        let again = PreparedEdit::outline(prepared.as_bytes(), sid, object, edit).unwrap();
        assert_eq!(again.as_bytes(), prepared.as_bytes());
    }
}

#[test]
fn invalid_geometry_and_non_outline_targets_are_rejected() {
    let store = Store::parse(SOURCE).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    for (_, sid, object, _) in cases(&document) {
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            for edit in [
                Edit::Position { x: value, y: 0.0 },
                Edit::Position { x: 0.0, y: value },
                Edit::Width {
                    points: value,
                    user_set: true,
                },
            ] {
                assert!(PreparedEdit::outline(SOURCE, sid, object, edit).is_err());
            }
        }
        for points in [-1.0, 0.0, 35.999] {
            assert!(
                PreparedEdit::outline(
                    SOURCE,
                    sid,
                    object,
                    Edit::Width {
                        points,
                        user_set: false
                    }
                )
                .is_err()
            );
        }
        let space = &document.spaces[&sid];
        let view = &space.revisions[&space.contexts[&ExGuid::default()]];
        for (id, node) in &view.nodes {
            if !matches!(node.kind, Kind::Outline { .. }) {
                assert!(
                    PreparedEdit::outline(SOURCE, sid, *id, Edit::Position { x: 72.0, y: 72.0 })
                        .is_err()
                );
            }
            if matches!(node.kind, Kind::Title) {
                let mut descendants = node.children.clone();
                while let Some(child) = descendants.pop() {
                    descendants.extend(&view.nodes[&child].children);
                    for edit in [
                        Edit::Position { x: 72.0, y: 72.0 },
                        Edit::Width {
                            points: 144.0,
                            user_set: true,
                        },
                        Edit::Collapsed(true),
                    ] {
                        assert!(PreparedEdit::outline(SOURCE, sid, child, edit).is_err());
                    }
                }
            }
            if !matches!(node.kind, Kind::Paragraph { .. }) {
                assert!(PreparedEdit::outline(SOURCE, sid, *id, Edit::Collapsed(true)).is_err());
            }
        }
        assert!(
            PreparedEdit::outline(SOURCE, ExGuid::default(), object, Edit::Collapsed(true))
                .is_err()
        );
        assert!(
            PreparedEdit::outline(SOURCE, sid, ExGuid::default(), Edit::Collapsed(true)).is_err()
        );
    }
}

#[test]
fn resizing_a_native_reserved_width_preserves_content() {
    let source = include_bytes!("../../../corpus/outline-edit/after/notebook/synthetic.one");
    let store = Store::parse(source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let (_, sid, object, _) = cases(&document)
        .into_iter()
        .find(|(name, ..)| name == "Move outline")
        .unwrap();
    let space = &document.spaces[&sid];
    let before = &space.revisions[&space.contexts[&ExGuid::default()]];
    assert!(before.nodes[&object].layout.reserved_width.is_some());
    let edit = PreparedEdit::outline(
        source,
        sid,
        object,
        Edit::Width {
            points: 144.0,
            user_set: true,
        },
    )
    .unwrap();
    let after_store = Store::parse(edit.as_bytes()).unwrap();
    let after_index = RevisionIndex::parse(&after_store).unwrap();
    let after_document = Document::parse(&after_index).unwrap();
    let space = &after_document.spaces[&sid];
    let after = &space.revisions[&space.contexts[&ExGuid::default()]];
    assert_eq!(after.nodes[&object].layout.reserved_width, None);
    assert_eq!(after.nodes[&object].layout.max_width, Some(144.0));
    for (id, node) in &before.nodes {
        assert_eq!(node.children, after.nodes[id].children);
        assert_eq!(node.content, after.nodes[id].content);
        if matches!(node.kind, Kind::RichText { .. }) {
            assert_eq!(
                serde_json::to_value(node).unwrap(),
                serde_json::to_value(&after.nodes[id]).unwrap()
            );
        }
    }
    if let Some(output) = std::env::var_os("ONESTORE_RESERVED_WIDTH_OUTPUT") {
        let output = std::path::PathBuf::from(output);
        assert!(output.is_absolute());
        std::fs::create_dir(&output).unwrap();
        std::fs::write(output.join("synthetic.one"), edit.as_bytes()).unwrap();
    }
}

#[test]
fn outline_movement_updates_both_automatic_title_fields_without_changing_content() {
    let source = onestore::create_section("titles.one", "First", "Author").unwrap();
    let store = Store::parse(&source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let (sid, page) = document.pages().unwrap()[0];
    let insertion =
        onestore::Insertion::outline(page, 144.0, 144.0, "Second 🦋 é", "Author").unwrap();
    let source = PreparedEdit::insert(&source, sid, &insertion)
        .unwrap()
        .as_bytes()
        .to_vec();
    let store = Store::parse(&source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let space = &document.spaces[&sid];
    let rid = space.contexts[&ExGuid::default()];
    let before = &space.revisions[&rid];
    for (x, y, expected) in [
        (144.0, 0.0, "Second 🦋 é"),
        (0.0, 36.0, "Second 🦋 é"),
        (144.0, 36.0, "First"),
        (0.0, 144.0, "First"),
    ] {
        let edit = PreparedEdit::outline(&source, sid, insertion.object(), Edit::Position { x, y })
            .unwrap();
        let saved = Store::parse(edit.as_bytes()).unwrap();
        let saved_index = RevisionIndex::parse(&saved).unwrap();
        saved_index.validate_current().unwrap();
        let saved_document = Document::parse(&saved_index).unwrap();
        let saved_space = &saved_document.spaces[&sid];
        let after = &saved_space.revisions[&saved_space.contexts[&ExGuid::default()]];
        let metadata = before.roots[&2];
        assert!(
            matches!(&after.nodes[&metadata].kind, Kind::Metadata { title: Some(title), .. } if title == expected)
        );
        assert!(
            matches!(&after.nodes[&page].kind, Kind::Page { alternate_title: Some(title), .. } if title == expected)
        );
        assert_eq!(
            before.nodes.keys().collect::<Vec<_>>(),
            after.nodes.keys().collect::<Vec<_>>()
        );
        for (oid, node) in &before.nodes {
            if ![page, metadata, insertion.object()].contains(oid) {
                assert_eq!(
                    serde_json::to_value(node).unwrap(),
                    serde_json::to_value(&after.nodes[oid]).unwrap()
                );
            }
        }
        assert_eq!(
            format!("{:?}", index.resolve(sid, rid).unwrap()),
            format!("{:?}", saved_index.resolve(sid, rid).unwrap())
        );
        assert_eq!(
            PreparedEdit::outline(
                edit.as_bytes(),
                sid,
                insertion.object(),
                Edit::Position { x, y }
            )
            .unwrap()
            .as_bytes(),
            edit.as_bytes()
        );
    }
}

#[test]
fn native_automatic_and_explicit_titles_follow_outline_movement() {
    let source = include_bytes!(
        "../../../corpus/outline-edit/automatic-title/before/notebook/TitleControl.one"
    );
    let store = Store::parse(source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let mut candidate = source.to_vec();
    let pages = document.pages().unwrap();
    assert_eq!(pages.len(), 3);
    for (sid, page) in pages {
        let space = &document.spaces[&sid];
        let view = &space.revisions[&space.contexts[&ExGuid::default()]];
        let Kind::Metadata {
            title: Some(title), ..
        } = &view.nodes[&view.roots[&2]].kind
        else {
            panic!()
        };
        let outline = *view.nodes[&page].children.iter().find(|oid| {
            view.nodes[oid].children.iter().any(|paragraph| view.nodes[paragraph].content.iter().any(|text| {
                matches!(&view.nodes[text].kind, Kind::RichText { text, .. } if text.starts_with("Second"))
            }))
        }).unwrap();
        let change = Edit::Position {
            x: 36.0,
            y: if title == "Horizontal first" {
                108.0
            } else {
                72.0
            },
        };
        candidate = PreparedEdit::outline(&candidate, sid, outline, change)
            .unwrap()
            .as_bytes()
            .to_vec();
        let store = Store::parse(&candidate).unwrap();
        let saved = RevisionIndex::parse(&store).unwrap();
        saved.validate_current().unwrap();
        let document = Document::parse(&saved).unwrap();
        let space = &document.spaces[&sid];
        let after = &space.revisions[&space.contexts[&ExGuid::default()]];
        let expected = if title == "Explicit title" {
            "Explicit title"
        } else {
            "Second 🦋 é"
        };
        assert!(
            matches!(&after.nodes[&after.roots[&2]].kind, Kind::Metadata { title: Some(title), .. } if title == expected)
        );
        for (oid, node) in &view.nodes {
            assert_eq!(node.children, after.nodes[oid].children);
            assert_eq!(node.content, after.nodes[oid].content);
            if matches!(node.kind, Kind::RichText { .. }) {
                assert_eq!(
                    serde_json::to_value(node).unwrap(),
                    serde_json::to_value(&after.nodes[oid]).unwrap()
                );
            }
        }
    }
    if let Some(output) = std::env::var_os("ONESTORE_OUTLINE_TITLE_OUTPUT") {
        let output = std::path::PathBuf::from(output);
        assert!(output.is_absolute());
        std::fs::create_dir(&output).unwrap();
        std::fs::write(output.join("TitleControl.one"), candidate).unwrap();
    }
}

#[test]
fn outline_publication_interruptions_reopen_as_complete_old_or_new_layout() {
    let source = onestore::create_section("layout.one", "Before 🦀 after", "Author").unwrap();
    let store = Store::parse(&source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let (sid, page) = document.pages().unwrap()[0];
    let space = &document.spaces[&sid];
    let view = &space.revisions[&space.contexts[&ExGuid::default()]];
    let outline = *view.nodes[&page]
        .children
        .iter()
        .find(|id| matches!(view.nodes[id].kind, Kind::Outline { .. }))
        .unwrap();
    let paragraph = view.nodes[&outline].children[0];
    let second = onestore::Insertion::outline(page, 144.0, 36.0, "Second", "Author").unwrap();
    let source = PreparedEdit::insert(&source, sid, &second)
        .unwrap()
        .as_bytes()
        .to_vec();
    for (object, operation) in [
        (outline, Edit::Position { x: 216.0, y: 72.0 }),
        (
            outline,
            Edit::Width {
                points: 144.0,
                user_set: true,
            },
        ),
        (paragraph, Edit::Collapsed(true)),
    ] {
        let edit = PreparedEdit::outline(&source, sid, object, operation).unwrap();
        let before = current::current(&source);
        let after = current::current(edit.as_bytes());
        for write_limit in [17, 4096] {
            let disk = |fail_at| disk::Disk {
                visible: source.clone(),
                durable: source.clone(),
                operation: 0,
                fail_at,
                write_limit,
                random: 1927,
            };
            let mut success = disk(None);
            edit.commit(&mut success).unwrap();
            assert_eq!(success.durable, edit.as_bytes());
            for at in 1..=success.operation {
                let mut interrupted = disk(Some(at));
                let failure = edit.commit(&mut interrupted).unwrap_err();
                let actual = current::current(&interrupted.durable);
                assert!(
                    actual == before || actual == after,
                    "{operation:?} operation {at}"
                );
                if failure.state == onestore::CommitState::NotCommitted {
                    assert_eq!(actual, before);
                }
            }
        }
    }
}

#[test]
fn repeated_geometry_changes_and_expansion_match_an_independent_model() {
    let original = onestore::create_section("layout.one", "Before 🦀 é\rafter", "Author").unwrap();
    let store = Store::parse(&original).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let (sid, page) = document.pages().unwrap()[0];
    let space = &document.spaces[&sid];
    let view = &space.revisions[&space.contexts[&ExGuid::default()]];
    let outline = *view.nodes[&page]
        .children
        .iter()
        .find(|id| matches!(view.nodes[id].kind, Kind::Outline { .. }))
        .unwrap();
    let paragraph = view.nodes[&outline].children[0];
    let text = view.nodes[&paragraph].content[0];
    let second = onestore::Insertion::outline(page, 144.0, 36.0, "Second", "Author").unwrap();
    let original = PreparedEdit::insert(&original, sid, &second)
        .unwrap()
        .as_bytes()
        .to_vec();
    for seed in 1..=16_u64 {
        let mut rng = seed;
        let mut source = original.clone();
        let mut layout = serde_json::to_value(&view.nodes[&outline].layout).unwrap();
        let mut collapsed = None;
        for step in 0..48 {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            let (object, edit) = match step % 3 {
                0 => {
                    let x = ((rng % 400) as f32 - 100.0) * 18.0;
                    let y = ((rng % 217) as f32 - 60.0) * 18.0;
                    layout["x"] = x.into();
                    layout["y"] = y.into();
                    (outline, Edit::Position { x, y })
                }
                1 => {
                    let points = (2 + rng % 86) as f32 * 18.0;
                    layout["max_width"] = points.into();
                    layout["width_set_by_user"] = (rng & 1 != 0).into();
                    (
                        outline,
                        Edit::Width {
                            points,
                            user_set: rng & 1 != 0,
                        },
                    )
                }
                _ => {
                    collapsed = Some(u8::from(rng & 1 != 0));
                    (paragraph, Edit::Collapsed(rng & 1 != 0))
                }
            };
            source = PreparedEdit::outline(&source, sid, object, edit)
                .unwrap()
                .as_bytes()
                .to_vec();
            let store = Store::parse(&source).unwrap();
            let index = RevisionIndex::parse(&store).unwrap();
            index.validate_current().unwrap();
            let document = Document::parse(&index).unwrap();
            let space = &document.spaces[&sid];
            let current = &space.revisions[&space.contexts[&ExGuid::default()]];
            let x = layout["x"].as_f64().unwrap();
            let y = layout["y"].as_f64().unwrap();
            let title = if y < 36.0 || (y == 36.0 && x <= 144.0) {
                "Before 🦀 é"
            } else {
                "Second"
            };
            assert!(
                matches!(&current.nodes[&current.roots[&2]].kind, Kind::Metadata { title: Some(actual), .. } if actual == title)
            );
            assert!(
                matches!(&current.nodes[&page].kind, Kind::Page { alternate_title: Some(actual), .. } if actual == title)
            );
            assert_eq!(
                serde_json::to_value(&current.nodes[&outline].layout).unwrap(),
                layout
            );
            let Kind::Paragraph { collapse_state, .. } = &current.nodes[&paragraph].kind else {
                panic!()
            };
            assert_eq!(*collapse_state, collapsed);
            assert_eq!(
                current.nodes[&outline].children,
                view.nodes[&outline].children
            );
            assert_eq!(
                serde_json::to_value(&current.nodes[&text]).unwrap(),
                serde_json::to_value(&view.nodes[&text]).unwrap()
            );
        }
    }
}

#[test]
#[ignore = "exports public-API outline candidates for cold native validation"]
fn export_native_outline_candidates() {
    let output = std::path::PathBuf::from(std::env::var_os("ONESTORE_OUTLINE_OUTPUT").unwrap());
    assert!(output.is_absolute());
    std::fs::create_dir(&output).unwrap();
    let store = Store::parse(SOURCE).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let mut bytes = SOURCE.to_vec();
    for (_, sid, object, edit) in cases(&document) {
        bytes = PreparedEdit::outline(&bytes, sid, object, edit)
            .unwrap()
            .as_bytes()
            .to_vec();
    }
    std::fs::write(output.join("synthetic.one"), bytes).unwrap();
}
