use super::*;
use onestore::OutlineEdit as Change;

pub(super) fn fixture() -> (Vec<u8>, ExGuid, ExGuid, ExGuid, ExGuid) {
    let source = onestore::create_section("layout.one", "Original 🦀 é", "Author").unwrap();
    let (sid, text, _) = text(&source);
    let store = Store::parse(&source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let space = &document.spaces[&sid];
    let view = &space.revisions[&space.contexts[&ExGuid::default()]];
    let paragraph = *view
        .nodes
        .iter()
        .find(|(_, node)| node.content == [text])
        .unwrap()
        .0;
    let outline = *view
        .nodes
        .iter()
        .find(|(_, node)| node.children.contains(&paragraph))
        .unwrap()
        .0;
    (source, sid, outline, paragraph, text)
}

pub(super) fn node(source: &[u8], sid: ExGuid, object: ExGuid) -> serde_json::Value {
    let store = Store::parse(source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    index.validate_current().unwrap();
    let document = Document::parse(&index).unwrap();
    let space = &document.spaces[&sid];
    let view = &space.revisions[&space.contexts[&ExGuid::default()]];
    serde_json::to_value(&view.nodes[&object]).unwrap()
}

#[test]
fn layout_and_dependent_text_survive_reopen_and_independent_remote_formatting() {
    let (source, sid, outline, paragraph, text_id) = fixture();
    for (object, change) in [
        (outline, Change::Position { x: 180.0, y: 216.0 }),
        (
            outline,
            Change::Width {
                points: 144.0,
                user_set: true,
            },
        ),
        (paragraph, Change::Collapsed(true)),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("cache.sqlite");
        let cache = Replica::create(&path, &source).unwrap();
        let id = cache
            .outline(&source, sid, object, change)
            .unwrap()
            .unwrap();
        let local = cache.snapshot().unwrap();
        let expected = node(&local, sid, object);
        let dependent = cache
            .edit_text(&local, sid, text_id, 0..0, "Local ")
            .unwrap()
            .unwrap();
        let pending = cache.pending().unwrap();
        let local = cache.snapshot().unwrap();
        drop(cache);
        let cache = Replica::open(&path).unwrap();
        assert_eq!(cache.pending().unwrap(), pending);
        assert!(cache.snapshot().unwrap() == local);
        let remote = PreparedEdit::format(
            &source,
            sid,
            text_id,
            0..8,
            &[onestore::TextAttribute::Bold(true)],
        )
        .unwrap();
        let mut server = Server::new(remote.as_bytes());
        assert!(
            matches!(cache.sync_once(&mut server).unwrap(), Some((n, EditStatus::Published { .. })) if n==id)
        );
        let actual = node(&server.durable, sid, object);
        assert_eq!(actual["layout"], expected["layout"]);
        assert_eq!(actual["kind"], expected["kind"]);
        assert_eq!(cache.pending().unwrap().len(), 1);
        drop(cache);
        let cache = Replica::open(&path).unwrap();
        assert!(
            matches!(cache.sync_once(&mut server).unwrap(), Some((n, EditStatus::Published { .. })) if n==dependent)
        );
        assert_eq!(text(&server.durable).2, "Local Original 🦀 é");
        assert!(cache.pending().unwrap().is_empty());
        let store = Store::parse(&server.durable).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let document = Document::parse(&index).unwrap();
        let space = &document.spaces[&sid];
        let view = &space.revisions[&space.contexts[&ExGuid::default()]];
        assert!(view.text_runs(text_id).unwrap()[0].format.bold.unwrap());
        assert_eq!(server.publications, 2);
        drop(cache);
        let cache = Replica::open(&path).unwrap();
        for id in [id, dependent] {
            assert!(matches!(
                cache.status(id).unwrap(),
                Some(EditStatus::Published { .. })
            ));
        }
    }
}

#[test]
fn rebased_outline_movement_uses_remote_title_text_and_confirms_after_reopen() {
    let (source, sid, outline, _, text_id) = fixture();
    let store = Store::parse(&source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let (_, page) = document.pages().unwrap()[0];
    let space = &document.spaces[&sid];
    let metadata = space.revisions[&space.contexts[&ExGuid::default()]].roots[&2];
    let second = onestore::Insertion::outline(page, 144.0, 36.0, "Second", "Author").unwrap();
    let source = PreparedEdit::insert(&source, sid, &second)
        .unwrap()
        .as_bytes()
        .to_vec();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("titles.sqlite");
    let cache = Replica::create(&path, &source).unwrap();
    let movement = cache
        .outline(
            &source,
            sid,
            outline,
            Change::Position { x: 216.0, y: 72.0 },
        )
        .unwrap()
        .unwrap();
    let local = cache.snapshot().unwrap();
    assert_eq!(node(&local, sid, metadata)["kind"]["title"], "Second");
    let dependent = cache
        .edit_text(&local, sid, text_id, 0..0, "Local ")
        .unwrap()
        .unwrap();
    drop(cache);
    let remote =
        PreparedEdit::text(&source, sid, second.text_object(), 0..6, "Remote second 🐈").unwrap();
    let mut server = Server::new(remote.as_bytes());
    server.fault = Fault::UnknownAfter;
    let cache = Replica::open(&path).unwrap();
    assert!(cache.sync_once(&mut server).is_err());
    assert!(matches!(
        cache.status(movement).unwrap(),
        Some(EditStatus::AwaitingConfirmation { .. })
    ));
    assert_eq!(
        node(&server.durable, sid, metadata)["kind"]["title"],
        "Original 🦀 é"
    );
    assert_eq!(
        node(&server.visible, sid, metadata)["kind"]["title"],
        "Remote second 🐈"
    );
    drop(cache);
    for id in [movement, dependent] {
        let cache = Replica::open(&path).unwrap();
        assert!(
            matches!(cache.sync_once(&mut server).unwrap(), Some((actual, EditStatus::Published { .. })) if actual == id)
        );
        assert_eq!(
            node(&server.durable, sid, metadata)["kind"]["title"],
            "Remote second 🐈"
        );
        assert_eq!(
            node(&server.durable, sid, page)["kind"]["alternate_title"],
            "Remote second 🐈"
        );
    }
    let cache = Replica::open(&path).unwrap();
    assert!(cache.pending().unwrap().is_empty());
    assert_eq!(
        node(&cache.snapshot().unwrap(), sid, text_id)["kind"]["text"],
        "Local Original 🦀 é"
    );
    assert_eq!(server.publications, 2);
}

#[test]
fn competing_layout_requires_current_review_and_preserves_dependent_edits() {
    let (source, sid, outline, _, text_id) = fixture();
    for (local_change, remote_change) in [
        (
            Change::Position { x: 180.0, y: 216.0 },
            Change::Position { x: 288.0, y: 216.0 },
        ),
        (
            Change::Width {
                points: 144.0,
                user_set: true,
            },
            Change::Width {
                points: 216.0,
                user_set: false,
            },
        ),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("cache.sqlite");
        let cache = Replica::create(&path, &source).unwrap();
        let id = cache
            .outline(&source, sid, outline, local_change)
            .unwrap()
            .unwrap();
        let dependent = cache
            .edit_text(&cache.snapshot().unwrap(), sid, text_id, 0..0, "Local ")
            .unwrap()
            .unwrap();
        let local = cache.snapshot().unwrap();
        let queue = cache.pending().unwrap();
        let remote = PreparedEdit::outline(&source, sid, outline, remote_change).unwrap();
        let mut server = Server::new(remote.as_bytes());
        assert_eq!(
            cache.sync_once(&mut server).unwrap(),
            Some((id, EditStatus::Conflict(ConflictKind::LayoutChanged)))
        );
        assert_eq!(server.publications, 0);
        assert_eq!(cache.pending().unwrap(), queue);
        assert!(cache.snapshot().unwrap() == local);
        assert!(
            cache
                .rebase_layout_conflict(id, &source, &server.visible)
                .is_err()
        );
        assert!(cache.rebase_layout_conflict(id, &local, &source).is_err());
        assert!(
            cache
                .rebase_conflict(id, &local, &server.visible, 0..0)
                .is_err()
        );
        drop(cache);
        let cache = Replica::open(&path).unwrap();
        assert_eq!(
            cache.status(id).unwrap(),
            Some(EditStatus::Conflict(ConflictKind::LayoutChanged))
        );
        cache
            .rebase_layout_conflict(id, &local, &server.visible)
            .unwrap();
        let reviewed = cache.pending().unwrap();
        assert_eq!(reviewed[1], queue[1]);
        assert!(cache.snapshot().unwrap() == local);
        drop(cache);
        let cache = Replica::open(&path).unwrap();
        assert_eq!(cache.pending().unwrap(), reviewed);
        for id in [id, dependent] {
            assert!(
                matches!(cache.sync_once(&mut server).unwrap(), Some((n, EditStatus::Published { .. })) if n == id)
            );
        }
        assert_eq!(
            node(&server.durable, sid, outline)["layout"],
            node(&local, sid, outline)["layout"]
        );
        assert_eq!(text(&server.durable).2, "Local Original 🦀 é");
    }
}

#[test]
fn independent_axes_converge_without_overwriting_competing_values() {
    let (source, sid, outline, _, _) = fixture();
    let directory = tempfile::tempdir().unwrap();
    let cache = Replica::create(directory.path().join("cache.sqlite"), &source).unwrap();
    let original = node(&source, sid, outline);
    let local = Change::Position { x: 180.0, y: 216.0 };
    let id = cache
        .outline(&source, sid, outline, local)
        .unwrap()
        .unwrap();
    let remote = PreparedEdit::outline(
        &source,
        sid,
        outline,
        Change::Position {
            x: original["layout"]["x"].as_f64().unwrap() as f32,
            y: 216.0,
        },
    )
    .unwrap();
    let mut server = Server::new(remote.as_bytes());
    assert!(
        matches!(cache.sync_once(&mut server).unwrap(), Some((n, EditStatus::Published { .. })) if n==id)
    );
    let actual = node(&server.durable, sid, outline);
    assert_eq!(actual["layout"]["x"], 180.0);
    assert_eq!(actual["layout"]["y"], 216.0);
}

#[test]
fn twelve_offline_writers_preserve_all_layout_intents_through_review_and_restarts() {
    let (source, sid, outline, paragraph, _) = fixture();
    let directory = tempfile::tempdir().unwrap();
    std::thread::scope(|scope| {
        let mut writers = Vec::new();
        for actor in 0..12 {
            let source = &source;
            let path = directory.path().join(format!("{actor}.sqlite"));
            writers.push(scope.spawn(move || {
                let cache = Replica::create(&path, source).unwrap();
                for round in 0..4 {
                    for (object, change) in [
                        (
                            outline,
                            Change::Position {
                                x: (actor + 2) as f32 * 36.0,
                                y: (round + 2) as f32 * 36.0,
                            },
                        ),
                        (
                            outline,
                            Change::Width {
                                points: (actor + round + 2) as f32 * 36.0,
                                user_set: true,
                            },
                        ),
                        (paragraph, Change::Collapsed((actor + round) % 2 != 0)),
                    ] {
                        assert!(
                            cache
                                .outline(&cache.snapshot().unwrap(), sid, object, change)
                                .unwrap()
                                .is_some()
                        );
                    }
                }
                assert_eq!(cache.pending().unwrap().len(), 12);
            }));
        }
        for writer in writers {
            writer.join().unwrap();
        }
    });
    let mut server = Server::new(&source);
    let mut expected_layout = node(&source, sid, outline)["layout"].clone();
    let mut expected_collapse = None;
    let mut reviewed = 0;
    for round in 0..12 {
        for actor in 0..12 {
            let path = directory.path().join(format!("{actor}.sqlite"));
            let cache = Replica::open(&path).unwrap();
            let first = cache.pending().unwrap()[0].clone();
            let notebook::Operation::Outline(edit) = first.operation else {
                panic!()
            };
            let local = cache.snapshot().unwrap();
            let result = cache.sync_once(&mut server).unwrap();
            if result == Some((first.id, EditStatus::Conflict(ConflictKind::LayoutChanged))) {
                let remote = cache.remote_snapshot().unwrap();
                cache
                    .rebase_layout_conflict(first.id, &local, &remote)
                    .unwrap();
                drop(cache);
                let cache = Replica::open(&path).unwrap();
                assert!(
                    matches!(cache.sync_once(&mut server).unwrap(), Some((id, EditStatus::Published { .. })) if id == first.id)
                );
                reviewed += 1;
            } else {
                assert!(
                    matches!(result, Some((id, EditStatus::Published { .. })) if id == first.id)
                );
            }
            match edit.change {
                Change::Position { x, y } => {
                    expected_layout["x"] = x.into();
                    expected_layout["y"] = y.into();
                }
                Change::Width { points, user_set } => {
                    expected_layout["max_width"] = points.into();
                    expected_layout["width_set_by_user"] = user_set.into();
                }
                Change::Collapsed(value) => expected_collapse = Some(u8::from(value)),
            }
            assert_eq!(
                node(&server.durable, sid, outline)["layout"],
                expected_layout
            );
            assert_eq!(
                node(&server.durable, sid, paragraph)["kind"]["collapse_state"],
                serde_json::json!(expected_collapse)
            );
            assert_eq!(text(&server.durable).2, "Original 🦀 é");
        }
        for actor in 0..12 {
            let cache = Replica::open(directory.path().join(format!("{actor}.sqlite"))).unwrap();
            assert_eq!(cache.pending().unwrap().len(), 11 - round);
        }
    }
    assert!(reviewed >= 12);
    for actor in 0..12 {
        let cache = Replica::open(directory.path().join(format!("{actor}.sqlite"))).unwrap();
        assert!(cache.pending().unwrap().is_empty());
        for id in 1..=12 {
            assert!(matches!(
                cache.status(id).unwrap(),
                Some(EditStatus::Published { .. })
            ));
        }
    }
}

#[test]
fn uncertain_layout_is_confirmed_by_revision_and_never_by_converged_values() {
    let (source, sid, outline, _, text_id) = fixture();
    for fault in [Fault::UnknownBefore, Fault::UnknownAfter] {
        let before = matches!(fault, Fault::UnknownBefore);
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("cache.sqlite");
        let cache = Replica::create(&path, &source).unwrap();
        let change = Change::Width {
            points: 144.0,
            user_set: true,
        };
        let id = cache
            .outline(&source, sid, outline, change)
            .unwrap()
            .unwrap();
        cache
            .edit_text(&cache.snapshot().unwrap(), sid, text_id, 0..0, "Local ")
            .unwrap();
        let local = cache.snapshot().unwrap();
        let mut server = Server::new(&source);
        server.fault = fault;
        assert!(cache.sync_once(&mut server).is_err());
        let status = cache.status(id).unwrap().unwrap();
        assert!(matches!(status, EditStatus::AwaitingConfirmation { .. }));
        drop(cache);
        let cache = Replica::open(&path).unwrap();
        if before {
            let converged = PreparedEdit::outline(&source, sid, outline, change).unwrap();
            server.visible = converged.as_bytes().to_vec();
            assert_eq!(cache.sync_once(&mut server).unwrap(), Some((id, status)));
            assert!(
                cache
                    .rebase_layout_conflict(id, &local, &server.visible)
                    .is_err()
            );
            assert_eq!(cache.pending().unwrap().len(), 2);
            assert!(cache.snapshot().unwrap() == local);
            let archive = directory.path().join("recovery.sqlite");
            cache.export_recovery(&archive).unwrap();
            let recovery = notebook::Recovery::open(&archive).unwrap();
            assert_eq!(recovery.pending().unwrap(), cache.pending().unwrap());
            assert_eq!(recovery.status(id).unwrap(), Some(status));
            assert_eq!(server.confirmations, 0);
        } else {
            assert!(
                matches!(cache.sync_once(&mut server).unwrap(), Some((n, EditStatus::Published { .. })) if n==id)
            );
            assert_eq!(
                node(&server.durable, sid, outline)["layout"]["max_width"],
                144.0
            );
            assert_eq!(server.confirmations, 1);
        }
        assert_eq!(server.publications, 1);
    }
}

#[test]
fn native_moves_deletions_and_layout_changes_merge_or_retain_explicit_conflicts() {
    let source = include_bytes!("../../../../corpus/outline-edit/before/notebook/synthetic.one");
    let native = include_bytes!("../../../../corpus/outline-edit/after/notebook/synthetic.one");
    let store = Store::parse(source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let mut server = Server::new(native);
    let mut counts = [0; 3];
    let mut records = Vec::new();
    for (sid, page) in document.pages().unwrap() {
        let space = &document.spaces[&sid];
        let view = &space.revisions[&space.contexts[&ExGuid::default()]];
        let Kind::Metadata {
            title: Some(name), ..
        } = &view.nodes[&view.roots[&2]].kind
        else {
            continue;
        };
        if name == "Unicode rich text" {
            continue;
        }
        let outlines: Vec<_> = view.nodes[&page]
            .children
            .iter()
            .filter(|id| matches!(view.nodes[id].kind, Kind::Outline { .. }))
            .copied()
            .collect();
        if outlines.len() != 2 {
            continue;
        }
        let outline = outlines[0];
        let mut pending = vec![outline];
        let mut target = None;
        while let Some(id) = pending.pop() {
            let node = &view.nodes[&id];
            pending.extend(&node.children);
            if node.content.first().is_some_and(|text| matches!(&view.nodes[text].kind, Kind::RichText { text, .. } if text.starts_with("Target "))) {
                target = Some(id);
            }
        }
        let target = target.unwrap();
        let text_id = view.nodes[&view.nodes[&outlines[1]].children[0]].content[0];
        let (object, change, conflict) = match name.as_str() {
            "Move outline" => (
                outline,
                Change::Position { x: 252.0, y: 288.0 },
                Some(ConflictKind::LayoutChanged),
            ),
            "Resize outline" | "Automatic outline size" => (
                outline,
                Change::Width {
                    points: 252.0,
                    user_set: true,
                },
                Some(ConflictKind::LayoutChanged),
            ),
            "Delete outline" => (
                outline,
                Change::Width {
                    points: 252.0,
                    user_set: true,
                },
                Some(ConflictKind::TargetUnavailable),
            ),
            "Delete leaf" | "Delete subtree" | "Delete only paragraph" => (
                target,
                Change::Collapsed(true),
                Some(ConflictKind::TargetUnavailable),
            ),
            "Indent subtree" | "Outdent subtree" => (
                target,
                Change::Collapsed(true),
                Some(ConflictKind::StructureChanged),
            ),
            "Expand subtree" => (target, Change::Collapsed(false), None),
            "Collapse subtree" | "Move leaf down" | "Move subtree down" | "Move subtree up" => {
                (target, Change::Collapsed(true), None)
            }
            _ => panic!("Unexpected native case {name}"),
        };
        let expected_layout = if conflict == Some(ConflictKind::TargetUnavailable) {
            None
        } else {
            let mut layout = node(&server.visible, sid, object)["layout"].clone();
            match change {
                Change::Position { x, y } => {
                    layout["x"] = x.into();
                    layout["y"] = y.into();
                }
                Change::Width { points, user_set } => {
                    layout["max_width"] = points.into();
                    layout["width_set_by_user"] = user_set.into();
                }
                Change::Collapsed(_) => {}
            }
            Some(layout)
        };
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("cache.sqlite");
        let cache = Replica::create(&path, source).unwrap();
        let id = cache.outline(source, sid, object, change).unwrap().unwrap();
        let dependent = cache
            .edit_text(&cache.snapshot().unwrap(), sid, text_id, 0..0, "Local ")
            .unwrap()
            .unwrap();
        let local = cache.snapshot().unwrap();
        let queue = cache.pending().unwrap();
        let result = cache.sync_once(&mut server).unwrap().unwrap();
        let mut retained = false;
        if let Some(kind) = conflict {
            assert_eq!(result, (id, EditStatus::Conflict(kind)), "{name}");
            assert_eq!(cache.pending().unwrap(), queue);
            assert!(cache.snapshot().unwrap() == local);
            drop(cache);
            let cache = Replica::open(&path).unwrap();
            assert_eq!(cache.status(id).unwrap(), Some(EditStatus::Conflict(kind)));
            if kind == ConflictKind::TargetUnavailable {
                assert!(
                    cache
                        .rebase_layout_conflict(id, &local, &server.visible)
                        .is_err()
                );
                counts[2] += 1;
                retained = true;
            } else {
                cache
                    .rebase_layout_conflict(id, &local, &server.visible)
                    .unwrap();
                drop(cache);
                let cache = Replica::open(&path).unwrap();
                assert!(
                    matches!(cache.sync_once(&mut server).unwrap(), Some((n, EditStatus::Published { .. })) if n == id)
                );
                counts[1] += 1;
            }
        } else {
            assert!(
                matches!(result, (n, EditStatus::Published { .. }) if n == id),
                "{name}"
            );
            counts[0] += 1;
            drop(cache);
        }
        let cache = Replica::open(&path).unwrap();
        if retained {
            assert_eq!(cache.pending().unwrap(), queue);
            assert!(cache.snapshot().unwrap() == local);
        } else {
            assert!(
                matches!(cache.sync_once(&mut server).unwrap(), Some((n, EditStatus::Published { .. })) if n == dependent)
            );
            assert_eq!(
                node(&server.durable, sid, object)["layout"],
                expected_layout.unwrap(),
                "{name}"
            );
            if matches!(change, Change::Collapsed(_)) {
                assert_eq!(
                    node(&server.durable, sid, object)["kind"]["collapse_state"],
                    node(&local, sid, object)["kind"]["collapse_state"]
                );
            }
            assert!(
                node(&server.durable, sid, text_id)["kind"]["text"]
                    .as_str()
                    .unwrap()
                    .starts_with("Local ")
            );
            drop(cache);
            let cache = Replica::open(&path).unwrap();
            for id in [id, dependent] {
                assert!(matches!(
                    cache.status(id).unwrap(),
                    Some(EditStatus::Published { .. })
                ));
            }
        }
        records.push(serde_json::json!({"name": name, "space": sid, "object": object, "change": change, "retained": retained, "dependent_text": text_id}));
    }
    assert_eq!(counts, [5, 5, 4]);
    assert_eq!(server.publications, 18);
    if let Some(output) = std::env::var_os("ONESTORE_OFFLINE_OUTLINE_OUTPUT") {
        let output = std::path::PathBuf::from(output);
        assert!(output.is_absolute());
        std::fs::create_dir(&output).unwrap();
        std::fs::write(output.join("synthetic.one"), &server.durable).unwrap();
        std::fs::write(
            output.with_extension("json"),
            serde_json::to_vec_pretty(&records).unwrap(),
        )
        .unwrap();
    }
}

#[test]
fn a_new_native_wrap_reservation_requires_review_before_width_replacement() {
    let source = include_bytes!("../../../../corpus/outline-edit/before/notebook/synthetic.one");
    let native = include_bytes!("../../../../corpus/outline-edit/after/notebook/synthetic.one");
    let store = Store::parse(source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let (sid, page) = document.pages().unwrap().into_iter().find(|(sid, _)| {
        let space = &document.spaces[sid];
        let view = &space.revisions[&space.contexts[&ExGuid::default()]];
        matches!(&view.nodes[&view.roots[&2]].kind, Kind::Metadata { title: Some(name), .. } if name=="Move outline")
    }).unwrap();
    let space = &document.spaces[&sid];
    let view = &space.revisions[&space.contexts[&ExGuid::default()]];
    let outline = *view.nodes[&page]
        .children
        .iter()
        .find(|id| matches!(view.nodes[id].kind, Kind::Outline { .. }))
        .unwrap();
    let old = node(source, sid, outline);
    let remote = node(native, sid, outline);
    assert_eq!(old["layout"]["max_width"], remote["layout"]["max_width"]);
    assert_eq!(
        old["layout"]["width_set_by_user"],
        remote["layout"]["width_set_by_user"]
    );
    assert!(remote["layout"]["reserved_width"].is_number());
    let directory = tempfile::tempdir().unwrap();
    let cache = Replica::create(directory.path().join("cache.sqlite"), source).unwrap();
    let id = cache
        .outline(
            source,
            sid,
            outline,
            Change::Width {
                points: 144.0,
                user_set: true,
            },
        )
        .unwrap()
        .unwrap();
    let mut server = Server::new(native);
    assert_eq!(
        cache.sync_once(&mut server).unwrap(),
        Some((id, EditStatus::Conflict(ConflictKind::LayoutChanged)))
    );
    cache
        .rebase_layout_conflict(id, &cache.snapshot().unwrap(), &server.visible)
        .unwrap();
    assert!(
        matches!(cache.sync_once(&mut server).unwrap(), Some((n, EditStatus::Published { .. })) if n==id)
    );
    let after = node(&server.durable, sid, outline);
    assert_eq!(after["layout"]["x"], remote["layout"]["x"]);
    assert_eq!(after["layout"]["y"], remote["layout"]["y"]);
    assert_eq!(after["layout"]["max_width"], 144.0);
    assert!(after["layout"]["reserved_width"].is_null());
}
