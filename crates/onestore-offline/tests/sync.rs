use onestore::{
    CommitError, CommitIo, CommitState, ExGuid, PreparedEdit, RevisionIndex, Store,
    document::{Document, Kind},
};
use onestore_offline::{ConflictKind, EditStatus, Error, Remote, Replica};
use std::io;

mod paragraph {
    use super::*;
    use onestore::{Insertion, ParagraphJoin, ParagraphSplit, TextAttribute};

    fn fixture() -> (Vec<u8>, ExGuid, ExGuid, ExGuid) {
        let source = onestore::create_section("paragraph.one", "ab🦀cd", "Fixture").unwrap();
        let (sid, left, _) = text(&source);
        let store = Store::parse(&source).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let doc = Document::parse(&index).unwrap();
        let space = &doc.spaces[&sid];
        let view = &space.revisions[&space.contexts[&ExGuid::default()]];
        let parent = *view
            .nodes
            .iter()
            .find(|(_, node)| node.content == [left])
            .unwrap()
            .0;
        (source, sid, left, parent)
    }

    #[test]
    fn native_splits_retain_independent_local_identities_and_changed_boundaries() {
        let source = include_bytes!("../../../corpus/paragraph-edit/before/notebook/synthetic.one");
        let native = include_bytes!("../../../corpus/paragraph-edit/split/notebook/synthetic.one");
        let manifest: serde_json::Value =
            serde_json::from_str(include_str!("../../../corpus/paragraph-edit/manifest.json"))
                .unwrap();
        let store = Store::parse(source).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let document = Document::parse(&index).unwrap();
        let mut accepted = 0;
        let mut conflicts = 0;
        for case in manifest["cases"].as_array().unwrap() {
            if case["case"] == "Split before hyperlink" {
                continue;
            }
            let left: ExGuid = serde_json::from_value(case["original_text"].clone()).unwrap();
            let native_right: ExGuid = serde_json::from_value(case["new_text"].clone()).unwrap();
            let (sid, _) = document
                .pages()
                .unwrap()
                .into_iter()
                .find(|(sid, _)| {
                    let space = &document.spaces[sid];
                    space.revisions[&space.contexts[&ExGuid::default()]]
                        .nodes
                        .contains_key(&left)
                })
                .unwrap();
            let split = ParagraphSplit::new(
                left,
                u32::try_from(case["offset_utf16"].as_u64().unwrap()).unwrap(),
                "Offline author",
            )
            .unwrap();
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("native-split.sqlite");
            let mut cache = Replica::create(&path, source).unwrap();
            let id = cache.split(source, sid, &split).unwrap().unwrap();
            let current = cache.snapshot().unwrap();
            let dependent = cache
                .edit_text(
                    &current,
                    sid,
                    split.text_object(),
                    0..0,
                    "Local dependent edit: ",
                )
                .unwrap()
                .unwrap();
            let local = cache.snapshot().unwrap();
            let pending = cache.pending().unwrap();
            let mut server = Server::new(native);
            let (published, status) = cache.sync_once(&mut server).unwrap().unwrap();
            assert_eq!(published, id);
            assert_eq!(cache.snapshot().unwrap(), local);
            if matches!(case["case"].as_str().unwrap(), "Split end" | "Split empty") {
                accepted += 1;
                assert!(
                    matches!(status, EditStatus::Published { .. }),
                    "{}: {status:?}",
                    case["case"]
                );
                drop(cache);
                cache = Replica::open(&path).unwrap();
                assert_eq!(cache.sync_once(&mut server).unwrap().unwrap().0, dependent);
                assert_eq!(server.publications, 2);
                let store = Store::parse(&server.visible).unwrap();
                let index = RevisionIndex::parse(&store).unwrap();
                let document = Document::parse(&index).unwrap();
                let space = &document.spaces[&sid];
                let view = &space.revisions[&space.contexts[&ExGuid::default()]];
                let original: ExGuid =
                    serde_json::from_value(case["original_paragraph"].clone()).unwrap();
                let native_paragraph: ExGuid =
                    serde_json::from_value(case["new_paragraph"].clone()).unwrap();
                let parent = view
                    .nodes
                    .values()
                    .find(|node| node.children.contains(&original))
                    .unwrap();
                let position = parent
                    .children
                    .iter()
                    .position(|id| *id == original)
                    .unwrap();
                assert_eq!(
                    &parent.children[position..position + 3],
                    &[original, split.object(), native_paragraph]
                );
                assert!(
                    matches!(&view.nodes[&native_right].kind, Kind::RichText {text,..} if text.is_empty())
                );
                assert!(
                    matches!(&view.nodes[&split.text_object()].kind, Kind::RichText {text,..} if text == "Local dependent edit: ")
                );
            } else {
                conflicts += 1;
                assert!(
                    matches!(
                        status,
                        EditStatus::Conflict(
                            ConflictKind::TextChanged | ConflictKind::StructureChanged
                        )
                    ),
                    "{}: {status:?}",
                    case["case"]
                );
                assert_eq!(server.publications, 0);
                assert_eq!(cache.pending().unwrap(), pending);
                drop(cache);
                cache = Replica::open(&path).unwrap();
                assert_eq!(cache.snapshot().unwrap(), local);
                assert_eq!(cache.pending().unwrap(), pending);
                assert_eq!(cache.status(id).unwrap(), Some(status));
            }
        }
        assert_eq!((accepted, conflicts), (2, 10));
    }

    #[test]
    fn native_joins_do_not_acknowledge_or_discard_an_independent_local_branch() {
        let source = include_bytes!(
            "../../../corpus/paragraph-edit/join-tags/before/notebook/synthetic.one"
        );
        let native = include_bytes!(
            "../../../corpus/paragraph-edit/join-tags/joined/notebook/synthetic.one"
        );
        let store = Store::parse(source).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let document = Document::parse(&index).unwrap();
        let mut cases = 0;
        for (sid, page) in document.pages().unwrap() {
            let space = &document.spaces[&sid];
            let view = &space.revisions[&space.contexts[&ExGuid::default()]];
            let Kind::Metadata {
                title: Some(name), ..
            } = &view.nodes[&view.roots[&2]].kind
            else {
                continue;
            };
            if !name.starts_with("Join ") {
                continue;
            }
            cases += 1;
            let outline = view.nodes[&page]
                .children
                .iter()
                .find(|id| matches!(view.nodes[id].kind, Kind::Outline { .. }))
                .unwrap();
            let children = &view.nodes[outline].children;
            let left = view.nodes[&children[0]].content[0];
            let right = view.nodes[&children[1]].content[0];
            let Kind::RichText {
                text: left_text, ..
            } = &view.nodes[&left].kind
            else {
                panic!()
            };
            let survivor = if left_text.is_empty() { right } else { left };
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("native-join.sqlite");
            let mut cache = Replica::create(&path, source).unwrap();
            let join = ParagraphJoin::new(left, right, "Offline author").unwrap();
            let id = cache.join(source, sid, &join).unwrap().unwrap();
            let current = cache.snapshot().unwrap();
            cache
                .edit_text(&current, sid, survivor, 0..0, "Local dependent edit: ")
                .unwrap()
                .unwrap();
            let local = cache.snapshot().unwrap();
            let pending = cache.pending().unwrap();
            let mut server = Server::new(native);
            let (conflicted, status) = cache.sync_once(&mut server).unwrap().unwrap();
            assert_eq!(conflicted, id);
            assert!(
                matches!(
                    status,
                    EditStatus::Conflict(ConflictKind::TargetUnavailable)
                ),
                "{name}: {status:?}"
            );
            assert_eq!(server.publications, 0);
            assert_eq!(cache.snapshot().unwrap(), local);
            assert_eq!(cache.pending().unwrap(), pending);
            drop(cache);
            cache = Replica::open(&path).unwrap();
            assert_eq!(cache.snapshot().unwrap(), local);
            assert_eq!(cache.pending().unwrap(), pending);
            assert_eq!(cache.status(id).unwrap(), Some(status));
        }
        assert_eq!(cases, 3);
    }

    #[test]
    fn split_join_dependencies_reopen_and_rebase_with_remote_text_and_styles() {
        let (source, sid, left, parent) = fixture();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("paragraph.sqlite");
        let mut cache = Replica::create(&path, &source).unwrap();
        let split = ParagraphSplit::new(left, 2, "Local").unwrap();
        let child = Insertion::paragraph(split.object(), None, "Child", "Local").unwrap();
        let join = ParagraphJoin::new(left, split.text_object(), "Local").unwrap();
        let mut ids = Vec::new();
        for step in 0..6 {
            let current = cache.snapshot().unwrap();
            let id = match step {
                0 => cache.split(&current, sid, &split),
                1 => cache.edit_text(&current, sid, split.text_object(), 0..2, "🦋"),
                2 => cache.format(
                    &current,
                    sid,
                    split.text_object(),
                    2..3,
                    &[TextAttribute::Bold(true)],
                ),
                3 => cache.insert(&current, sid, &child),
                4 => cache.join(&current, sid, &join),
                5 => cache.edit_text(&current, sid, left, 5..6, "D"),
                _ => unreachable!(),
            }
            .unwrap()
            .unwrap();
            ids.push(id);
            let saved = cache.snapshot().unwrap();
            let pending = cache.pending().unwrap();
            drop(cache);
            cache = Replica::open(&path).unwrap();
            assert_eq!(cache.snapshot().unwrap(), saved);
            assert_eq!(cache.pending().unwrap(), pending);
        }
        let remote = onestore::replace_text(&source, sid, left, 0..0, "Z").unwrap();
        let remote =
            PreparedEdit::format(&remote, sid, left, 1..2, &[TextAttribute::Italic(true)]).unwrap();
        let mut server = Server::new(remote.as_bytes());
        let local = cache.snapshot().unwrap();
        for (i, id) in ids.iter().enumerate() {
            let (published, state) = cache.sync_once(&mut server).unwrap().unwrap();
            assert_eq!(published, *id);
            assert!(matches!(state, EditStatus::Published { .. }), "{state:?}");
            drop(cache);
            cache = Replica::open(&path).unwrap();
            assert_eq!(cache.status(*id).unwrap(), Some(state));
            if i != ids.len() - 1 {
                assert_eq!(cache.snapshot().unwrap(), local);
            }
        }
        assert_eq!(server.publications, ids.len());
        assert_eq!(server.visible, server.durable);
        assert_eq!(cache.snapshot().unwrap(), server.durable);
        let store = Store::parse(&server.durable).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        index.validate_current().unwrap();
        let doc = Document::parse(&index).unwrap();
        let space = &doc.spaces[&sid];
        let view = &space.revisions[&space.contexts[&ExGuid::default()]];
        assert!(
            matches!(&view.nodes[&left].kind, Kind::RichText { text, .. } if text == "Zab🦋cD")
        );
        assert_eq!(view.nodes[&parent].children, [child.object()]);
        assert_eq!(view.nodes[&parent].content, [left]);
        let runs = view.text_runs(left).unwrap();
        let chars: Vec<_> = runs
            .iter()
            .flat_map(|run| {
                run.text
                    .chars()
                    .map(|c| (c, run.format.bold, run.format.italic))
            })
            .collect();
        assert_eq!(chars.iter().find(|v| v.0 == 'a').unwrap().2, Some(true));
        assert_eq!(chars.iter().find(|v| v.0 == 'c').unwrap().1, Some(true));
    }

    #[test]
    fn changed_split_children_require_fresh_review_without_reallocating_dependents() {
        let (source, sid, left, parent) = fixture();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("paragraph.sqlite");
        let cache = Replica::create(&path, &source).unwrap();
        let split = ParagraphSplit::new(left, 2, "Local").unwrap();
        let id = cache.split(&source, sid, &split).unwrap().unwrap();
        let dependent = cache
            .edit_text(
                &cache.snapshot().unwrap(),
                sid,
                split.text_object(),
                0..2,
                "🦋",
            )
            .unwrap()
            .unwrap();
        let local = cache.snapshot().unwrap();
        let child = Insertion::paragraph(parent, None, "Remote child", "Remote").unwrap();
        let remote = PreparedEdit::insert(&source, sid, &child).unwrap();
        let mut server = Server::new(remote.as_bytes());
        assert_eq!(
            cache.sync_once(&mut server).unwrap(),
            Some((id, EditStatus::Conflict(ConflictKind::StructureChanged)))
        );
        assert_eq!(server.publications, 0);
        assert_eq!(cache.snapshot().unwrap(), local);
        assert!(
            cache
                .rebase_conflict(id, &source, &server.visible, 2..2)
                .is_err()
        );
        assert!(
            cache
                .rebase_conflict(id, &local, &server.visible, 2..3)
                .is_err()
        );
        cache
            .rebase_conflict(id, &local, &server.visible, 2..2)
            .unwrap();
        let pending = cache.pending().unwrap();
        let onestore_offline::Operation::Split(edit) = &pending[0].operation else {
            panic!()
        };
        assert_eq!(edit.intent, split);
        assert_eq!(pending[1].id, dependent);
        drop(cache);
        let cache = Replica::open(&path).unwrap();
        for expected in [id, dependent] {
            let (actual, state) = cache.sync_once(&mut server).unwrap().unwrap();
            assert_eq!(actual, expected);
            assert!(matches!(state, EditStatus::Published { .. }));
        }
        let store = Store::parse(&server.durable).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let doc = Document::parse(&index).unwrap();
        let space = &doc.spaces[&sid];
        let view = &space.revisions[&space.contexts[&ExGuid::default()]];
        assert_eq!(view.nodes[&split.object()].children, [child.object()]);
        assert!(
            matches!(&view.nodes[&split.text_object()].kind, Kind::RichText { text, .. } if text == "🦋cd")
        );
    }

    #[test]
    fn uncertain_paragraph_operations_keep_the_original_attempt_across_reopen() {
        for join in [false, true] {
            for fault in [
                Fault::Before,
                Fault::UnknownBefore,
                Fault::UnknownAfter,
                Fault::Committed,
            ] {
                let (source, sid, left, _) = fixture();
                let split = ParagraphSplit::new(left, 2, "Local").unwrap();
                let source = if join {
                    PreparedEdit::split(&source, sid, &split)
                        .unwrap()
                        .as_bytes()
                        .to_vec()
                } else {
                    source
                };
                let directory = tempfile::tempdir().unwrap();
                let path = directory.path().join("paragraph.sqlite");
                let cache = Replica::create(&path, &source).unwrap();
                let id = if join {
                    cache.join(
                        &source,
                        sid,
                        &ParagraphJoin::new(left, split.text_object(), "Local").unwrap(),
                    )
                } else {
                    cache.split(&source, sid, &split)
                }
                .unwrap()
                .unwrap();
                let local = cache.snapshot().unwrap();
                let intent = cache.pending().unwrap();
                let mut server = Server::new(&source);
                server.fault = fault;
                assert!(cache.sync_once(&mut server).is_err());
                let state = cache.status(id).unwrap().unwrap();
                drop(cache);
                let cache = Replica::open(&path).unwrap();
                assert_eq!(cache.status(id).unwrap(), Some(state));
                if matches!(fault, Fault::UnknownBefore | Fault::UnknownAfter) {
                    assert!(matches!(state, EditStatus::AwaitingConfirmation { .. }));
                    assert_eq!(cache.pending().unwrap(), intent);
                    assert_eq!(cache.snapshot().unwrap(), local);
                    if matches!(fault, Fault::UnknownBefore) {
                        assert_eq!(cache.sync_once(&mut server).unwrap(), Some((id, state)));
                        assert_eq!(server.publications, 1);
                        continue;
                    }
                    server.fault = Fault::Confirm;
                    assert!(cache.sync_once(&mut server).is_err());
                    assert_eq!(cache.status(id).unwrap(), Some(state));
                    let (_, state) = cache.sync_once(&mut server).unwrap().unwrap();
                    assert!(matches!(state, EditStatus::Published { .. }));
                    assert_eq!(server.publications, 1);
                } else if matches!(fault, Fault::Before) {
                    assert_eq!(state, EditStatus::Pending);
                    assert!(matches!(
                        cache.sync_once(&mut server).unwrap().unwrap().1,
                        EditStatus::Published { .. }
                    ));
                    assert_eq!(server.publications, 2);
                } else {
                    assert!(matches!(state, EditStatus::Published { .. }));
                    assert_eq!(server.publications, 1);
                }
                assert!(cache.pending().unwrap().is_empty());
                let saved = cache.snapshot().unwrap();
                if matches!(fault, Fault::UnknownAfter) {
                    // Confirmation refreshes version metadata after reading the acknowledged snapshot.
                    assert!(saved[..212] == server.durable[..212]);
                    assert!(saved[252..] == server.durable[252..]);
                    assert_eq!(
                        Store::parse(&server.durable).unwrap().header.generation,
                        Store::parse(&saved).unwrap().header.generation + 1
                    );
                } else {
                    assert!(saved == server.durable);
                }
                assert_eq!(cache.sync_once(&mut server).unwrap(), None);
                assert!(cache.snapshot().unwrap() == server.durable);
            }
        }
    }

    #[test]
    fn join_reviews_preserve_survivors_and_require_current_child_placement() {
        for empty in [false, true] {
            let (source, sid, left, _) = fixture();
            let split = ParagraphSplit::new(left, if empty { 0 } else { 2 }, "Local").unwrap();
            let source = PreparedEdit::split(&source, sid, &split)
                .unwrap()
                .as_bytes()
                .to_vec();
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("join.sqlite");
            let cache = Replica::create(&path, &source).unwrap();
            let intent = ParagraphJoin::new(left, split.text_object(), "Local").unwrap();
            let id = cache.join(&source, sid, &intent).unwrap().unwrap();
            let survivor = if empty { split.text_object() } else { left };
            let dependent = cache
                .edit_text(&cache.snapshot().unwrap(), sid, survivor, 0..0, "Local ")
                .unwrap()
                .unwrap();
            let local = cache.snapshot().unwrap();
            let child =
                Insertion::paragraph(split.object(), None, "Remote child", "Remote").unwrap();
            let remote = PreparedEdit::insert(&source, sid, &child)
                .unwrap()
                .as_bytes()
                .to_vec();
            let mut server = Server::new(&remote);
            assert_eq!(
                cache.sync_once(&mut server).unwrap(),
                Some((id, EditStatus::Conflict(ConflictKind::StructureChanged)))
            );
            assert!(cache.rebase_join_conflict(id, &source, &remote).is_err());
            assert_eq!(server.publications, 0);
            cache.rebase_join_conflict(id, &local, &remote).unwrap();
            assert!(
                matches!(cache.sync_once(&mut server).unwrap(), Some((actual, EditStatus::Published { .. })) if actual == id)
            );
            assert!(
                matches!(cache.sync_once(&mut server).unwrap(), Some((actual, EditStatus::Published { .. })) if actual == dependent)
            );
            let store = Store::parse(&server.durable).unwrap();
            let index = RevisionIndex::parse(&store).unwrap();
            let doc = Document::parse(&index).unwrap();
            let space = &doc.spaces[&sid];
            let view = &space.revisions[&space.contexts[&ExGuid::default()]];
            let parent = view
                .nodes
                .values()
                .find(|node| node.content == [survivor])
                .unwrap();
            assert_eq!(parent.children, [child.object()]);
            assert!(
                matches!(&view.nodes[&survivor].kind, Kind::RichText { text, .. } if text == "Local ab🦀cd")
            );
        }

        let (source, sid, left, _) = fixture();
        let split = ParagraphSplit::new(left, 0, "Local").unwrap();
        let source = PreparedEdit::split(&source, sid, &split)
            .unwrap()
            .as_bytes()
            .to_vec();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("identity.sqlite");
        let cache = Replica::create(&path, &source).unwrap();
        let intent = ParagraphJoin::new(left, split.text_object(), "Local").unwrap();
        let id = cache.join(&source, sid, &intent).unwrap().unwrap();
        let local = cache.snapshot().unwrap();
        let remote = onestore::replace_text(&source, sid, left, 0..0, "Remote").unwrap();
        let mut server = Server::new(&remote);
        assert_eq!(
            cache.sync_once(&mut server).unwrap(),
            Some((id, EditStatus::Conflict(ConflictKind::TextChanged)))
        );
        assert!(cache.rebase_join_conflict(id, &local, &remote).is_err());
        assert!(cache.snapshot().unwrap() == local);
        assert_eq!(server.publications, 0);
        assert_eq!(cache.pending().unwrap().len(), 1);
    }

    #[test]
    fn overlapping_clients_retain_each_local_branch_without_replaying_removed_targets() {
        let (source, sid, left, _) = fixture();
        let split = ParagraphSplit::new(left, 2, "Seed").unwrap();
        let source = PreparedEdit::split(&source, sid, &split)
            .unwrap()
            .as_bytes()
            .to_vec();
        let directory = tempfile::tempdir().unwrap();
        let first = Replica::create(directory.path().join("first.sqlite"), &source).unwrap();
        let second_path = directory.path().join("second.sqlite");
        let second = Replica::create(&second_path, &source).unwrap();
        let join = ParagraphJoin::new(left, split.text_object(), "First").unwrap();
        first.join(&source, sid, &join).unwrap();
        let right_split = ParagraphSplit::new(split.text_object(), 2, "Second").unwrap();
        let id = second.split(&source, sid, &right_split).unwrap().unwrap();
        second
            .edit_text(
                &second.snapshot().unwrap(),
                sid,
                right_split.text_object(),
                0..0,
                "Second ",
            )
            .unwrap();
        let local = second.snapshot().unwrap();
        let pending = second.pending().unwrap();
        let mut server = Server::new(&source);
        assert!(matches!(
            first.sync_once(&mut server).unwrap().unwrap().1,
            EditStatus::Published { .. }
        ));
        assert_eq!(
            second.sync_once(&mut server).unwrap(),
            Some((id, EditStatus::Conflict(ConflictKind::TargetUnavailable)))
        );
        drop(second);
        let second = Replica::open(&second_path).unwrap();
        assert_eq!(second.pending().unwrap(), pending);
        assert!(second.snapshot().unwrap() == local);
        assert_eq!(server.publications, 1);
        assert_eq!(
            second.status(id).unwrap(),
            Some(EditStatus::Conflict(ConflictKind::TargetUnavailable))
        );
    }

    #[test]
    #[ignore = "exports reconciled paragraph edits for independent native validation"]
    fn export_native_offline_paragraphs() {
        use std::{fs, path::PathBuf};
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/paragraph-edit");
        let output = PathBuf::from(std::env::var_os("ONESTORE_OFFLINE_PARAGRAPH_OUTPUT").unwrap());
        assert!(output.is_absolute());
        fs::create_dir(&output).unwrap();
        for (name, source, manifest, split) in [
            (
                "splits",
                "before/notebook/synthetic.one",
                "rust-split/manifest.json",
                true,
            ),
            (
                "joins",
                "split/notebook/synthetic.one",
                "rust-join/split/manifest.json",
                false,
            ),
            (
                "inheritance",
                "join-edges/before/notebook/synthetic.one",
                "rust-join/inheritance/manifest.json",
                false,
            ),
            (
                "tags",
                "join-tags/before/notebook/synthetic.one",
                "rust-join/tags/manifest.json",
                false,
            ),
        ] {
            let source = fs::read(root.join(source)).unwrap();
            let folder = output.join(name);
            fs::create_dir(&folder).unwrap();
            let path = folder.join("cache.sqlite");
            let mut cache = Replica::create(&path, &source).unwrap();
            let mut server = Server::new(&source);
            let manifest: serde_json::Value =
                serde_json::from_slice(&fs::read(root.join(manifest)).unwrap()).unwrap();
            let cases = if split { &manifest["cases"] } else { &manifest };
            let mut recorded = Vec::new();
            for case in cases
                .as_array()
                .unwrap()
                .iter()
                .filter(|case| case.get("intent").is_some())
            {
                let local = cache.snapshot().unwrap();
                let (text, offset) = if split {
                    serde_json::from_value::<ParagraphSplit>(case["intent"].clone())
                        .unwrap()
                        .position()
                } else {
                    (
                        serde_json::from_value::<ParagraphJoin>(case["intent"].clone())
                            .unwrap()
                            .texts()[0],
                        1,
                    )
                };
                let store = Store::parse(&local).unwrap();
                let index = RevisionIndex::parse(&store).unwrap();
                let doc = Document::parse(&index).unwrap();
                let (sid, space) = doc
                    .spaces
                    .iter()
                    .find(|(_, space)| {
                        space.revisions[&space.contexts[&ExGuid::default()]]
                            .nodes
                            .contains_key(&text)
                    })
                    .unwrap();
                let view = &space.revisions[&space.contexts[&ExGuid::default()]];
                let Kind::RichText { text: content, .. } = &view.nodes[&text].kind else {
                    panic!()
                };
                let remote_prefix = offset > 0 && !content.is_empty();
                assert!(!content.contains('☂'));
                if remote_prefix {
                    server.visible =
                        onestore::replace_text(&server.visible, *sid, text, 0..0, "☂").unwrap();
                    server.durable.clone_from(&server.visible);
                }
                let id = if split {
                    cache.split(
                        &local,
                        *sid,
                        &serde_json::from_value(case["intent"].clone()).unwrap(),
                    )
                } else {
                    cache.join(
                        &local,
                        *sid,
                        &serde_json::from_value(case["intent"].clone()).unwrap(),
                    )
                }
                .unwrap()
                .unwrap();
                recorded.push(serde_json::json!({"case": case["case"], "id": id,
                    "space": sid, "intent": case["intent"], "remote_prefix": remote_prefix}));
                drop(cache);
                cache = Replica::open(&path).unwrap();
            }
            for case in &mut recorded {
                let id = case["id"].as_u64().unwrap();
                if id % 2 == 0 {
                    server.fault = Fault::UnknownAfter;
                    assert!(cache.sync_once(&mut server).is_err());
                    assert!(matches!(
                        cache.status(id).unwrap(),
                        Some(EditStatus::AwaitingConfirmation { .. })
                    ));
                    drop(cache);
                    cache = Replica::open(&path).unwrap();
                }
                let (actual, status) = cache.sync_once(&mut server).unwrap().unwrap();
                assert_eq!(actual, id);
                let EditStatus::Published { revision } = status else {
                    panic!("{name}: {status:?}")
                };
                case["revision"] = serde_json::to_value(revision).unwrap();
            }
            assert_eq!(server.publications, recorded.len());
            assert_eq!(cache.sync_once(&mut server).unwrap(), None);
            assert!(cache.snapshot().unwrap() == server.durable);
            cache
                .export_recovery(folder.join("recovery.sqlite"))
                .unwrap();
            fs::create_dir(folder.join("candidate")).unwrap();
            fs::write(folder.join("candidate/synthetic.one"), &server.durable).unwrap();
            fs::write(
                folder.join("manifest.json"),
                serde_json::to_vec_pretty(&recorded).unwrap(),
            )
            .unwrap();
        }
    }
}

#[derive(Clone, Copy, Default)]
enum Fault {
    #[default]
    None,
    Before,
    UnknownBefore,
    UnknownAfter,
    Committed,
    PanicBefore,
    PanicAfter,
    Confirm,
    ConfirmCommitted,
}

struct Server {
    visible: Vec<u8>,
    durable: Vec<u8>,
    fault: Fault,
    publications: usize,
    confirmations: usize,
}

impl Server {
    fn new(source: &[u8]) -> Self {
        Self {
            visible: source.to_vec(),
            durable: source.to_vec(),
            fault: Fault::None,
            publications: 0,
            confirmations: 0,
        }
    }
}

fn failure(state: CommitState) -> CommitError {
    CommitError {
        state,
        error: io::Error::from(io::ErrorKind::ConnectionAborted),
    }
}

impl CommitIo for Server {
    fn read_at(&mut self, offset: u64, output: &mut [u8]) -> io::Result<usize> {
        let offset = usize::try_from(offset).unwrap();
        let size = output.len().min(self.visible.len().saturating_sub(offset));
        if size > 0 {
            output[..size].copy_from_slice(&self.visible[offset..offset + size]);
        }
        Ok(size)
    }
    fn write_at(&mut self, offset: u64, bytes: &[u8]) -> io::Result<usize> {
        let offset = usize::try_from(offset).unwrap();
        self.visible
            .resize(self.visible.len().max(offset + bytes.len()), 0);
        self.visible[offset..offset + bytes.len()].copy_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        self.durable.clone_from(&self.visible);
        Ok(())
    }
}

impl Remote for Server {
    fn read(&mut self) -> io::Result<Vec<u8>> {
        Ok(self.visible.clone())
    }
    fn publish(&mut self, edit: &PreparedEdit<'_>) -> Result<(), CommitError> {
        self.publications += 1;
        let fault = std::mem::take(&mut self.fault);
        match fault {
            Fault::Before => return Err(failure(CommitState::NotCommitted)),
            Fault::UnknownBefore => return Err(failure(CommitState::Unknown)),
            Fault::PanicBefore => panic!("Terminated before remote I/O"),
            _ => {}
        }
        let old = self.durable.clone();
        edit.commit(self)?;
        match fault {
            Fault::UnknownAfter => {
                self.durable = old;
                Err(failure(CommitState::Unknown))
            }
            Fault::Committed => Err(failure(CommitState::Committed)),
            Fault::PanicAfter => panic!("Terminated after remote publication"),
            _ => Ok(()),
        }
    }
    fn confirm(&mut self, snapshot: &[u8]) -> Result<(), CommitError> {
        self.confirmations += 1;
        if matches!(self.fault, Fault::Confirm) {
            self.fault = Fault::None;
            return Err(failure(CommitState::Unknown));
        }
        onestore::confirm_snapshot(self, snapshot)?;
        if matches!(self.fault, Fault::ConfirmCommitted) {
            self.fault = Fault::None;
            return Err(failure(CommitState::Committed));
        }
        Ok(())
    }
}

fn text(source: &[u8]) -> (ExGuid, ExGuid, String) {
    let store = Store::parse(source).unwrap();
    assert!(store.checksum_mismatches.is_empty());
    let index = RevisionIndex::parse(&store).unwrap();
    index.validate_current().unwrap();
    let doc = Document::parse(&index).unwrap();
    doc.spaces
        .iter()
        .find_map(|(sid, space)| {
            space.revisions[&space.contexts[&ExGuid::default()]]
                .nodes
                .iter()
                .find_map(|(oid, node)| match &node.kind {
                    Kind::RichText { text, .. } => Some((*sid, *oid, text.clone())),
                    _ => None,
                })
        })
        .unwrap()
}

#[test]
fn recovery_archive_preserves_typed_queue_uncertainty_and_receipts_without_becoming_a_writer() {
    use onestore::{Insertion, TextAttribute};
    use onestore_offline::{Recovery, RecoverySummary};

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("live.sqlite");
    let archive_path = directory.path().join("recovery.sqlite");
    let source = onestore::create_section("recovery.one", "Original", "Fixture").unwrap();
    let (space, object, _) = text(&source);
    let cache = Replica::create(&path, &source).unwrap();
    let published = cache
        .edit_text(&source, space, object, 0..0, "Published ")
        .unwrap()
        .unwrap();
    let mut server = Server::new(&source);
    let (_, receipt) = cache.sync_once(&mut server).unwrap().unwrap();
    let source = cache.snapshot().unwrap();
    let store = Store::parse(&source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let (page_space, page) = document.pages().unwrap()[0];
    let outline = Insertion::outline(page, 100.0, 200.0, "Outline", "Fixture").unwrap();
    let inserted = cache
        .insert(&source, page_space, &outline)
        .unwrap()
        .unwrap();
    let paragraph = Insertion::paragraph(outline.object(), None, "Recovery 🦀", "Fixture").unwrap();
    cache
        .insert(&cache.snapshot().unwrap(), page_space, &paragraph)
        .unwrap();
    cache
        .format(
            &cache.snapshot().unwrap(),
            page_space,
            paragraph.text_object(),
            0..3,
            &[TextAttribute::Bold(true)],
        )
        .unwrap();
    cache
        .edit_text(
            &cache.snapshot().unwrap(),
            page_space,
            paragraph.text_object(),
            0..0,
            "Pending ",
        )
        .unwrap();
    server.fault = Fault::UnknownBefore;
    assert!(matches!(
        cache.sync_once(&mut server),
        Err(Error::Remote(CommitError {
            state: CommitState::Unknown,
            ..
        }))
    ));
    let working = cache.snapshot().unwrap();
    let remote = cache.remote_snapshot().unwrap();
    let pending = cache.pending().unwrap();
    let uncertain = cache.status(inserted).unwrap();
    let source_file = std::fs::read(&path).unwrap();
    let summary = RecoverySummary {
        queued_edits: 4,
        conflicts: 0,
        uncertain_edits: 1,
        published_receipts: 1,
        working_bytes: working.len() as u64,
        remote_bytes: remote.len() as u64,
        cached_assets: 0,
        cached_asset_bytes: 0,
    };
    assert_eq!(cache.recovery_summary().unwrap(), summary);
    cache.export_recovery(&archive_path).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), source_file);
    let archive_bytes = std::fs::read(&archive_path).unwrap();
    assert!(Replica::open(&archive_path).is_err());
    assert_eq!(std::fs::read(&archive_path).unwrap(), archive_bytes);
    assert!(Recovery::open(&path).is_err());
    let archive = Recovery::open(&archive_path).unwrap();
    assert_eq!(archive.summary().unwrap(), summary);
    assert_eq!(archive.snapshot().unwrap(), working);
    assert_eq!(archive.remote_snapshot().unwrap(), remote);
    assert_eq!(archive.pending().unwrap(), pending);
    assert_eq!(archive.status(published).unwrap(), Some(receipt));
    assert_eq!(archive.status(inserted).unwrap(), uncertain);
    for edit in &pending {
        assert_eq!(
            archive.status(edit.id).unwrap(),
            cache.status(edit.id).unwrap()
        );
    }
    let EditStatus::Published { revision } = receipt else {
        panic!()
    };
    assert_eq!(archive.receipts().unwrap(), [(published, revision)].into());
    assert_eq!(
        archive.status(u64::MAX - 1).unwrap_err().to_string(),
        cache.status(u64::MAX - 1).unwrap_err().to_string()
    );
    for existing in [&path, &archive_path] {
        assert!(
            matches!(cache.export_recovery(existing), Err(Error::Io(error)) if error.kind() == io::ErrorKind::AlreadyExists)
        );
    }
    assert_eq!(std::fs::read(&path).unwrap(), source_file);
    assert_eq!(std::fs::read(&archive_path).unwrap(), archive_bytes);
    cache
        .edit_text(&working, space, object, 0..0, "Later ")
        .unwrap();
    assert_eq!(archive.snapshot().unwrap(), working);
    assert_eq!(archive.pending().unwrap(), pending);
    drop(archive);
    assert_eq!(
        Recovery::open(&archive_path).unwrap().summary().unwrap(),
        summary
    );
    assert_eq!(std::fs::read(&archive_path).unwrap(), archive_bytes);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&archive_path)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    let remaining: Vec<_> = std::fs::read_dir(directory.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert!(
        remaining
            .iter()
            .all(|name| !name.to_string_lossy().starts_with(".onestore-recovery-")),
        "Temporary recovery files remain: {remaining:?}"
    );
}

#[test]
fn recovery_archive_retains_conflict_images_and_rejects_foreign_or_future_archives() {
    use onestore_offline::Recovery;

    let directory = tempfile::tempdir().unwrap();
    let source = onestore::create_section("recovery.one", "Original", "Fixture").unwrap();
    let (space, object, _) = text(&source);
    let cache = Replica::create(directory.path().join("live.sqlite"), &source).unwrap();
    let id = cache
        .edit_text(&source, space, object, 0..8, "Local")
        .unwrap()
        .unwrap();
    let changed = onestore::replace_text(&source, space, object, 0..8, "Remote").unwrap();
    let mut server = Server::new(&changed);
    let outcome = cache.sync_once(&mut server).unwrap().unwrap();
    assert_eq!(
        outcome,
        (id, EditStatus::Conflict(ConflictKind::TextChanged))
    );
    let path = directory.path().join("conflict.sqlite");
    cache.export_recovery(&path).unwrap();
    let archive = Recovery::open(&path).unwrap();
    assert_eq!(text(&archive.snapshot().unwrap()).2, "Local");
    assert_eq!(archive.remote_snapshot().unwrap(), changed);
    assert_eq!(archive.status(id).unwrap(), Some(outcome.1));
    assert_eq!(archive.summary().unwrap().conflicts, 1);
    assert_eq!(archive.summary().unwrap().uncertain_edits, 0);
    drop(archive);
    for sql in [
        "PRAGMA user_version=99",
        "PRAGMA user_version=4; PRAGMA application_id=0",
    ] {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection.execute_batch(sql).unwrap();
        drop(connection);
        let before = std::fs::read(&path).unwrap();
        assert!(Recovery::open(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
    assert_eq!(cache.status(id).unwrap(), Some(outcome.1));
    assert_eq!(text(&cache.snapshot().unwrap()).2, "Local");
}

#[test]
fn rebases_multiple_disjoint_remote_changes_and_persists_the_remote_receipt() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.sqlite");
    let source = onestore::create_section("sync.one", "ab🦀cd", "Fixture").unwrap();
    let (sid, oid, _) = text(&source);
    let cache = Replica::create(&path, &source).unwrap();
    let id = cache
        .edit_text(&source, sid, oid, 2..4, "🐈")
        .unwrap()
        .unwrap();
    let remote = onestore::replace_text(&source, sid, oid, 0..6, "Xab🦀cYd").unwrap();
    let mut server = Server::new(&remote);
    let outcome = cache.sync_once(&mut server).unwrap().unwrap();
    assert_eq!(outcome.0, id);
    assert!(matches!(outcome.1, EditStatus::Published { .. }));
    assert_eq!(text(&server.durable).2, "Xab🐈cYd");
    assert_eq!(cache.snapshot().unwrap(), server.durable);
    assert!(cache.pending().unwrap().is_empty());
    drop(cache);
    let cache = Replica::open(&path).unwrap();
    assert_eq!(cache.status(id).unwrap(), Some(outcome.1));
    assert_eq!(cache.sync_once(&mut server).unwrap(), None);
    assert_eq!(server.publications, 1);
    assert_eq!(cache.status(id + 1).unwrap(), None);
    let source = cache.snapshot().unwrap();
    let next = cache
        .edit_text(&source, sid, oid, 0..0, "Later ")
        .unwrap()
        .unwrap();
    assert!(next > id);
}

#[test]
fn overlapping_changes_preserve_both_images_and_survive_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.sqlite");
    let source = onestore::create_section("sync.one", "abc", "Fixture").unwrap();
    let (sid, oid, _) = text(&source);
    let cache = Replica::create(&path, &source).unwrap();
    let id = cache
        .edit_text(&source, sid, oid, 1..2, "L")
        .unwrap()
        .unwrap();
    let local = cache.snapshot().unwrap();
    let remote = onestore::replace_text(&source, sid, oid, 1..2, "R").unwrap();
    let mut server = Server::new(&remote);
    assert_eq!(
        cache.sync_once(&mut server).unwrap(),
        Some((id, EditStatus::Conflict(ConflictKind::TextChanged)))
    );
    assert_eq!(server.publications, 0);
    drop(cache);
    let cache = Replica::open(&path).unwrap();
    assert_eq!(cache.snapshot().unwrap(), local);
    assert_eq!(cache.remote_snapshot().unwrap(), remote);
    assert_eq!(cache.pending().unwrap().len(), 1);
    assert_eq!(
        cache.status(id).unwrap(),
        Some(EditStatus::Conflict(ConflictKind::TextChanged))
    );
}

#[test]
fn lost_replies_and_process_termination_never_blindly_replay_an_attempt() {
    for fault in [
        Fault::UnknownBefore,
        Fault::UnknownAfter,
        Fault::PanicBefore,
        Fault::PanicAfter,
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.sqlite");
        let source = onestore::create_section("sync.one", "abc", "Fixture").unwrap();
        let (sid, oid, _) = text(&source);
        let cache = Replica::create(&path, &source).unwrap();
        let id = cache
            .edit_text(&source, sid, oid, 0..0, "Once ")
            .unwrap()
            .unwrap();
        let mut server = Server::new(&source);
        server.fault = fault;
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            cache.sync_once(&mut server)
        }));
        let attempted = cache.status(id).unwrap().unwrap();
        assert!(matches!(attempted, EditStatus::AwaitingConfirmation { .. }));
        drop(cache);
        let cache = Replica::open(&path).unwrap();
        let result = cache.sync_once(&mut server).unwrap().unwrap();
        if matches!(fault, Fault::UnknownAfter | Fault::PanicAfter) {
            assert!(matches!(result.1, EditStatus::Published { .. }));
            assert_eq!(text(&server.durable).2, "Once abc");
            assert_eq!(server.confirmations, 1);
            assert!(cache.pending().unwrap().is_empty());
        } else {
            assert_eq!(result.1, attempted);
            assert_eq!(cache.pending().unwrap().len(), 1);
            assert_eq!(server.confirmations, 0);
            assert_eq!(server.durable, source);
        }
        assert_eq!(server.publications, 1);
    }
}

#[test]
fn failed_confirmation_does_not_promote_visible_bytes_to_a_receipt() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.sqlite");
    let source = onestore::create_section("sync.one", "abc", "Fixture").unwrap();
    let (sid, oid, _) = text(&source);
    let cache = Replica::create(&path, &source).unwrap();
    let id = cache
        .edit_text(&source, sid, oid, 0..0, "Once ")
        .unwrap()
        .unwrap();
    let mut server = Server::new(&source);
    server.fault = Fault::UnknownAfter;
    assert!(cache.sync_once(&mut server).is_err());
    server.fault = Fault::Confirm;
    assert!(cache.sync_once(&mut server).is_err());
    assert_eq!(server.durable, source);
    assert!(matches!(
        cache.status(id).unwrap(),
        Some(EditStatus::AwaitingConfirmation { .. })
    ));
    assert!(matches!(
        cache.sync_once(&mut server).unwrap(),
        Some((_, EditStatus::Published { .. }))
    ));
    assert_eq!(server.publications, 1);
    assert_eq!(server.confirmations, 2);
    assert_eq!(server.visible, server.durable);
}

#[test]
fn confirmation_cleanup_failure_still_records_a_durable_receipt() {
    let dir = tempfile::tempdir().unwrap();
    let source = onestore::create_section("sync.one", "abc", "Fixture").unwrap();
    let (sid, oid, _) = text(&source);
    let cache = Replica::create(dir.path().join("cache.sqlite"), &source).unwrap();
    let id = cache
        .edit_text(&source, sid, oid, 0..0, "Once ")
        .unwrap()
        .unwrap();
    let mut server = Server::new(&source);
    server.fault = Fault::UnknownAfter;
    assert!(cache.sync_once(&mut server).is_err());
    server.fault = Fault::ConfirmCommitted;
    assert!(
        matches!(cache.sync_once(&mut server), Err(Error::Remote(error)) if error.state == CommitState::Committed)
    );
    assert!(matches!(
        cache.status(id).unwrap(),
        Some(EditStatus::Published { .. })
    ));
    assert!(cache.pending().unwrap().is_empty());
    assert_eq!(server.visible, server.durable);
    assert_eq!(server.publications, 1);
    assert_eq!(server.confirmations, 1);
}

#[test]
fn proven_unpublished_attempts_retry_and_committed_cleanup_errors_keep_receipts() {
    for fault in [Fault::Before, Fault::Committed] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.sqlite");
        let source = onestore::create_section("sync.one", "abc", "Fixture").unwrap();
        let (sid, oid, _) = text(&source);
        let cache = Replica::create(&path, &source).unwrap();
        let id = cache
            .edit_text(&source, sid, oid, 0..0, "Once ")
            .unwrap()
            .unwrap();
        let mut server = Server::new(&source);
        server.fault = fault;
        assert!(matches!(
            cache.sync_once(&mut server),
            Err(Error::Remote(_))
        ));
        if matches!(fault, Fault::Before) {
            assert_eq!(cache.status(id).unwrap(), Some(EditStatus::Pending));
            assert!(matches!(
                cache.sync_once(&mut server).unwrap(),
                Some((_, EditStatus::Published { .. }))
            ));
            assert_eq!(server.publications, 2);
        } else {
            assert!(matches!(
                cache.status(id).unwrap(),
                Some(EditStatus::Published { .. })
            ));
            assert_eq!(cache.sync_once(&mut server).unwrap(), None);
            assert_eq!(server.publications, 1);
        }
        assert_eq!(text(&server.durable).2, "Once abc");
    }
}

#[test]
fn database_failures_before_and_after_publication_preserve_recovery_state() {
    for table in ["attempt", "receipts"] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.sqlite");
        let source = onestore::create_section("sync.one", "abc", "Fixture").unwrap();
        let (sid, oid, _) = text(&source);
        let cache = Replica::create(&path, &source).unwrap();
        let id = cache
            .edit_text(&source, sid, oid, 0..0, "Once ")
            .unwrap()
            .unwrap();
        drop(cache);
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch(&format!("CREATE TRIGGER interrupted BEFORE INSERT ON {table} BEGIN SELECT RAISE(ABORT,'Injected cache failure'); END;")).unwrap();
        drop(db);
        let cache = Replica::open(&path).unwrap();
        let mut server = Server::new(&source);
        assert!(matches!(
            cache.sync_once(&mut server),
            Err(Error::Database(_))
        ));
        assert_eq!(server.publications, usize::from(table == "receipts"));
        assert_eq!(cache.pending().unwrap().len(), 1);
        drop(cache);
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch("DROP TRIGGER interrupted").unwrap();
        drop(db);
        let cache = Replica::open(&path).unwrap();
        assert!(matches!(
            cache.sync_once(&mut server).unwrap(),
            Some((_, EditStatus::Published { .. }))
        ));
        assert!(matches!(
            cache.status(id).unwrap(),
            Some(EditStatus::Published { .. })
        ));
        assert_eq!(server.publications, 1);
        assert_eq!(server.confirmations, usize::from(table == "receipts"));
        assert_eq!(text(&server.durable).2, "Once abc");
    }
}

#[test]
fn twelve_local_editors_progress_during_remote_reads_publication_and_confirmation() {
    struct Paused {
        server: Server,
        phase: &'static str,
        entered: std::sync::mpsc::Sender<()>,
        resume: std::sync::mpsc::Receiver<()>,
    }
    impl Paused {
        fn wait(&self, phase: &str) {
            if self.phase == phase {
                self.entered.send(()).unwrap();
                self.resume
                    .recv_timeout(std::time::Duration::from_secs(5))
                    .unwrap();
            }
        }
    }
    impl Remote for Paused {
        fn read(&mut self) -> io::Result<Vec<u8>> {
            self.wait("read");
            self.server.read()
        }
        fn publish(&mut self, edit: &PreparedEdit<'_>) -> Result<(), CommitError> {
            self.wait("publish");
            self.server.publish(edit)
        }
        fn confirm(&mut self, source: &[u8]) -> Result<(), CommitError> {
            self.wait("confirm");
            self.server.confirm(source)
        }
    }
    for phase in ["read", "publish", "confirm"] {
        let dir = tempfile::tempdir().unwrap();
        let source = onestore::create_section("sync.one", "abc", "Fixture").unwrap();
        let (sid, oid, _) = text(&source);
        let cache = Replica::create(dir.path().join("cache.sqlite"), &source).unwrap();
        let first = cache
            .edit_text(&source, sid, oid, 0..0, "First ")
            .unwrap()
            .unwrap();
        let mut server = Server::new(&source);
        if phase == "confirm" {
            server.fault = Fault::UnknownAfter;
            assert!(cache.sync_once(&mut server).is_err());
        }
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (resume_tx, resume_rx) = std::sync::mpsc::channel();
        let mut paused = Paused {
            server,
            phase,
            entered: entered_tx,
            resume: resume_rx,
        };
        let mut server = std::thread::scope(|scope| {
            let running = scope.spawn(|| {
                let result = cache.sync_once(&mut paused);
                assert!(
                    matches!(result, Ok(Some((id, EditStatus::Published { .. }))) if id == first)
                );
                paused.server
            });
            entered_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            assert!(
                matches!(cache.sync_once(&mut Server::new(&source)),Err(Error::Io(error)) if error.kind()==io::ErrorKind::WouldBlock)
            );
            assert!(
                matches!(cache.rebase_conflict(first, &[], &[], 0..0), Err(Error::Io(error)) if error.kind() == io::ErrorKind::WouldBlock)
            );
            let started = std::time::Instant::now();
            let handles: Vec<_> = (0..12)
                .map(|writer| {
                    let cache = &cache;
                    scope.spawn(move || {
                        loop {
                            let snapshot = cache.snapshot().unwrap();
                            match cache.edit_text(
                                &snapshot,
                                sid,
                                oid,
                                0..0,
                                &format!("[{writer}] "),
                            ) {
                                Ok(Some(id)) => break id,
                                Err(Error::Io(error))
                                    if error.kind() == io::ErrorKind::ResourceBusy => {}
                                other => panic!("Unexpected local outcome {other:?}"),
                            }
                        }
                    })
                })
                .collect();
            let ids: std::collections::BTreeSet<_> = handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .collect();
            assert_eq!(ids.len(), 12);
            assert!(
                started.elapsed() < std::time::Duration::from_secs(2),
                "Local edits waited for remote {phase}"
            );
            resume_tx.send(()).unwrap();
            running.join().unwrap()
        });
        assert_eq!(cache.pending().unwrap().len(), 12);
        let expected = text(&cache.snapshot().unwrap()).2;
        for _ in 0..12 {
            assert!(matches!(
                cache.sync_once(&mut server).unwrap(),
                Some((_, EditStatus::Published { .. }))
            ));
        }
        assert!(cache.pending().unwrap().is_empty());
        assert_eq!(server.publications, 13);
        assert_eq!(text(&server.durable).2, expected);
        assert_eq!(cache.snapshot().unwrap(), server.durable);
    }
}

#[test]
fn unrelated_remote_files_never_replace_a_local_cache() {
    let dir = tempfile::tempdir().unwrap();
    let source = onestore::create_section("sync.one", "abc", "Fixture").unwrap();
    let (sid, oid, _) = text(&source);
    let cache = Replica::create(dir.path().join("cache.sqlite"), &source).unwrap();
    let other = onestore::create_section("other.one", "abc", "Fixture").unwrap();
    let mut server = Server::new(&other);
    for pending in [false, true] {
        if pending {
            cache.edit_text(&source, sid, oid, 0..0, "Local ").unwrap();
        }
        let before = cache.snapshot().unwrap();
        assert!(
            matches!(cache.sync_once(&mut server),Err(Error::Io(error)) if error.kind()==io::ErrorKind::InvalidInput)
        );
        assert_eq!(cache.snapshot().unwrap(), before);
        assert_eq!(cache.remote_snapshot().unwrap(), source);
        assert_eq!(server.publications, 0);
    }
}

#[test]
fn version_one_cache_migration_preserves_images_intents_and_local_ids() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.sqlite");
    let source = onestore::create_section("sync.one", "abc", "Fixture").unwrap();
    let (sid, oid, _) = text(&source);
    let cache = Replica::create(&path, &source).unwrap();
    let id = cache
        .edit_text(&source, sid, oid, 0..0, "Local ")
        .unwrap()
        .unwrap();
    let snapshot = cache.snapshot().unwrap();
    let pending = cache.pending().unwrap();
    drop(cache);
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch(
        "DROP TABLE attempt; DROP TABLE conflicts; DROP TABLE receipts; DROP TABLE edits; DROP TABLE assets;
         CREATE TABLE edits (
            id INTEGER PRIMARY KEY AUTOINCREMENT CHECK(id>0), space TEXT NOT NULL,
            object TEXT NOT NULL, before_text TEXT NOT NULL,
            start INTEGER NOT NULL CHECK(start BETWEEN 0 AND 4294967295),
            end INTEGER NOT NULL CHECK(end BETWEEN start AND 4294967295), replacement TEXT NOT NULL
         ) STRICT; PRAGMA user_version=1;",
    )
    .unwrap();
    db.execute("INSERT INTO edits(id,space,object,before_text,start,end,replacement) VALUES (?1,?2,?3,'abc',0,0,'Local ')",rusqlite::params![i64::try_from(id).unwrap(),sid.to_string(),oid.to_string()]).unwrap();
    drop(db);
    let cache = Replica::open(&path).unwrap();
    assert_eq!(cache.snapshot().unwrap(), snapshot);
    assert_eq!(cache.pending().unwrap(), pending);
    assert_eq!(cache.status(id).unwrap(), Some(EditStatus::Pending));
    let mut server = Server::new(&source);
    assert!(matches!(
        cache.sync_once(&mut server).unwrap(),
        Some((_, EditStatus::Published { .. }))
    ));
    drop(cache);
    let db = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(
        db.pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
            .unwrap(),
        6
    );
}

#[test]
fn reviewed_conflict_rebase_preserves_twelve_dependent_edits_and_survives_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.sqlite");
    let source = onestore::create_section("resolve.one", "abc", "Fixture").unwrap();
    let (sid, oid, _) = text(&source);
    let cache = Replica::create(&path, &source).unwrap();
    let first = cache
        .edit_text(&source, sid, oid, 1..2, "L")
        .unwrap()
        .unwrap();
    for n in 0..12 {
        cache
            .edit_text(
                &cache.snapshot().unwrap(),
                sid,
                oid,
                0..0,
                &format!("[{n}] "),
            )
            .unwrap();
    }
    let local = cache.snapshot().unwrap();
    let pending = cache.pending().unwrap();
    let remote = onestore::replace_text(&source, sid, oid, 0..3, "aRcZ").unwrap();
    let mut server = Server::new(&remote);
    assert_eq!(
        cache.sync_once(&mut server).unwrap(),
        Some((first, EditStatus::Conflict(ConflictKind::TextChanged)))
    );
    cache.rebase_conflict(first, &local, &remote, 1..2).unwrap();
    assert_eq!(cache.snapshot().unwrap(), local);
    assert_eq!(cache.remote_snapshot().unwrap(), remote);
    let rebased = cache.pending().unwrap();
    assert_eq!(&rebased[1..], &pending[1..]);
    assert_eq!(rebased[0].id, first);
    let onestore_offline::Operation::Text(updated) = &rebased[0].operation else {
        panic!()
    };
    let onestore_offline::Operation::Text(previous) = &pending[0].operation else {
        panic!()
    };
    assert_eq!(updated.replacement, previous.replacement);
    assert_eq!(updated.before, "aRcZ");
    assert_eq!(cache.status(first).unwrap(), Some(EditStatus::Pending));
    drop(cache);
    let cache = Replica::open(&path).unwrap();
    assert_eq!(cache.pending().unwrap(), rebased);
    assert_eq!(cache.snapshot().unwrap(), local);
    for intent in &pending {
        assert!(
            matches!(cache.sync_once(&mut server).unwrap(), Some((id, EditStatus::Published { .. })) if id == intent.id)
        );
    }
    assert_eq!(server.publications, 13);
    assert_eq!(text(&server.durable).2, text(&local).2 + "Z");
    assert_eq!(cache.snapshot().unwrap(), server.durable);
    assert!(cache.pending().unwrap().is_empty());
}

#[test]
fn conflict_review_rejects_stale_images_invalid_ranges_and_nonconflicting_states() {
    let dir = tempfile::tempdir().unwrap();
    let source = onestore::create_section("resolve.one", "abc", "Fixture").unwrap();
    let (sid, oid, _) = text(&source);
    let cache = Replica::create(dir.path().join("cache.sqlite"), &source).unwrap();
    let id = cache
        .edit_text(&source, sid, oid, 1..2, "L")
        .unwrap()
        .unwrap();
    let local = cache.snapshot().unwrap();
    assert!(
        matches!(cache.rebase_conflict(id, &local, &source, 1..2), Err(Error::Io(error)) if error.kind() == io::ErrorKind::InvalidInput)
    );
    let remote = onestore::replace_text(&source, sid, oid, 0..3, "🦀Rc").unwrap();
    let mut server = Server::new(&remote);
    assert!(matches!(
        cache.sync_once(&mut server).unwrap(),
        Some((_, EditStatus::Conflict(_)))
    ));
    let pending = cache.pending().unwrap();
    for range in [1..2, 100..101] {
        assert!(matches!(
            cache.rebase_conflict(id, &local, &remote, range),
            Err(Error::Document(_))
        ));
        assert_eq!(cache.pending().unwrap(), pending);
    }
    assert!(
        matches!(cache.rebase_conflict(id + 1, &local, &remote, 2..3), Err(Error::Io(error)) if error.kind() == io::ErrorKind::InvalidInput)
    );
    cache.edit_text(&local, sid, oid, 0..0, "Later ").unwrap();
    let changed = cache.snapshot().unwrap();
    assert!(
        matches!(cache.rebase_conflict(id, &local, &remote, 2..3), Err(Error::Io(error)) if error.kind() == io::ErrorKind::ResourceBusy)
    );
    let new_remote = onestore::replace_text(&remote, sid, oid, 2..3, "Q").unwrap();
    server.visible = new_remote.clone();
    server.durable = new_remote;
    assert!(matches!(
        cache.sync_once(&mut server).unwrap(),
        Some((_, EditStatus::Conflict(_)))
    ));
    assert!(
        matches!(cache.rebase_conflict(id, &changed, &remote, 2..3), Err(Error::Io(error)) if error.kind() == io::ErrorKind::ResourceBusy)
    );
    assert_eq!(cache.snapshot().unwrap(), changed);
    assert_eq!(cache.pending().unwrap()[0], pending[0]);
    assert_eq!(server.publications, 0);

    let current = cache.remote_snapshot().unwrap();
    cache.rebase_conflict(id, &changed, &current, 2..3).unwrap();
    server.fault = Fault::UnknownBefore;
    assert!(
        matches!(cache.sync_once(&mut server), Err(Error::Remote(error)) if error.state == CommitState::Unknown)
    );
    let attempted = cache.status(id).unwrap();
    let pending = cache.pending().unwrap();
    assert!(
        matches!(cache.rebase_conflict(id, &changed, &current, 2..3), Err(Error::Io(error)) if error.kind() == io::ErrorKind::InvalidInput)
    );
    assert_eq!(cache.status(id).unwrap(), attempted);
    assert_eq!(cache.pending().unwrap(), pending);
    assert_eq!(server.publications, 1);
}

#[test]
fn failure_between_rebase_and_conflict_clear_rolls_back_the_entire_resolution() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.sqlite");
    let source = onestore::create_section("resolve.one", "abc", "Fixture").unwrap();
    let (sid, oid, _) = text(&source);
    let cache = Replica::create(&path, &source).unwrap();
    let id = cache
        .edit_text(&source, sid, oid, 1..2, "L")
        .unwrap()
        .unwrap();
    let remote = onestore::replace_text(&source, sid, oid, 0..3, "XaRc").unwrap();
    let mut server = Server::new(&remote);
    cache.sync_once(&mut server).unwrap();
    let local = cache.snapshot().unwrap();
    let pending = cache.pending().unwrap();
    let status = cache.status(id).unwrap();
    drop(cache);
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection.execute_batch("CREATE TRIGGER fail_clear BEFORE DELETE ON conflicts BEGIN SELECT RAISE(ABORT, 'test conflict clear failure'); END;").unwrap();
    drop(connection);
    let cache = Replica::open(&path).unwrap();
    assert!(matches!(
        cache.rebase_conflict(id, &local, &remote, 2..3),
        Err(Error::Database(_))
    ));
    drop(cache);
    let cache = Replica::open(&path).unwrap();
    assert_eq!(cache.pending().unwrap(), pending);
    assert_eq!(cache.status(id).unwrap(), status);
    assert_eq!(cache.snapshot().unwrap(), local);
    assert_eq!(cache.remote_snapshot().unwrap(), remote);
    assert_eq!(server.publications, 0);
}

#[test]
fn seeded_reviewed_ranges_preserve_unicode_and_edits_inside_the_original_replacement() {
    let dir = tempfile::tempdir().unwrap();
    let original: Vec<_> = "abcdefghij🦀klmnop".chars().collect();
    let mut seed = 911_u64;
    for case in 0..64 {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let at = seed as usize % original.len();
        let prefix = "[".repeat((seed >> 8) as usize % 5);
        let suffix = "]".repeat((seed >> 16) as usize % 5);
        let start: u32 = original[..at].iter().map(|ch| ch.len_utf16() as u32).sum();
        let end = start + original[at].len_utf16() as u32;
        let source = onestore::create_section(
            "resolve.one",
            &original.iter().collect::<String>(),
            "Fixture",
        )
        .unwrap();
        let (sid, oid, _) = text(&source);
        let cache = Replica::create(dir.path().join(format!("{case}.sqlite")), &source).unwrap();
        let id = cache
            .edit_text(&source, sid, oid, start..end, "λ🦊μ")
            .unwrap()
            .unwrap();
        let dependent = cache
            .edit_text(
                &cache.snapshot().unwrap(),
                sid,
                oid,
                start + 1..start + 3,
                "🐕",
            )
            .unwrap()
            .unwrap();
        let mut remote_text = original.clone();
        remote_text.splice(at..at + 1, "Ω🐈π".chars());
        let remote_text = prefix.clone() + &remote_text.iter().collect::<String>() + &suffix;
        let remote = onestore::replace_text(
            &source,
            sid,
            oid,
            0..original.iter().map(|ch| ch.len_utf16() as u32).sum(),
            &remote_text,
        )
        .unwrap();
        let mut server = Server::new(&remote);
        assert!(
            matches!(
                cache.sync_once(&mut server).unwrap(),
                Some((_, EditStatus::Conflict(_)))
            ),
            "case {case}, seed {seed}"
        );
        let at_remote = start + prefix.len() as u32;
        cache
            .rebase_conflict(
                id,
                &cache.snapshot().unwrap(),
                &remote,
                at_remote..at_remote + 4,
            )
            .unwrap();
        for expected in [id, dependent] {
            assert!(
                matches!(cache.sync_once(&mut server).unwrap(), Some((actual, EditStatus::Published { .. })) if actual == expected),
                "case {case}, seed {seed}"
            );
        }
        let mut expected = original.clone();
        expected.splice(at..at + 1, "λ🐕μ".chars());
        let expected = prefix + &expected.iter().collect::<String>() + &suffix;
        assert_eq!(
            text(&server.durable).2,
            expected,
            "case {case}, seed {seed}"
        );
        assert_eq!(server.publications, 2);
        assert!(cache.pending().unwrap().is_empty());
    }
}

#[test]
fn remote_changes_after_review_cannot_be_overwritten_by_the_reviewed_placement() {
    struct ChangedAfterRead {
        server: Server,
        change: Option<Vec<u8>>,
    }
    impl Remote for ChangedAfterRead {
        fn read(&mut self) -> io::Result<Vec<u8>> {
            let snapshot = self.server.read()?;
            if let Some(changed) = self.change.take() {
                self.server.visible = changed.clone();
                self.server.durable = changed;
            }
            Ok(snapshot)
        }
        fn publish(&mut self, edit: &PreparedEdit<'_>) -> Result<(), CommitError> {
            self.server.publish(edit)
        }
        fn confirm(&mut self, snapshot: &[u8]) -> Result<(), CommitError> {
            self.server.confirm(snapshot)
        }
    }
    for after_read in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let source = onestore::create_section("resolve.one", "abc", "Fixture").unwrap();
        let (sid, oid, _) = text(&source);
        let cache = Replica::create(dir.path().join("cache.sqlite"), &source).unwrap();
        let id = cache
            .edit_text(&source, sid, oid, 1..2, "L")
            .unwrap()
            .unwrap();
        let local = cache.snapshot().unwrap();
        let remote = onestore::replace_text(&source, sid, oid, 1..2, "R").unwrap();
        let mut remote = ChangedAfterRead {
            server: Server::new(&remote),
            change: None,
        };
        assert!(matches!(
            cache.sync_once(&mut remote).unwrap(),
            Some((_, EditStatus::Conflict(_)))
        ));
        cache
            .rebase_conflict(id, &local, &cache.remote_snapshot().unwrap(), 1..2)
            .unwrap();
        let changed = onestore::replace_text(&remote.server.visible, sid, oid, 1..2, "Q").unwrap();
        if after_read {
            remote.change = Some(changed.clone());
        } else {
            remote.server.visible = changed.clone();
            remote.server.durable = changed.clone();
        }
        if after_read {
            assert!(
                matches!(cache.sync_once(&mut remote), Err(Error::Remote(error)) if error.state == CommitState::NotCommitted)
            );
            assert_eq!(cache.status(id).unwrap(), Some(EditStatus::Pending));
        }
        assert_eq!(
            cache.sync_once(&mut remote).unwrap(),
            Some((id, EditStatus::Conflict(ConflictKind::TextChanged)))
        );
        assert_eq!(remote.server.durable, changed);
        assert_eq!(remote.server.visible, changed);
        assert_eq!(remote.server.publications, usize::from(after_read));
        assert_eq!(cache.snapshot().unwrap(), local);
        let onestore_offline::Operation::Text(edit) = &cache.pending().unwrap()[0].operation else {
            panic!()
        };
        assert_eq!(edit.replacement, "L");
    }
}

mod worker {
    use super::*;
    use std::{
        sync::{Arc, Mutex, mpsc},
        time::{Duration, Instant},
    };

    #[derive(Clone)]
    struct Shared(Arc<Mutex<Server>>);

    impl Remote for Shared {
        fn read(&mut self) -> io::Result<Vec<u8>> {
            self.0.lock().unwrap().read()
        }
        fn publish(&mut self, edit: &PreparedEdit<'_>) -> Result<(), CommitError> {
            self.0.lock().unwrap().publish(edit)
        }
        fn confirm(&mut self, snapshot: &[u8]) -> Result<(), CommitError> {
            self.0.lock().unwrap().confirm(snapshot)
        }
    }

    #[test]
    fn reconnects_after_connect_read_and_uncertain_publish_without_replaying() {
        struct Session {
            shared: Shared,
            fail_read: bool,
        }
        impl Remote for Session {
            fn read(&mut self) -> io::Result<Vec<u8>> {
                if self.fail_read {
                    return Err(io::ErrorKind::ConnectionReset.into());
                }
                self.shared.read()
            }
            fn publish(&mut self, edit: &PreparedEdit<'_>) -> Result<(), CommitError> {
                self.shared.publish(edit)
            }
            fn confirm(&mut self, snapshot: &[u8]) -> Result<(), CommitError> {
                self.shared.confirm(snapshot)
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.sqlite");
        let source = onestore::create_section("worker.one", "abc", "Fixture").unwrap();
        let (sid, oid, _) = text(&source);
        let cache = Arc::new(Replica::create(&path, &source).unwrap());
        let id = cache
            .edit_text(&source, sid, oid, 1..2, "🦀")
            .unwrap()
            .unwrap();
        let mut server = Server::new(&source);
        server.fault = Fault::UnknownAfter;
        let server = Arc::new(Mutex::new(server));
        let shared = Shared(Arc::clone(&server));
        let (connected_tx, connected_rx) = mpsc::channel();
        let (observed_tx, observed_rx) = mpsc::channel();
        let mut connections = 0;
        let worker = cache
            .start_sync(
                Duration::from_millis(10),
                move || {
                    connections += 1;
                    connected_tx.send(connections).unwrap();
                    if connections <= 2 {
                        return Err(io::ErrorKind::ConnectionRefused.into());
                    }
                    Ok(Session {
                        shared: shared.clone(),
                        fail_read: connections == 3,
                    })
                },
                move |result| {
                    observed_tx
                        .send(result.as_ref().copied().map_err(|error| error.to_string()))
                        .unwrap();
                },
            )
            .unwrap();
        let mut errors = 0;
        let published = loop {
            match observed_rx.recv_timeout(Duration::from_secs(5)).unwrap() {
                Err(_) => errors += 1,
                Ok(Some((actual, status @ EditStatus::Published { .. }))) => {
                    assert_eq!(actual, id);
                    break status;
                }
                other => panic!("Unexpected result: {other:?}"),
            }
        };
        worker.stop().unwrap();
        assert_eq!(errors, 4);
        assert_eq!(connected_rx.try_iter().collect::<Vec<_>>(), [1, 2, 3, 4, 5]);
        let server = server.lock().unwrap();
        assert_eq!(server.publications, 1);
        assert_eq!(server.confirmations, 1);
        assert_eq!(text(&server.durable).2, "a🦀c");
        drop(cache);
        let cache = Replica::open(&path).unwrap();
        assert_eq!(cache.status(id).unwrap(), Some(published));
        assert_eq!(cache.snapshot().unwrap(), server.durable);
    }

    #[test]
    fn local_edits_wake_an_idle_worker_and_coalesced_notifications_drain_twelve_writers() {
        let dir = tempfile::tempdir().unwrap();
        let source = onestore::create_section("worker.one", "abc", "Fixture").unwrap();
        let (sid, oid, _) = text(&source);
        let cache = Arc::new(Replica::create(dir.path().join("cache.sqlite"), &source).unwrap());
        let server = Arc::new(Mutex::new(Server::new(&source)));
        let shared = Shared(Arc::clone(&server));
        let (observed_tx, observed_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        let mut first = true;
        let worker = cache
            .start_sync(
                Duration::from_secs(3600),
                move || Ok(shared.clone()),
                move |result| {
                    observed_tx.send(*result.as_ref().unwrap()).unwrap();
                    if first {
                        first = false;
                        resume_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                    }
                },
            )
            .unwrap();
        assert_eq!(
            observed_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            None
        );
        let started = Instant::now();
        let edits = std::thread::scope(|scope| {
            (0..12)
                .map(|writer| {
                    let cache = &cache;
                    scope.spawn(move || {
                        let replacement = format!("[{writer}] ");
                        loop {
                            let snapshot = cache.snapshot().unwrap();
                            match cache.edit_text(&snapshot, sid, oid, 0..0, &replacement) {
                                Ok(Some(id)) => break (id, replacement),
                                Err(Error::Io(error))
                                    if error.kind() == io::ErrorKind::ResourceBusy => {}
                                other => panic!("Unexpected local outcome: {other:?}"),
                            }
                        }
                    })
                })
                .collect::<Vec<_>>()
                .into_iter()
                .map(|join| join.join().unwrap())
                .collect::<std::collections::BTreeMap<_, _>>()
        });
        resume_tx.send(()).unwrap();
        let mut published = std::collections::BTreeSet::new();
        while published.len() < 12 {
            if let Some((id, status)) = observed_rx.recv_timeout(Duration::from_secs(5)).unwrap() {
                assert!(matches!(status, EditStatus::Published { .. }), "{status:?}");
                assert!(published.insert(id));
            }
        }
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "Edits waited for the hourly poll"
        );
        worker.stop().unwrap();
        assert_eq!(published, edits.keys().copied().collect());
        let expected = edits.values().rev().cloned().collect::<String>() + "abc";
        let server = server.lock().unwrap();
        assert_eq!(text(&server.durable).2, expected);
        assert_eq!(server.publications, 12);
        assert_eq!(cache.snapshot().unwrap(), server.durable);
        assert!(cache.pending().unwrap().is_empty());
    }

    #[test]
    fn dropping_during_publication_is_nonblocking_and_retains_ownership_until_recovery_is_recorded()
    {
        struct Paused {
            shared: Shared,
            entered: mpsc::Sender<()>,
            resume: mpsc::Receiver<()>,
        }
        impl Remote for Paused {
            fn read(&mut self) -> io::Result<Vec<u8>> {
                self.shared.read()
            }
            fn publish(&mut self, edit: &PreparedEdit<'_>) -> Result<(), CommitError> {
                self.entered.send(()).unwrap();
                self.resume.recv_timeout(Duration::from_secs(5)).unwrap();
                self.shared.publish(edit)
            }
            fn confirm(&mut self, snapshot: &[u8]) -> Result<(), CommitError> {
                self.shared.confirm(snapshot)
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.sqlite");
        let source = onestore::create_section("worker.one", "abc", "Fixture").unwrap();
        let (sid, oid, _) = text(&source);
        let cache = Arc::new(Replica::create(&path, &source).unwrap());
        let id = cache
            .edit_text(&source, sid, oid, 0..0, "L ")
            .unwrap()
            .unwrap();
        let mut server = Server::new(&source);
        server.fault = Fault::UnknownAfter;
        let server = Arc::new(Mutex::new(server));
        let shared = Shared(Arc::clone(&server));
        let (entered_tx, entered_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        let mut session = Some(Paused {
            shared,
            entered: entered_tx,
            resume: resume_rx,
        });
        let worker = cache
            .start_sync(
                Duration::from_secs(3600),
                move || Ok(session.take().unwrap()),
                |_| {},
            )
            .unwrap();
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let stopped = Instant::now();
        drop(worker);
        assert!(stopped.elapsed() < Duration::from_millis(500));
        let shared = Shared(Arc::clone(&server));
        let start = cache.start_sync(
            Duration::from_secs(3600),
            move || Ok(shared.clone()),
            |_| {},
        );
        assert!(matches!(start, Err(error) if error.kind() == io::ErrorKind::WouldBlock));
        let weak = Arc::downgrade(&cache);
        drop(cache);
        resume_tx.send(()).unwrap();
        while weak.upgrade().is_some() {
            assert!(stopped.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(1));
        }
        let cache = Arc::new(Replica::open(&path).unwrap());
        assert!(matches!(
            cache.status(id).unwrap(),
            Some(EditStatus::AwaitingConfirmation { .. })
        ));
        assert_eq!(server.lock().unwrap().publications, 1);
        let shared = Shared(Arc::clone(&server));
        let (tx, rx) = mpsc::channel();
        let worker = cache
            .start_sync(
                Duration::from_secs(3600),
                move || Ok(shared.clone()),
                move |result| {
                    tx.send(*result.as_ref().unwrap()).unwrap();
                },
            )
            .unwrap();
        assert!(
            matches!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), Some((actual, EditStatus::Published { .. })) if actual == id)
        );
        worker.stop().unwrap();
        let server = server.lock().unwrap();
        assert_eq!(server.publications, 1);
        assert_eq!(server.confirmations, 1);
        assert_eq!(text(&server.durable).2, "L abc");
    }

    #[test]
    fn cache_failures_stop_retries_and_return_the_error_without_remote_publication() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.sqlite");
        let source = onestore::create_section("worker.one", "abc", "Fixture").unwrap();
        let (sid, oid, _) = text(&source);
        let cache = Replica::create(&path, &source).unwrap();
        let id = cache
            .edit_text(&source, sid, oid, 0..0, "L ")
            .unwrap()
            .unwrap();
        drop(cache);
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection.execute_batch("CREATE TRIGGER fail_attempt BEFORE INSERT ON attempt BEGIN SELECT RAISE(ABORT, 'test cache write failure'); END;").unwrap();
        drop(connection);
        let cache = Arc::new(Replica::open(&path).unwrap());
        let server = Arc::new(Mutex::new(Server::new(&source)));
        let shared = Shared(Arc::clone(&server));
        let (tx, rx) = mpsc::channel();
        let worker = cache
            .start_sync(
                Duration::from_millis(1),
                move || Ok(shared.clone()),
                move |result| {
                    tx.send(matches!(result, Err(Error::Database(_)))).unwrap();
                },
            )
            .unwrap();
        assert!(rx.recv_timeout(Duration::from_secs(5)).unwrap());
        assert!(matches!(worker.stop(), Err(Error::Database(_))));
        assert!(rx.try_iter().next().is_none());
        assert_eq!(server.lock().unwrap().publications, 0);
        assert_eq!(cache.status(id).unwrap(), Some(EditStatus::Pending));
        assert_eq!(text(&cache.snapshot().unwrap()).2, "L abc");
    }

    #[test]
    fn polling_preserves_conflicts_and_absent_uncertain_revisions_without_replay() {
        for uncertain in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let source = onestore::create_section("worker.one", "abc", "Fixture").unwrap();
            let (sid, oid, _) = text(&source);
            let cache =
                Arc::new(Replica::create(dir.path().join("cache.sqlite"), &source).unwrap());
            let id = cache
                .edit_text(&source, sid, oid, 1..2, "L")
                .unwrap()
                .unwrap();
            let local = cache.snapshot().unwrap();
            let mut server = if uncertain {
                Server::new(&source)
            } else {
                Server::new(&onestore::replace_text(&source, sid, oid, 1..2, "R").unwrap())
            };
            if uncertain {
                server.fault = Fault::UnknownBefore;
            }
            let server = Arc::new(Mutex::new(server));
            let shared = Shared(Arc::clone(&server));
            let (tx, rx) = mpsc::channel();
            let worker = cache
                .start_sync(
                    Duration::from_millis(10),
                    move || Ok(shared.clone()),
                    move |result| {
                        tx.send(result.as_ref().copied().map_err(|_| ())).unwrap();
                    },
                )
                .unwrap();
            if uncertain {
                assert!(rx.recv_timeout(Duration::from_secs(5)).unwrap().is_err());
            }
            let mut previous = None;
            let started = Instant::now();
            for _ in 0..5 {
                let (actual, status) = rx
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap()
                    .unwrap()
                    .unwrap();
                assert_eq!(actual, id);
                if uncertain {
                    assert!(matches!(status, EditStatus::AwaitingConfirmation { .. }));
                } else {
                    assert_eq!(status, EditStatus::Conflict(ConflictKind::TextChanged));
                }
                if let Some(previous) = previous {
                    assert_eq!(previous, status);
                }
                previous = Some(status);
            }
            assert!(
                started.elapsed() >= Duration::from_millis(30),
                "Worker spun instead of waiting between retries"
            );
            worker.stop().unwrap();
            assert_eq!(cache.snapshot().unwrap(), local);
            assert_eq!(cache.pending().unwrap().len(), 1);
            let server = server.lock().unwrap();
            assert_eq!(server.publications, usize::from(uncertain));
            assert_eq!(server.confirmations, 0);
        }
    }

    #[test]
    fn reachability_notification_retries_without_waiting_for_the_poll() {
        let dir = tempfile::tempdir().unwrap();
        let source = onestore::create_section("worker.one", "abc", "Fixture").unwrap();
        let cache = Arc::new(Replica::create(dir.path().join("cache.sqlite"), &source).unwrap());
        let (tx, rx) = mpsc::channel();
        let worker = cache
            .start_sync(
                Duration::from_secs(3600),
                || -> io::Result<Shared> { Err(io::ErrorKind::NotConnected.into()) },
                move |result| {
                    tx.send(matches!(result, Err(Error::RemoteIo(_)))).unwrap();
                },
            )
            .unwrap();
        assert!(rx.recv_timeout(Duration::from_secs(5)).unwrap());
        worker.wake();
        assert!(rx.recv_timeout(Duration::from_secs(5)).unwrap());
        worker.stop().unwrap();
        assert!(rx.try_iter().next().is_none());
    }

    #[test]
    fn cancellation_during_connect_does_not_read_or_report_a_false_refresh() {
        let dir = tempfile::tempdir().unwrap();
        let source = onestore::create_section("worker.one", "abc", "Fixture").unwrap();
        let cache = Arc::new(Replica::create(dir.path().join("cache.sqlite"), &source).unwrap());
        let (entered_tx, entered_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        let (tx, rx) = mpsc::channel();
        // An invalid image makes any unexpected read observable as an error callback.
        let shared = Shared(Arc::new(Mutex::new(Server::new(&[]))));
        let worker = cache
            .start_sync(
                Duration::from_secs(3600),
                move || {
                    entered_tx.send(()).unwrap();
                    resume_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                    Ok(shared.clone())
                },
                move |_| {
                    tx.send(()).unwrap();
                },
            )
            .unwrap();
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        drop(worker);
        let weak = Arc::downgrade(&cache);
        drop(cache);
        resume_tx.send(()).unwrap();
        let started = Instant::now();
        while weak.upgrade().is_some() {
            assert!(started.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)),
            Err(mpsc::RecvTimeoutError::Disconnected)
        );
    }

    #[test]
    fn invalid_intervals_and_callback_panics_leave_worker_ownership_recoverable() {
        let dir = tempfile::tempdir().unwrap();
        let source = onestore::create_section("worker.one", "abc", "Fixture").unwrap();
        let cache = Arc::new(Replica::create(dir.path().join("cache.sqlite"), &source).unwrap());
        for interval in [Duration::ZERO, Duration::MAX] {
            assert!(
                matches!(cache.start_sync(interval, || -> io::Result<Shared> { panic!("Unexpected connection") }, |_| {}), Err(error) if error.kind() == io::ErrorKind::InvalidInput)
            );
        }
        let shared = Shared(Arc::new(Mutex::new(Server::new(&source))));
        let first = shared.clone();
        let (tx, rx) = mpsc::channel();
        let worker = cache
            .start_sync(
                Duration::from_secs(3600),
                move || Ok(first.clone()),
                move |_| {
                    tx.send(()).unwrap();
                    panic!("Test observer panic");
                },
            )
            .unwrap();
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(worker.stop(), Err(Error::Io(_))));
        let (tx, rx) = mpsc::channel();
        let worker = cache
            .start_sync(
                Duration::from_secs(3600),
                move || Ok(shared.clone()),
                move |result| {
                    tx.send(*result.as_ref().unwrap()).unwrap();
                },
            )
            .unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), None);
        worker.stop().unwrap();
        assert_eq!(cache.snapshot().unwrap(), source);
    }

    #[test]
    fn reviewed_conflict_wakes_the_worker_and_publishes_the_original_intent_once() {
        let dir = tempfile::tempdir().unwrap();
        let source = onestore::create_section("resolve.one", "abc", "Fixture").unwrap();
        let (sid, oid, _) = text(&source);
        let cache = Arc::new(Replica::create(dir.path().join("cache.sqlite"), &source).unwrap());
        let id = cache
            .edit_text(&source, sid, oid, 1..2, "L")
            .unwrap()
            .unwrap();
        let remote = onestore::replace_text(&source, sid, oid, 1..2, "R").unwrap();
        let server = Arc::new(Mutex::new(Server::new(&remote)));
        let shared = Shared(Arc::clone(&server));
        let (tx, rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        let mut first = true;
        let worker = cache
            .start_sync(
                Duration::from_secs(3600),
                move || Ok(shared.clone()),
                move |result| {
                    tx.send(*result.as_ref().unwrap()).unwrap();
                    if first {
                        first = false;
                        resume_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                    }
                },
            )
            .unwrap();
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            Some((id, EditStatus::Conflict(ConflictKind::TextChanged)))
        );
        cache
            .rebase_conflict(
                id,
                &cache.snapshot().unwrap(),
                &cache.remote_snapshot().unwrap(),
                1..2,
            )
            .unwrap();
        assert_eq!(cache.status(id).unwrap(), Some(EditStatus::Pending));
        resume_tx.send(()).unwrap();
        assert!(
            matches!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), Some((actual, EditStatus::Published { .. })) if actual == id)
        );
        worker.stop().unwrap();
        assert!(cache.pending().unwrap().is_empty());
        let server = server.lock().unwrap();
        assert_eq!(server.publications, 1);
        assert_eq!(text(&server.durable).2, "aLc");
        assert_eq!(cache.snapshot().unwrap(), server.durable);
    }

    #[test]
    fn ordinary_read_and_unpublished_write_contention_reuse_the_connection() {
        struct Busy {
            server: Server,
            reads: usize,
            writes: usize,
        }
        impl Remote for Busy {
            fn read(&mut self) -> io::Result<Vec<u8>> {
                self.reads += 1;
                match self.reads {
                    1 => Err(io::ErrorKind::WouldBlock.into()),
                    2 => Err(io::ErrorKind::ResourceBusy.into()),
                    _ => self.server.read(),
                }
            }
            fn publish(&mut self, edit: &PreparedEdit<'_>) -> Result<(), CommitError> {
                self.writes += 1;
                match self.writes {
                    1 | 2 => Err(CommitError {
                        state: CommitState::NotCommitted,
                        error: if self.writes == 1 {
                            io::ErrorKind::WouldBlock
                        } else {
                            io::ErrorKind::ResourceBusy
                        }
                        .into(),
                    }),
                    _ => self.server.publish(edit),
                }
            }
            fn confirm(&mut self, source: &[u8]) -> Result<(), CommitError> {
                self.server.confirm(source)
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let source = onestore::create_section("worker.one", "abc", "Fixture").unwrap();
        let (sid, oid, _) = text(&source);
        let cache = Arc::new(Replica::create(dir.path().join("cache.sqlite"), &source).unwrap());
        let id = cache
            .edit_text(&source, sid, oid, 0..0, "L ")
            .unwrap()
            .unwrap();
        let mut remote = Some(Busy {
            server: Server::new(&source),
            reads: 0,
            writes: 0,
        });
        let (tx, rx) = mpsc::channel();
        let worker = cache
            .start_sync(
                Duration::from_millis(1),
                move || Ok(remote.take().expect("Contention caused a reconnect")),
                move |result| {
                    tx.send(result.as_ref().copied().map_err(|error| error.to_string()))
                        .unwrap();
                },
            )
            .unwrap();
        for _ in 0..4 {
            assert!(rx.recv_timeout(Duration::from_secs(5)).unwrap().is_err());
        }
        assert!(
            matches!(rx.recv_timeout(Duration::from_secs(5)).unwrap().unwrap(), Some((actual, EditStatus::Published { .. })) if actual == id)
        );
        worker.stop().unwrap();
        assert_eq!(text(&cache.snapshot().unwrap()).2, "L abc");
        assert!(cache.pending().unwrap().is_empty());
    }
    #[test]
    fn publication_backoff_drains_local_wakes_without_waiting_for_the_idle_poll() {
        struct BusyOnce(Server);
        impl Remote for BusyOnce {
            fn read(&mut self) -> io::Result<Vec<u8>> {
                self.0.read()
            }
            fn publish(&mut self, edit: &PreparedEdit<'_>) -> Result<(), CommitError> {
                let server = &mut self.0;
                if server.publications == 0 {
                    server.publications += 1;
                    return Err(CommitError {
                        state: CommitState::NotCommitted,
                        error: io::ErrorKind::ResourceBusy.into(),
                    });
                }
                server.publish(edit)
            }
            fn confirm(&mut self, source: &[u8]) -> Result<(), CommitError> {
                self.0.confirm(source)
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let source = onestore::create_section("worker.one", "abc", "Fixture").unwrap();
        let (sid, oid, _) = text(&source);
        let cache = Arc::new(Replica::create(dir.path().join("cache.sqlite"), &source).unwrap());
        let first = cache
            .edit_text(&source, sid, oid, 0..0, "L ")
            .unwrap()
            .unwrap();
        let mut remote = Some(BusyOnce(Server::new(&source)));
        let observed = Arc::clone(&cache);
        let (tx, rx) = mpsc::channel();
        let mut failed = false;
        let worker = cache
            .start_sync(
                Duration::from_secs(3600),
                move || Ok(remote.take().expect("Contention must retain the session")),
                move |result| match result {
                    Err(Error::Remote(error)) if error.state == CommitState::NotCommitted => {
                        assert!(!failed);
                        failed = true;
                        let source = observed.snapshot().unwrap();
                        let second = observed
                            .edit_text(&source, sid, oid, 0..0, "Q ")
                            .unwrap()
                            .unwrap();
                        tx.send((second, None)).unwrap();
                    }
                    Ok(Some((id, status))) => tx.send((*id, Some(*status))).unwrap(),
                    Ok(None) => {}
                    other => panic!("Unexpected worker result: {other:?}"),
                },
            )
            .unwrap();
        let (second, status) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(status, None);
        for expected in [first, second] {
            let (id, status) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
            assert_eq!(id, expected);
            assert!(matches!(status, Some(EditStatus::Published { .. })));
        }
        worker.stop().unwrap();
        assert!(cache.pending().unwrap().is_empty());
        assert_eq!(text(&cache.snapshot().unwrap()).2, "Q L abc");
    }
}

#[test]
fn offline_insertions_survive_reopen_rebase_and_dependent_text_edits() {
    use onestore::Insertion;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("insert.sqlite");
    let source = onestore::create_section("insert.one", "Original", "Author").unwrap();
    let (sid, original, _) = text(&source);
    let store = Store::parse(&source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let doc = Document::parse(&index).unwrap();
    let (_, page) = doc.pages().unwrap()[0];
    let cache = Replica::create(&path, &source).unwrap();
    let outline = Insertion::outline(page, 72.0, 144.0, "Offline outline", "Offline author")
        .unwrap()
        .with_formatting(0..7, &[onestore::TextAttribute::Italic(true)])
        .unwrap();
    let first = cache.insert(&source, sid, &outline).unwrap().unwrap();
    let snapshot = cache.snapshot().unwrap();
    let paragraph = Insertion::paragraph(
        outline.object(),
        None,
        "Offline paragraph 🦀",
        "Offline author",
    )
    .unwrap()
    .with_formatting(8..17, &[onestore::TextAttribute::Bold(true)])
    .unwrap();
    let second = cache.insert(&snapshot, sid, &paragraph).unwrap().unwrap();
    let third = cache
        .edit_text(
            &cache.snapshot().unwrap(),
            sid,
            paragraph.text_object(),
            0..0,
            "Edited ",
        )
        .unwrap()
        .unwrap();
    let local = cache.snapshot().unwrap();
    let pending = cache.pending().unwrap();
    drop(cache);
    let cache = Replica::open(&path).unwrap();
    assert_eq!(cache.snapshot().unwrap(), local);
    assert_eq!(cache.pending().unwrap(), pending);
    let remote = onestore::replace_text(&source, sid, original, 0..0, "Remote ").unwrap();
    let mut server = Server::new(&remote);
    for id in [first, second, third] {
        assert!(
            matches!(cache.sync_once(&mut server).unwrap(),Some((observed,EditStatus::Published{..})) if observed==id)
        );
    }
    assert_eq!(server.publications, 3);
    assert!(cache.pending().unwrap().is_empty());
    assert_eq!(cache.snapshot().unwrap(), server.durable);
    let store = Store::parse(&server.durable).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let doc = Document::parse(&index).unwrap();
    let s = &doc.spaces[&sid];
    let v = &s.revisions[&s.contexts[&ExGuid::default()]];
    for (id, wanted) in [
        (original, "Remote Original"),
        (outline.text_object(), "Offline outline"),
        (paragraph.text_object(), "Edited Offline paragraph 🦀"),
    ] {
        assert!(matches!(&v.nodes[&id].kind,Kind::RichText{text,..} if text==wanted));
    }
    let outline_runs = v.text_runs(outline.text_object()).unwrap();
    assert_eq!(outline_runs[0].text, "Offline");
    assert_eq!(outline_runs[0].format.italic, Some(true));
    let runs = v.text_runs(paragraph.text_object()).unwrap();
    assert_eq!(runs[1].text, "paragraph");
    assert_eq!(runs[1].format.bold, Some(true));
    assert_eq!(
        v.nodes[&outline.object()].children.last(),
        Some(&paragraph.object())
    );
}

#[test]
fn uncertain_insertions_reconcile_the_original_revision_without_duplicate_objects() {
    use onestore::Insertion;
    for fault in [
        Fault::Before,
        Fault::UnknownBefore,
        Fault::UnknownAfter,
        Fault::PanicBefore,
        Fault::PanicAfter,
        Fault::Committed,
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("uncertain-insert.sqlite");
        let source = onestore::create_section("insert.one", "Original", "Author").unwrap();
        let store = Store::parse(&source).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let doc = Document::parse(&index).unwrap();
        let (sid, page) = doc.pages().unwrap()[0];
        let cache = Replica::create(&path, &source).unwrap();
        let insertion = Insertion::outline(page, 144.0, 144.0, "Uncertain insertion", "Author")
            .unwrap()
            .with_formatting(0..9, &[onestore::TextAttribute::Bold(true)])
            .unwrap()
            .with_formatting(10..19, &[onestore::TextAttribute::Italic(true)])
            .unwrap();
        let id = cache.insert(&source, sid, &insertion).unwrap().unwrap();
        let local = cache.snapshot().unwrap();
        let mut server = Server::new(&source);
        server.fault = fault;
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            cache.sync_once(&mut server)
        }));
        drop(cache);
        let cache = Replica::open(&path).unwrap();
        assert_eq!(server.publications, 1);
        if matches!(fault, Fault::UnknownBefore | Fault::PanicBefore) {
            let status = cache.status(id).unwrap();
            assert!(matches!(
                status,
                Some(EditStatus::AwaitingConfirmation { .. })
            ));
            for _ in 0..5 {
                assert_eq!(
                    cache.sync_once(&mut server).unwrap(),
                    status.map(|state| (id, state))
                );
            }
            assert_eq!(server.publications, 1);
            assert_eq!(server.durable, source);
            assert_eq!(cache.snapshot().unwrap(), local);
        } else {
            if !matches!(fault, Fault::Committed) {
                assert!(matches!(
                    cache.sync_once(&mut server).unwrap(),
                    Some((_, EditStatus::Published { .. }))
                ));
            }
            assert_eq!(
                server.publications,
                if matches!(fault, Fault::Before) { 2 } else { 1 }
            );
            assert_eq!(
                server.confirmations,
                usize::from(matches!(fault, Fault::UnknownAfter | Fault::PanicAfter))
            );
            let store = Store::parse(&server.durable).unwrap();
            let index = RevisionIndex::parse(&store).unwrap();
            let doc = Document::parse(&index).unwrap();
            let s = &doc.spaces[&sid];
            let v = &s.revisions[&s.contexts[&ExGuid::default()]];
            assert_eq!(v.nodes.values().filter(|node|matches!(&node.kind,Kind::RichText{text,..} if text=="Uncertain insertion")).count(),1);
            assert!(v.nodes.contains_key(&insertion.object()));
            let runs = v.text_runs(insertion.text_object()).unwrap();
            assert_eq!(runs.len(), 3);
            assert_eq!(runs[0].text, "Uncertain");
            assert_eq!(runs[0].format.bold, Some(true));
            assert_eq!(runs[1].text, " ");
            assert_ne!(runs[1].format.bold, Some(true));
            assert_ne!(runs[1].format.italic, Some(true));
            assert_eq!(runs[2].text, "insertion");
            assert_eq!(runs[2].format.italic, Some(true));
            assert!(cache.pending().unwrap().is_empty());
        }
    }
}

#[test]
fn cross_run_text_rebases_preserve_remote_styles_and_survive_lost_replies() {
    use onestore::TextAttribute as A;
    for fault in [Fault::None, Fault::UnknownAfter, Fault::PanicAfter] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("cross-run.sqlite");
        let plain = onestore::create_section("cross.one", "ab🦀cde\u{301}fg", "Author").unwrap();
        let (sid, oid, _) = text(&plain);
        let bold = PreparedEdit::format(&plain, sid, oid, 1..4, &[A::Bold(true)]).unwrap();
        let source = PreparedEdit::format(bold.as_bytes(), sid, oid, 4..7, &[A::Italic(true)])
            .unwrap()
            .as_bytes()
            .to_vec();
        let cache = Replica::create(&path, &source).unwrap();
        let first = cache
            .edit_text(&source, sid, oid, 2..7, "日本語")
            .unwrap()
            .unwrap();
        let local = cache.snapshot().unwrap();
        let second = cache
            .edit_text(&local, sid, oid, 3..4, "🐈")
            .unwrap()
            .unwrap();
        let pending = cache.pending().unwrap();
        drop(cache);
        let cache = Replica::open(&path).unwrap();
        assert_eq!(cache.pending().unwrap(), pending);
        let prefix = PreparedEdit::text(&source, sid, oid, 0..0, "Prefix ").unwrap();
        let remote =
            PreparedEdit::format(prefix.as_bytes(), sid, oid, 9..11, &[A::Underline(true)])
                .unwrap();
        let mut server = Server::new(remote.as_bytes());
        server.fault = fault;
        let attempted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            cache.sync_once(&mut server)
        }));
        if matches!(fault, Fault::None) {
            assert!(
                matches!(attempted.unwrap().unwrap(), Some((id, EditStatus::Published { .. })) if id == first)
            );
        } else {
            assert!(matches!(
                cache.status(first).unwrap(),
                Some(EditStatus::AwaitingConfirmation { .. })
            ));
        }
        drop(cache);
        let cache = Replica::open(&path).unwrap();
        while let Some((_, status)) = cache.sync_once(&mut server).unwrap() {
            assert!(matches!(status, EditStatus::Published { .. }));
        }
        assert_eq!(server.publications, 2);
        assert!(matches!(
            cache.status(second).unwrap(),
            Some(EditStatus::Published { .. })
        ));
        assert_eq!(text(&server.durable).2, "Prefix ab日🐈語\u{301}fg");
        assert_eq!(cache.snapshot().unwrap(), server.durable);
        let store = Store::parse(&server.durable).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let document = Document::parse(&index).unwrap();
        let space = &document.spaces[&sid];
        let revision = &space.revisions[&space.contexts[&ExGuid::default()]];
        let runs = revision.text_runs(oid).unwrap();
        let replacement = runs.iter().find(|run| run.text == "日🐈語").unwrap();
        assert_eq!(replacement.format.bold, Some(true));
        assert_eq!(replacement.format.underline, Some(true));
        assert_ne!(replacement.format.italic, Some(true));
        let suffix = runs.last().unwrap();
        assert_eq!(suffix.text, "\u{301}fg");
        assert_ne!(suffix.format.bold, Some(true));
        assert_ne!(suffix.format.underline, Some(true));
        assert_ne!(suffix.format.italic, Some(true));
    }
}

#[test]
fn offline_formatting_rebases_text_and_merges_independent_attributes() {
    use onestore::TextAttribute as A;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("format.sqlite");
    let source = onestore::create_section("format.one", "ab🦀cd", "Author").unwrap();
    let (sid, id, _) = text(&source);
    let cache = Replica::create(&path, &source).unwrap();
    let local = cache
        .format(&source, sid, id, 2..4, &[A::Bold(true)])
        .unwrap()
        .unwrap();
    let pending = cache.pending().unwrap();
    drop(cache);
    let cache = Replica::open(&path).unwrap();
    assert_eq!(cache.pending().unwrap(), pending);
    let moved = PreparedEdit::text(&source, sid, id, 0..0, "Prefix ").unwrap();
    let styled =
        PreparedEdit::format(moved.as_bytes(), sid, id, 9..11, &[A::Italic(true)]).unwrap();
    let mut server = Server::new(styled.as_bytes());
    assert!(
        matches!(cache.sync_once(&mut server).unwrap(),Some((observed,EditStatus::Published{..}))if observed==local)
    );
    let store = Store::parse(&server.durable).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let doc = Document::parse(&index).unwrap();
    let space = &doc.spaces[&sid];
    let view = &space.revisions[&space.contexts[&ExGuid::default()]];
    let runs = view.text_runs(id).unwrap();
    let selected = runs.iter().find(|r| r.text == "🦀").unwrap();
    assert_eq!(selected.format.bold, Some(true));
    assert_eq!(selected.format.italic, Some(true));
    assert_eq!(
        runs.iter().map(|r| r.text).collect::<String>(),
        "Prefix ab🦀cd"
    );
}

#[test]
fn competing_font_changes_remain_preserved_conflicts() {
    use onestore::TextAttribute as A;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("conflict.sqlite");
    let source = onestore::create_section("format.one", "abcdef", "Author").unwrap();
    let (sid, id, _) = text(&source);
    let cache = Replica::create(&path, &source).unwrap();
    let local_id = cache
        .format(&source, sid, id, 1..5, &[A::FontSize(14.0)])
        .unwrap()
        .unwrap();
    let local = cache.snapshot().unwrap();
    let pending = cache.pending().unwrap();
    let remote = PreparedEdit::format(&source, sid, id, 2..4, &[A::FontSize(18.0)]).unwrap();
    let mut server = Server::new(remote.as_bytes());
    for _ in 0..3 {
        assert_eq!(
            cache.sync_once(&mut server).unwrap(),
            Some((
                local_id,
                EditStatus::Conflict(ConflictKind::FormattingChanged)
            ))
        );
    }
    assert_eq!(server.publications, 0);
    assert_eq!(server.confirmations, 0);
    assert_eq!(cache.snapshot().unwrap(), local);
    assert_eq!(cache.pending().unwrap(), pending);
    drop(cache);
    let cache = Replica::open(&path).unwrap();
    assert_eq!(
        cache.status(local_id).unwrap(),
        Some(EditStatus::Conflict(ConflictKind::FormattingChanged))
    );
    assert_eq!(cache.snapshot().unwrap(), local);
}

#[test]
fn independently_satisfied_formatting_requires_confirmation_before_a_receipt() {
    use onestore::TextAttribute as A;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("satisfied.sqlite");
    let source = onestore::create_section("format.one", "abcdef", "Author").unwrap();
    let (sid, id, _) = text(&source);
    let cache = Replica::create(&path, &source).unwrap();
    let local_id = cache
        .format(&source, sid, id, 1..5, &[A::Bold(true)])
        .unwrap()
        .unwrap();
    let remote = PreparedEdit::format(&source, sid, id, 1..5, &[A::Bold(true)]).unwrap();
    let mut server = Server::new(remote.as_bytes());
    server.durable = source.clone();
    server.fault = Fault::Confirm;
    assert!(
        matches!(cache.sync_once(&mut server),Err(Error::Remote(e))if e.state==CommitState::Unknown)
    );
    assert_eq!(cache.status(local_id).unwrap(), Some(EditStatus::Pending));
    assert_eq!(server.publications, 0);
    assert_eq!(server.durable, source);
    drop(cache);
    let cache = Replica::open(&path).unwrap();
    assert!(
        matches!(cache.sync_once(&mut server).unwrap(),Some((id,EditStatus::Published{..}))if id==local_id)
    );
    assert_eq!(server.publications, 0);
    assert_eq!(server.confirmations, 2);
    assert_ne!(server.durable, source);
    assert!(cache.pending().unwrap().is_empty());
}

#[test]
fn retired_format_attempts_require_the_complete_durable_effect_without_replay() {
    use onestore::TextAttribute as A;
    for (complete, fault) in [
        (true, Fault::None),
        (true, Fault::Confirm),
        (true, Fault::ConfirmCommitted),
        (false, Fault::None),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("retired-format.sqlite");
        let source = onestore::create_section("format.one", "abcdef", "Author").unwrap();
        let (sid, object, _) = text(&source);
        let cache = Replica::create(&path, &source).unwrap();
        let id = cache
            .format(
                &source,
                sid,
                object,
                1..5,
                &[A::Bold(true), A::FontSize(18.0)],
            )
            .unwrap()
            .unwrap();
        let local = cache.snapshot().unwrap();
        let mut server = Server::new(&source);
        server.fault = Fault::UnknownAfter;
        assert!(cache.sync_once(&mut server).is_err());
        let attempted = cache.status(id).unwrap();
        let Some(EditStatus::AwaitingConfirmation { revision: retired }) = attempted else {
            panic!()
        };
        drop(cache);
        let prefix = PreparedEdit::text(&source, sid, object, 0..0, "prefix ").unwrap();
        let mut attributes = vec![A::Bold(true), A::Italic(true)];
        if complete {
            attributes.push(A::FontSize(18.0));
        }
        server.visible = PreparedEdit::format(prefix.as_bytes(), sid, object, 8..12, &attributes)
            .unwrap()
            .as_bytes()
            .to_vec();
        let store = Store::parse(&server.visible).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let current = index.spaces[&sid].labels[&(ExGuid::default(), 1)];
        assert!(!index.spaces[&sid].revisions.contains_key(&retired));
        let cache = Replica::open(&path).unwrap();
        server.fault = fault;
        let result = cache.sync_once(&mut server);
        if !complete {
            assert_eq!(result.unwrap(), attempted.map(|s| (id, s)));
            assert_eq!(server.confirmations, 0);
            assert_eq!(cache.snapshot().unwrap(), local);
            assert_eq!(server.durable, source);
        } else {
            if matches!(fault, Fault::Confirm) {
                assert!(matches!(result, Err(Error::Remote(e)) if e.state == CommitState::Unknown));
                assert_eq!(cache.status(id).unwrap(), attempted);
                assert_eq!(cache.snapshot().unwrap(), local);
                assert_eq!(server.durable, source);
                let complete = server.visible.clone();
                server.visible =
                    PreparedEdit::format(&complete, sid, object, 8..12, &[A::Bold(false)])
                        .unwrap()
                        .as_bytes()
                        .to_vec();
                assert_eq!(
                    cache.sync_once(&mut server).unwrap(),
                    attempted.map(|s| (id, s))
                );
                assert_eq!(server.confirmations, 1);
                assert_eq!(cache.snapshot().unwrap(), local);
                server.visible = complete;
                cache.sync_once(&mut server).unwrap();
            } else if matches!(fault, Fault::ConfirmCommitted) {
                assert!(
                    matches!(result, Err(Error::Remote(e)) if e.state == CommitState::Committed)
                );
            } else {
                assert_eq!(
                    result.unwrap(),
                    Some((id, EditStatus::Published { revision: current }))
                );
            }
            assert_ne!(current, retired);
            assert_eq!(
                cache.status(id).unwrap(),
                Some(EditStatus::Published { revision: current })
            );
            assert!(cache.pending().unwrap().is_empty());
            assert_eq!(text(&server.durable).2, "prefix abcdef");
            assert_ne!(server.durable, source);
            drop(cache);
            let cache = Replica::open(&path).unwrap();
            assert_eq!(
                cache.status(id).unwrap(),
                Some(EditStatus::Published { revision: current })
            );
        }
        assert_eq!(server.publications, 1);
    }
}

#[test]
fn retired_text_with_equal_plaintext_but_a_different_surviving_run_remains_uncertain() {
    use onestore::TextAttribute;

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("repeated.sqlite");
    let plain = onestore::create_section("repeated.one", "aa", "Fixture").unwrap();
    let (space, object, _) = text(&plain);
    let source = PreparedEdit::format(&plain, space, object, 0..1, &[TextAttribute::Bold(true)])
        .unwrap()
        .as_bytes()
        .to_vec();
    let cache = Replica::create(&path, &source).unwrap();
    let id = cache
        .edit_text(&source, space, object, 0..1, "")
        .unwrap()
        .unwrap();
    let local = cache.snapshot().unwrap();
    let mut server = Server::new(&source);
    server.fault = Fault::UnknownAfter;
    assert!(cache.sync_once(&mut server).is_err());
    let attempt = cache.status(id).unwrap();
    server.visible = onestore::replace_text(&source, space, object, 1..2, "").unwrap();
    assert_eq!(text(&server.visible).2, text(&local).2);
    let bold = |bytes: &[u8]| {
        let store = Store::parse(bytes).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let document = Document::parse(&index).unwrap();
        let space = &document.spaces[&space];
        space.revisions[&space.contexts[&ExGuid::default()]]
            .text_runs(object)
            .unwrap()
            .into_iter()
            .find(|run| !run.text.is_empty())
            .unwrap()
            .format
            .bold
    };
    assert_eq!(bold(&server.visible), Some(true));
    assert_ne!(bold(&server.visible), bold(&local));
    drop(cache);
    let cache = Replica::open(&path).unwrap();
    assert_eq!(
        cache.sync_once(&mut server).unwrap(),
        attempt.map(|state| (id, state))
    );
    assert_eq!(server.publications, 1);
    assert_eq!(server.confirmations, 0);
    assert_eq!(cache.snapshot().unwrap(), local);
}

#[test]
fn uncertain_formatting_keeps_the_original_attempt_and_never_replays() {
    use onestore::TextAttribute as A;
    for fault in [Fault::UnknownBefore, Fault::UnknownAfter] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("unknown-format.sqlite");
        let source = onestore::create_section("format.one", "abcdef", "Author").unwrap();
        let (sid, id, _) = text(&source);
        let cache = Replica::create(&path, &source).unwrap();
        let local_id = cache
            .format(&source, sid, id, 1..5, &[A::Bold(true)])
            .unwrap()
            .unwrap();
        let mut server = Server::new(&source);
        server.fault = fault;
        assert!(
            matches!(cache.sync_once(&mut server),Err(Error::Remote(e))if e.state==CommitState::Unknown)
        );
        let status = cache.status(local_id).unwrap();
        drop(cache);
        let cache = Replica::open(&path).unwrap();
        if matches!(fault, Fault::UnknownBefore) {
            for _ in 0..4 {
                assert_eq!(
                    cache.sync_once(&mut server).unwrap(),
                    status.map(|s| (local_id, s))
                );
            }
            assert_eq!(server.confirmations, 0);
        } else {
            assert!(matches!(
                cache.sync_once(&mut server).unwrap(),
                Some((_, EditStatus::Published { .. }))
            ));
            assert_eq!(server.confirmations, 1);
        }
        assert_eq!(server.publications, 1);
    }
}

#[test]
fn reviewed_format_conflicts_preserve_dependent_edits_and_recheck_later_remote_changes() {
    use onestore::TextAttribute as A;
    for changed_again in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("review-format.sqlite");
        let source = onestore::create_section("format.one", "abcdef", "Author").unwrap();
        let (sid, id, _) = text(&source);
        let cache = Replica::create(&path, &source).unwrap();
        let first = cache
            .format(&source, sid, id, 1..5, &[A::FontSize(14.0)])
            .unwrap()
            .unwrap();
        let second = cache
            .edit_text(&cache.snapshot().unwrap(), sid, id, 2..2, "X")
            .unwrap()
            .unwrap();
        let local = cache.snapshot().unwrap();
        let pending = cache.pending().unwrap();
        let remote = PreparedEdit::format(&source, sid, id, 1..5, &[A::FontSize(18.0)]).unwrap();
        let mut server = Server::new(remote.as_bytes());
        assert_eq!(
            cache.sync_once(&mut server).unwrap(),
            Some((first, EditStatus::Conflict(ConflictKind::FormattingChanged)))
        );
        assert!(
            matches!(cache.rebase_conflict(first,&source,remote.as_bytes(),1..5),Err(Error::Io(e))if e.kind()==io::ErrorKind::ResourceBusy)
        );
        cache
            .rebase_conflict(first, &local, remote.as_bytes(), 1..5)
            .unwrap();
        assert_eq!(cache.snapshot().unwrap(), local);
        assert_eq!(cache.pending().unwrap()[1], pending[1]);
        drop(cache);
        let cache = Replica::open(&path).unwrap();
        if changed_again {
            let newer =
                PreparedEdit::format(&server.visible, sid, id, 1..5, &[A::FontSize(20.0)]).unwrap();
            server = Server::new(newer.as_bytes());
            assert_eq!(
                cache.sync_once(&mut server).unwrap(),
                Some((first, EditStatus::Conflict(ConflictKind::FormattingChanged)))
            );
            assert_eq!(server.publications, 0);
            assert_eq!(cache.snapshot().unwrap(), local);
        } else {
            for expected in [first, second] {
                assert!(
                    matches!(cache.sync_once(&mut server).unwrap(),Some((id,EditStatus::Published{..}))if id==expected)
                );
            }
            assert_eq!(text(&server.durable).2, "abXcdef");
            assert!(cache.pending().unwrap().is_empty());
        }
    }
}

#[test]
fn superscript_reconciliation_checks_the_implicit_subscript_change() {
    use onestore::TextAttribute as A;
    let directory = tempfile::tempdir().unwrap();
    let source = onestore::create_section("script.one", "abc", "Author").unwrap();
    let (sid, id, _) = text(&source);
    let cache = Replica::create(directory.path().join("script.sqlite"), &source).unwrap();
    let local = cache
        .format(&source, sid, id, 0..3, &[A::Superscript(true)])
        .unwrap()
        .unwrap();
    let remote = PreparedEdit::format(&source, sid, id, 0..3, &[A::Subscript(true)]).unwrap();
    let mut server = Server::new(remote.as_bytes());
    assert_eq!(
        cache.sync_once(&mut server).unwrap(),
        Some((local, EditStatus::Conflict(ConflictKind::FormattingChanged)))
    );
    assert_eq!(server.publications, 0);
}

#[test]
fn seeded_formatting_reconciliation_matches_a_character_model() {
    use onestore::TextAttribute as A;
    #[derive(Clone, Debug, PartialEq)]
    struct Style {
        size: f32,
        color: u32,
        bold: bool,
        italic: bool,
    }
    let mut conflicts = 0;
    let mut published = 0;
    let mut satisfied_parts = 0;
    for seed in 1_u64..=128 {
        let mut random = seed;
        let mut next = || {
            random = random
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            random >> 32
        };
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("model.sqlite");
        let mut source =
            onestore::create_section("model.one", "abcdefghijklmnopqrstuvwxyz012345", "Author")
                .unwrap();
        let (sid, object, original_text) = text(&source);
        let mut baseline = vec![
            Style {
                size: 11.0,
                color: 0xff000000,
                bold: false,
                italic: false
            };
            32
        ];
        for _ in 0..6 {
            let start = next() as usize % 32;
            let end = start + 1 + next() as usize % (32 - start);
            let size = [12.0, 14.0, 18.0][next() as usize % 3];
            let color: u32 = [0xabcdef, 0x987654, 0xff000000][next() as usize % 3];
            let bold = next() % 2 == 0;
            source = PreparedEdit::format(
                &source,
                sid,
                object,
                start as u32..end as u32,
                &[
                    A::FontSize(size),
                    A::Color((color != 0xff000000).then(|| {
                        let b = color.to_le_bytes();
                        [b[0], b[1], b[2]]
                    })),
                    A::Bold(bold),
                ],
            )
            .unwrap()
            .as_bytes()
            .to_vec();
            for style in &mut baseline[start..end] {
                style.size = size;
                style.color = color;
                style.bold = bold;
            }
        }
        let start = next() as usize % 24;
        let end = start + 2 + next() as usize % (31 - start);
        let attribute = match seed % 3 {
            0 => A::FontSize(21.0),
            1 => A::Color(Some([0x12, 0x34, 0x56])),
            _ => A::Bold(!baseline[start].bold),
        };
        let cache = Replica::create(&path, &source).unwrap();
        let id = cache
            .format(
                &source,
                sid,
                object,
                start as u32..end as u32,
                std::slice::from_ref(&attribute),
            )
            .unwrap()
            .unwrap();
        let local = cache.snapshot().unwrap();
        let mut remote = source.clone();
        let mut expected = baseline.clone();
        for step in 0..8 {
            let left = next() as usize % 32;
            let right = left + 1 + next() as usize % (32 - left);
            let update = if step % 3 == 0 {
                attribute.clone()
            } else {
                match next() % 4 {
                    0 => A::FontSize([11.0, 14.0, 18.0, 21.0][next() as usize % 4]),
                    1 => A::Color(Some([0x99, 0x88, 0x77])),
                    2 => A::Bold(next() % 2 == 0),
                    _ => A::Italic(true),
                }
            };
            remote = PreparedEdit::format(
                &remote,
                sid,
                object,
                left as u32..right as u32,
                std::slice::from_ref(&update),
            )
            .unwrap()
            .as_bytes()
            .to_vec();
            for style in &mut expected[left..right] {
                match update {
                    A::FontSize(value) => style.size = value,
                    A::Color(Some([r, g, b])) => style.color = u32::from_le_bytes([r, g, b, 0]),
                    A::Bold(value) => style.bold = value,
                    A::Italic(value) => style.italic = value,
                    _ => unreachable!(),
                }
            }
        }
        let conflict = (start..end).any(|i| match attribute {
            A::FontSize(value) => expected[i].size != baseline[i].size && expected[i].size != value,
            A::Color(_) => expected[i].color != baseline[i].color && expected[i].color != 0x563412,
            A::Bold(value) => expected[i].bold != baseline[i].bold && expected[i].bold != value,
            _ => unreachable!(),
        });
        drop(cache);
        let cache = Replica::open(&path).unwrap();
        let mut server = Server::new(&remote);
        let result = cache.sync_once(&mut server).unwrap().unwrap();
        assert_eq!(result.0, id, "seed {seed}");
        if conflict {
            conflicts += 1;
            assert_eq!(
                result.1,
                EditStatus::Conflict(ConflictKind::FormattingChanged),
                "seed {seed}"
            );
            assert_eq!(server.publications, 0, "seed {seed}");
            assert_eq!(cache.snapshot().unwrap(), local, "seed {seed}");
            continue;
        }
        published += 1;
        assert!(
            matches!(result.1, EditStatus::Published { .. }),
            "seed {seed}: {result:?}"
        );
        for style in &mut expected[start..end] {
            match attribute {
                A::FontSize(value) => {
                    satisfied_parts += usize::from(style.size == value);
                    style.size = value;
                }
                A::Color(_) => {
                    satisfied_parts += usize::from(style.color == 0x563412);
                    style.color = 0x563412;
                }
                A::Bold(value) => {
                    satisfied_parts += usize::from(style.bold == value);
                    style.bold = value;
                }
                _ => unreachable!(),
            }
        }
        assert_eq!(text(&server.durable).2, original_text, "seed {seed}");
        let store = Store::parse(&server.durable).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let doc = Document::parse(&index).unwrap();
        let space = &doc.spaces[&sid];
        let revision = &space.revisions[&space.contexts[&ExGuid::default()]];
        let actual: Vec<_> = revision
            .text_runs(object)
            .unwrap()
            .iter()
            .flat_map(|run| {
                std::iter::repeat_n(
                    Style {
                        size: run.format.font_size.unwrap(),
                        color: run.format.color.unwrap_or(0xff000000),
                        bold: run.format.bold.unwrap_or(false),
                        italic: run.format.italic.unwrap_or(false),
                    },
                    run.text.chars().count(),
                )
            })
            .collect();
        assert_eq!(actual, expected, "seed {seed}");
        drop(cache);
        let cache = Replica::open(&path).unwrap();
        assert_eq!(cache.status(id).unwrap(), Some(result.1));
        let snapshot = cache.snapshot().unwrap();
        assert_eq!(snapshot.len(), server.durable.len(), "seed {seed}");
        assert!(
            snapshot
                .iter()
                .zip(&server.durable)
                .enumerate()
                .all(|(offset, (a, b))| a == b || (212..252).contains(&offset)),
            "seed {seed}: only confirmation version metadata may change"
        );
        assert_eq!(cache.sync_once(&mut server).unwrap(), None);
        assert!(
            cache.snapshot().unwrap() == server.durable,
            "seed {seed}: refreshed snapshot"
        );
    }
    assert!(
        conflicts >= 16 && published >= 32 && satisfied_parts >= 128,
        "conflicts={conflicts}, published={published}, already desired characters={satisfied_parts}"
    );
    println!(
        "128 seeds: {conflicts} conflicts, {published} publications, {satisfied_parts} already desired characters"
    );
}

#[test]
fn inserted_empty_text_retains_formatting_and_dependent_text_across_sync() {
    use onestore::{Insertion, TextAttribute as A};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("empty.sqlite");
    let source = onestore::create_section("empty.one", "Original", "Author").unwrap();
    let store = Store::parse(&source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let doc = Document::parse(&index).unwrap();
    let (sid, page) = doc.pages().unwrap()[0];
    let insertion = Insertion::outline(page, 144.0, 144.0, "", "Author").unwrap();
    let cache = Replica::create(&path, &source).unwrap();
    let first = cache.insert(&source, sid, &insertion).unwrap().unwrap();
    let second = cache
        .format(
            &cache.snapshot().unwrap(),
            sid,
            insertion.text_object(),
            0..0,
            &[A::Bold(true), A::FontSize(18.0)],
        )
        .unwrap()
        .unwrap();
    let third = cache
        .edit_text(
            &cache.snapshot().unwrap(),
            sid,
            insertion.text_object(),
            0..0,
            "Typed 🦀",
        )
        .unwrap()
        .unwrap();
    let pending = cache.pending().unwrap();
    drop(cache);
    let cache = Replica::open(&path).unwrap();
    assert_eq!(cache.pending().unwrap(), pending);
    let mut server = Server::new(&source);
    for id in [first, second, third] {
        let result = cache.sync_once(&mut server).unwrap();
        assert!(
            matches!(result,Some((actual,EditStatus::Published{..}))if actual==id),
            "{result:?}"
        );
    }
    let store = Store::parse(&server.durable).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let doc = Document::parse(&index).unwrap();
    let space = &doc.spaces[&sid];
    let revision = &space.revisions[&space.contexts[&ExGuid::default()]];
    let runs = revision.text_runs(insertion.text_object()).unwrap();
    assert_eq!(runs.iter().map(|r| r.text).collect::<String>(), "Typed 🦀");
    assert!(
        runs.iter()
            .all(|r| r.format.bold == Some(true) && r.format.font_size == Some(18.0))
    );
    assert_eq!(cache.snapshot().unwrap(), server.durable);
}

#[test]
fn every_visual_attribute_rebases_with_an_independent_remote_attribute() {
    use onestore::TextAttribute as A;
    use serde_json::json;
    let mut cases = Vec::new();
    for value in [false, true] {
        cases.extend([
            (A::Bold(!value), A::Bold(value), "bold", json!(value)),
            (A::Italic(!value), A::Italic(value), "italic", json!(value)),
            (
                A::Underline(!value),
                A::Underline(value),
                "underline",
                json!(value),
            ),
            (A::Strike(!value), A::Strike(value), "strike", json!(value)),
            (
                A::Superscript(!value),
                A::Superscript(value),
                "superscript",
                json!(value),
            ),
            (
                A::Subscript(!value),
                A::Subscript(value),
                "subscript",
                json!(value),
            ),
        ]);
    }
    cases.extend([
        (
            A::Font("Georgia".into()),
            A::Font("Arial".into()),
            "font",
            json!("Arial"),
        ),
        (
            A::FontSize(11.0),
            A::FontSize(18.0),
            "font_size",
            json!(18.0),
        ),
        (
            A::Color(None),
            A::Color(Some([1, 2, 3])),
            "color",
            json!(0x030201),
        ),
        (
            A::Color(Some([1, 2, 3])),
            A::Color(None),
            "color",
            json!(0xff000000_u32),
        ),
        (
            A::Highlight(None),
            A::Highlight(Some([4, 5, 6])),
            "highlight",
            json!(0x060504),
        ),
        (
            A::Highlight(Some([4, 5, 6])),
            A::Highlight(None),
            "highlight",
            json!(0xff000000_u32),
        ),
    ]);
    for (baseline, desired, field, value) in cases {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("attribute.sqlite");
        let source = onestore::create_section("attribute.one", "abc", "Author").unwrap();
        let (sid, object, _) = text(&source);
        let source = PreparedEdit::format(&source, sid, object, 0..3, &[baseline])
            .unwrap()
            .as_bytes()
            .to_vec();
        let cache = Replica::create(&path, &source).unwrap();
        let id = cache
            .format(&source, sid, object, 0..3, &[desired])
            .unwrap()
            .unwrap();
        let moved = PreparedEdit::text(&source, sid, object, 0..0, "Z").unwrap();
        let (other, other_field, other_value) = if field == "font_size" {
            (A::Italic(true), "italic", json!(true))
        } else {
            (A::FontSize(22.0), "font_size", json!(22.0))
        };
        let remote = PreparedEdit::format(moved.as_bytes(), sid, object, 1..4, &[other]).unwrap();
        let mut server = Server::new(remote.as_bytes());
        drop(cache);
        let cache = Replica::open(&path).unwrap();
        let result = cache.sync_once(&mut server).unwrap();
        assert!(
            matches!(result,Some((observed,EditStatus::Published{..}))if observed==id),
            "{field}={value}: {result:?}"
        );
        assert_eq!(text(&server.durable).2, "Zabc");
        let store = Store::parse(&server.durable).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let doc = Document::parse(&index).unwrap();
        let space = &doc.spaces[&sid];
        let revision = &space.revisions[&space.contexts[&ExGuid::default()]];
        let mut position = 0;
        for run in revision.text_runs(object).unwrap() {
            let format = serde_json::to_value(run.format).unwrap();
            for _ in run.text.chars() {
                if position > 0 {
                    assert_eq!(format[field], value, "{field}, character {position}");
                    assert_eq!(
                        format[other_field], other_value,
                        "{field}, character {position}"
                    );
                }
                position += 1;
            }
        }
        assert_eq!(position, 4);
    }
}

#[test]
fn reviewed_insertion_placements_preserve_identity_and_dependent_operations() {
    use onestore::{Insertion, TextAttribute as A};
    use onestore_offline::Operation;
    for outline_case in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("placement.sqlite");
        let base = onestore::create_section("placement.one", "Original", "Author").unwrap();
        let (sid, _, _) = text(&base);
        let store = Store::parse(&base).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let doc = Document::parse(&index).unwrap();
        let (_, page) = doc.pages().unwrap()[0];
        let space = &doc.spaces[&sid];
        let view = &space.revisions[&space.contexts[&ExGuid::default()]];
        let parent = *view.nodes[&page]
            .children
            .iter()
            .find(|id| matches!(view.nodes[id].kind, Kind::Outline { .. }))
            .unwrap();
        let anchor = Insertion::paragraph(parent, None, "Temporary anchor", "Author").unwrap();
        let source = PreparedEdit::insert(&base, sid, &anchor)
            .unwrap()
            .as_bytes()
            .to_vec();
        let insertion = if outline_case {
            Insertion::outline(page, 144.0, 144.0, "Offline", "Author").unwrap()
        } else {
            Insertion::paragraph(parent, Some(anchor.object()), "Offline", "Author").unwrap()
        }
        .with_formatting(0..7, &[A::Italic(true)])
        .unwrap();
        let cache = Replica::create(&path, &source).unwrap();
        let first = cache.insert(&source, sid, &insertion).unwrap().unwrap();
        let second = cache
            .format(
                &cache.snapshot().unwrap(),
                sid,
                insertion.text_object(),
                0..7,
                &[A::Bold(true)],
            )
            .unwrap()
            .unwrap();
        let third = cache
            .edit_text(
                &cache.snapshot().unwrap(),
                sid,
                insertion.text_object(),
                7..7,
                " 🦀",
            )
            .unwrap()
            .unwrap();
        let local = cache.snapshot().unwrap();
        let pending = cache.pending().unwrap();
        let mut server = Server::new(&base);
        if outline_case {
            drop(cache);
            let db = rusqlite::Connection::open(&path).unwrap();
            db.execute(
                "INSERT INTO conflicts VALUES (?1,2)",
                [i64::try_from(first).unwrap()],
            )
            .unwrap();
            db.execute("UPDATE replica SET base=?1", [&base]).unwrap();
            drop(db);
        } else {
            assert_eq!(
                cache.sync_once(&mut server).unwrap(),
                Some((first, EditStatus::Conflict(ConflictKind::UnsupportedEdit)))
            );
            drop(cache);
        }
        let cache = Replica::open(&path).unwrap();
        if outline_case {
            assert!(
                cache
                    .rebase_paragraph_conflict(first, &local, &base, parent, None)
                    .is_err()
            );
            assert!(
                cache
                    .rebase_outline_conflict(first, &local, &base, page, f32::NAN, 288.0)
                    .is_err()
            );
            assert!(
                matches!(cache.rebase_outline_conflict(first,&source,&base,page,288.0,360.0),Err(Error::Io(e))if e.kind()==io::ErrorKind::ResourceBusy)
            );
            cache
                .rebase_outline_conflict(first, &local, &base, page, 288.0, 360.0)
                .unwrap();
        } else {
            assert!(
                cache
                    .rebase_outline_conflict(first, &local, &base, page, 288.0, 360.0)
                    .is_err()
            );
            assert!(
                cache
                    .rebase_paragraph_conflict(first, &local, &base, parent, Some(anchor.object()))
                    .is_err()
            );
            assert!(
                matches!(cache.rebase_paragraph_conflict(first,&source,&base,parent,None),Err(Error::Io(e))if e.kind()==io::ErrorKind::ResourceBusy)
            );
            cache
                .rebase_paragraph_conflict(first, &local, &base, parent, None)
                .unwrap();
        }
        assert_eq!(cache.snapshot().unwrap(), local);
        assert_eq!(cache.pending().unwrap()[1..], pending[1..]);
        let Operation::Insert(rebased) = cache.pending().unwrap().remove(0).operation else {
            panic!()
        };
        assert_eq!(rebased.object(), insertion.object());
        assert_eq!(rebased.text_object(), insertion.text_object());
        assert_eq!(
            serde_json::to_value(&rebased).unwrap()["formats"],
            serde_json::to_value(&insertion).unwrap()["formats"]
        );
        assert_eq!(cache.status(first).unwrap(), Some(EditStatus::Pending));
        drop(cache);
        let cache = Replica::open(&path).unwrap();
        for id in [first, second, third] {
            let result = cache.sync_once(&mut server).unwrap();
            assert!(
                matches!(result,Some((actual,EditStatus::Published{..}))if actual==id),
                "{result:?}"
            );
        }
        let store = Store::parse(&server.durable).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let doc = Document::parse(&index).unwrap();
        let space = &doc.spaces[&sid];
        let view = &space.revisions[&space.contexts[&ExGuid::default()]];
        let runs = view.text_runs(insertion.text_object()).unwrap();
        assert_eq!(
            runs.iter().map(|r| r.text).collect::<String>(),
            "Offline 🦀"
        );
        assert!(runs.iter().all(|r| r.format.bold == Some(true)));
        assert!(runs.iter().all(|r| r.format.italic == Some(true)));
        if outline_case {
            assert_eq!(
                (
                    view.nodes[&insertion.object()].layout.x,
                    view.nodes[&insertion.object()].layout.y
                ),
                (Some(288.0), Some(360.0))
            );
        } else {
            assert_eq!(
                view.nodes[&parent].children.last(),
                Some(&insertion.object())
            );
        }
        assert!(!view.nodes.contains_key(&anchor.object()));
        assert_eq!(server.publications, 3);
        assert!(cache.pending().unwrap().is_empty());
        assert_eq!(cache.snapshot().unwrap(), server.durable);
    }
}

#[test]
fn placement_reviews_cannot_replace_pending_or_uncertain_attempts() {
    use onestore::Insertion;
    for outline_case in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("attempt-placement.sqlite");
        let source = onestore::create_section("placement.one", "Original", "Author").unwrap();
        let store = Store::parse(&source).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let doc = Document::parse(&index).unwrap();
        let (sid, page) = doc.pages().unwrap()[0];
        let space = &doc.spaces[&sid];
        let view = &space.revisions[&space.contexts[&ExGuid::default()]];
        let parent = *view.nodes[&page]
            .children
            .iter()
            .find(|id| matches!(view.nodes[id].kind, Kind::Outline { .. }))
            .unwrap();
        let insertion = if outline_case {
            Insertion::outline(page, 144.0, 144.0, "Offline", "Author").unwrap()
        } else {
            Insertion::paragraph(parent, None, "Offline", "Author").unwrap()
        };
        let cache = Replica::create(&path, &source).unwrap();
        let id = cache.insert(&source, sid, &insertion).unwrap().unwrap();
        let local = cache.snapshot().unwrap();
        let pending = cache.pending().unwrap();
        let mut server = Server::new(&source);
        for attempted in [false, true] {
            if attempted {
                server.fault = Fault::UnknownBefore;
                assert!(
                    matches!(cache.sync_once(&mut server),Err(Error::Remote(e))if e.state==CommitState::Unknown)
                );
            }
            let state = cache.status(id).unwrap();
            let result = if outline_case {
                cache.rebase_outline_conflict(id, &local, &source, page, 288.0, 360.0)
            } else {
                cache.rebase_paragraph_conflict(id, &local, &source, parent, None)
            };
            assert!(matches!(result,Err(Error::Io(e))if e.kind()==io::ErrorKind::InvalidInput));
            assert_eq!(cache.pending().unwrap(), pending);
            assert_eq!(cache.status(id).unwrap(), state);
            assert_eq!(cache.snapshot().unwrap(), local);
        }
        drop(cache);
        let cache = Replica::open(&path).unwrap();
        assert_eq!(cache.pending().unwrap(), pending);
        assert!(matches!(
            cache.sync_once(&mut server).unwrap(),
            Some((_, EditStatus::AwaitingConfirmation { .. }))
        ));
        assert_eq!(server.publications, 1);
    }
}
