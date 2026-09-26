#[path = "../../onestore/tests/support/ops.rs"]
mod ops;
use notebook::{Error, Replica};
use onestore::{
    ExGuid, RevisionIndex, Store,
    document::{Document, Kind},
    op::{Edit, Op, PageOp, SectionOp},
    page::{Outline, PageObject},
};
use std::{
    collections::BTreeSet,
    fs,
    io::ErrorKind,
    sync::Barrier,
    time::{Duration, Instant},
};

#[path = "support/model_ops.rs"]
mod model_ops;
#[path = "support/server.rs"]
mod server;
use server::snapshot;

fn target(source: &[u8]) -> (ExGuid, ExGuid, String) {
    let store = Store::parse(source).unwrap();
    assert!(store.checksum_mismatches.is_empty());
    let index = RevisionIndex::parse(&store).unwrap();
    index.validate_current().unwrap();
    let doc = Document::parse(&index).unwrap();
    doc.spaces
        .iter()
        .find_map(|(sid, space)| {
            let revision = &space.revisions[&space.contexts[&ExGuid::default()]];
            revision
                .nodes
                .iter()
                .find_map(|(oid, node)| match &node.kind {
                    Kind::RichText { text, .. } => Some((*sid, *oid, text.clone())),
                    _ => None,
                })
        })
        .unwrap()
}

/// An edit typing `with` at the start of a text.
fn typed(space: ExGuid, text: ExGuid, with: &str) -> Edit {
    Edit {
        at: model_ops::now(),
        ops: vec![Op::Page {
            space,
            op: PageOp::Text {
                text,
                range: 0..0,
                with: with.into(),
            },
        }],
    }
}

#[test]
fn cache_reopen_preserves_the_base_and_queued_edits() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("section.sqlite");
    let source = onestore::create_section("section.one", "café 🦀", "Fixture").unwrap();
    let replica = Replica::create(&path, &source).unwrap();
    assert_eq!(snapshot(&replica), source);
    assert!(replica.pending().unwrap().is_empty());
    let (sid, oid, _) = target(&source);
    let unchanged = |page: &mut onestore::page::Page| {
        model_ops::replace_text(page, oid, 0..0, "");
        model_ops::replace_text(page, oid, 0..4, "café");
    };
    assert_eq!(model_ops::save(&replica, oid, unchanged).unwrap(), None);
    let first = model_ops::save(&replica, oid, |page| {
        model_ops::replace_text(page, oid, 5..7, "🐈 日本語")
    })
    .unwrap()
    .unwrap();
    assert_eq!(target(&snapshot(&replica)).2, "café 🐈 日本語");
    let edits = replica.pending().unwrap();
    assert_eq!(edits.len(), 1);
    assert_eq!(
        (edits[0].id, edits[0].author.as_str()),
        (first, model_ops::AUTHOR)
    );
    assert!(matches!(
        &edits[0].edit.ops[..],
        [Op::Page { space, op: PageOp::Text { text, range, with } }]
            if (*space, *text, range.clone(), with.as_str()) == (sid, oid, 5..7, "🐈 日本語")
    ));
    let beside = |suffix: &str| {
        let mut file = path.as_os_str().to_owned();
        file.push(suffix);
        std::path::PathBuf::from(file)
    };
    // Commits go to the write-ahead log; its index stays in memory.
    assert!(beside("-wal").exists());
    assert!(!beside("-shm").exists());
    drop(replica);
    assert!(
        !beside("-wal").exists(),
        "closing checkpoints and removes the log"
    );
    let replica = Replica::open(&path).unwrap();
    assert_eq!(target(&snapshot(&replica)).2, "café 🐈 日本語");
    assert_eq!(replica.pending().unwrap(), edits);
    let second = replica
        .apply(model_ops::AUTHOR, typed(sid, oid, "Recovered "))
        .unwrap();
    assert!(second > first);
    assert_eq!(
        target(&snapshot(&replica)).2,
        "Recovered café 🐈 日本語"
    );
    assert_eq!(replica.pending().unwrap().len(), 2);
}

#[test]
fn refused_edits_and_failed_writes_leave_the_queue_as_it_was() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("section.sqlite");
    let source = onestore::create_section("section.one", "café 🦀", "Fixture").unwrap();
    let replica = Replica::create(&path, &source).unwrap();
    let (sid, oid, _) = target(&source);
    let empty = Outline {
        id: onestore::page::text::new_id().unwrap(),
        title: false,
        min_width: None,
        layout: onestore::document::Layout {
            x: Some(72.0),
            y: Some(400.0),
            ..Default::default()
        },
        indents: Vec::new(),
        paragraphs: Vec::new(),
        unsupported: Vec::new(),
    };
    let refused = Edit {
        at: model_ops::now(),
        ops: vec![
            Op::Page {
                space: sid,
                op: PageOp::Text {
                    text: oid,
                    range: 0..0,
                    with: "Undone ".into(),
                },
            },
            Op::Page {
                space: sid,
                op: PageOp::Add {
                    object: PageObject::Outline(empty),
                    before: None,
                },
            },
        ],
    };
    assert!(matches!(
        replica.apply("Author", refused),
        Err(Error::Rejected(_))
    ));
    assert!(matches!(
        replica.apply("Author", typed(ExGuid::default(), oid, "Elsewhere ")),
        Err(Error::Rejected(_))
    ));
    assert_eq!(snapshot(&replica), source);
    assert!(replica.pending().unwrap().is_empty());
    drop(replica);
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection.execute_batch("CREATE TRIGGER fail_edit BEFORE INSERT ON edits BEGIN SELECT RAISE(ABORT, 'Injected queue failure'); END;").unwrap();
    drop(connection);
    let replica = Replica::open(&path).unwrap();
    assert!(replica.apply("Author", typed(sid, oid, "lost? ")).is_err());
    assert_eq!(snapshot(&replica), source);
    drop(replica);
    let replica = Replica::open(&path).unwrap();
    assert_eq!(snapshot(&replica), source);
    assert!(replica.pending().unwrap().is_empty());
}

#[test]
fn concurrent_recovery_exports_capture_one_complete_acknowledged_queue() {
    use notebook::Recovery;

    let directory = tempfile::tempdir().unwrap();
    let source = onestore::create_section("recovery.one", "base", "Fixture").unwrap();
    let (space, object, _) = target(&source);
    let replica = Replica::create(directory.path().join("live.sqlite"), &source).unwrap();
    let start = Barrier::new(4);
    std::thread::scope(|scope| {
        for writer in 0..3 {
            let (replica, start) = (&replica, &start);
            scope.spawn(move || {
                start.wait();
                for edit in 0..20 {
                    replica
                        .apply(
                            "Fixture",
                            typed(space, object, &format!("[{writer}:{edit}] ")),
                        )
                        .unwrap();
                }
            });
        }
        start.wait();
        for n in 0..12 {
            let path = directory.path().join(format!("recovery-{n}.sqlite"));
            replica.export_recovery(&path).unwrap();
            let archive = Recovery::open(path).unwrap();
            let pending = archive.pending().unwrap();
            // Each edit types at the start, so the text is the queue read backwards.
            let expected: String = pending
                .iter()
                .rev()
                .map(|edit| match &edit.edit.ops[..] {
                    [
                        Op::Page {
                            op: PageOp::Text { with, .. },
                            ..
                        },
                    ] => with.as_str(),
                    other => panic!("{other:?}"),
                })
                .chain(["base"])
                .collect();
            assert_eq!(target(&archive.snapshot().unwrap()).2, expected);
            assert_eq!(archive.remote_snapshot().unwrap(), source);
            assert_eq!(
                archive.summary().unwrap().queued_edits,
                pending.len() as u64
            );
            assert!(archive.receipts().unwrap().is_empty());
        }
    });
    assert_eq!(replica.pending().unwrap().len(), 60);
    let content = target(&snapshot(&replica)).2;
    for writer in 0..3 {
        for edit in 0..20 {
            assert_eq!(content.matches(&format!("[{writer}:{edit}] ")).count(), 1);
        }
    }
}

#[test]
fn ownership_and_foreign_file_rejection_preserve_existing_data() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("section.sqlite");
    assert!(Replica::open(&path).is_err());
    assert!(!path.exists());
    assert!(Replica::create(&path, b"invalid").is_err());
    assert!(!path.exists());
    let source = onestore::create_section("section.one", "Owned", "Fixture").unwrap();
    let replica = Replica::create(&path, &source).unwrap();
    for _ in 0..3 {
        assert!(
            matches!(Replica::open(&path), Err(Error::Database(error)) if error.sqlite_error_code() == Some(rusqlite::ErrorCode::DatabaseBusy))
        );
        assert!(
            matches!(Replica::create(&path, &source), Err(Error::Io(error)) if error.kind() == ErrorKind::AlreadyExists)
        );
        assert_eq!(snapshot(&replica), source);
    }
    drop(replica);
    let current: u32 = rusqlite::Connection::open(&path)
        .unwrap()
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    for sql in [
        "PRAGMA application_id=0".to_owned(),
        // Schemas 14 and 15 convert; older caches do not.
        format!(
            "PRAGMA application_id=1330529615; PRAGMA user_version={}",
            current - 3
        ),
        format!("PRAGMA user_version={}", current + 1),
    ] {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection.execute_batch(&sql).unwrap();
        drop(connection);
        let before = fs::read(&path).unwrap();
        let Err(Error::Io(error)) = Replica::open(&path) else {
            panic!("{sql}")
        };
        assert_eq!(error.kind(), ErrorKind::InvalidData, "{sql}");
        if sql.contains("user_version") {
            let written = sql.rsplit('=').next().unwrap();
            let message = error.to_string();
            assert!(
                message.contains(&format!("version {written} "))
                    && message.contains(&format!("version {current}")),
                "{message}"
            );
        }
        assert!(
            matches!(notebook::Recovery::open(&path), Err(Error::Io(error)) if error.kind() == ErrorKind::InvalidData),
            "{sql}"
        );
        assert_eq!(fs::read(&path).unwrap(), before);
    }
    let foreign = dir.path().join("foreign.sqlite");
    let connection = rusqlite::Connection::open(&foreign).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE unrelated (value TEXT); INSERT INTO unrelated VALUES ('preserve');",
        )
        .unwrap();
    drop(connection);
    let before = fs::read(&foreign).unwrap();
    assert!(Replica::open(&foreign).is_err());
    assert_eq!(fs::read(&foreign).unwrap(), before);
    let incomplete = dir.path().join("incomplete.sqlite");
    fs::write(&incomplete, []).unwrap();
    assert!(Replica::open(&incomplete).is_err());
    assert_eq!(fs::read(&incomplete).unwrap(), b"");
}

#[test]
fn twelve_local_editors_queue_every_edit_once_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("section.sqlite");
    let source = onestore::create_section("section.one", "Shared café 🦀", "Fixture").unwrap();
    let replica = Replica::create(&path, &source).unwrap();
    let (sid, oid, _) = target(&source);
    let barrier = Barrier::new(12);
    let outcomes = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..12)
            .map(|writer| {
                let (replica, barrier) = (&replica, &barrier);
                scope.spawn(move || {
                    barrier.wait();
                    (0..20)
                        .map(|edit| {
                            replica
                                .apply("Fixture", typed(sid, oid, &format!("[{writer}-{edit}] ")))
                                .unwrap()
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>()
    });
    for ids in &outcomes {
        assert!(
            ids.windows(2).all(|pair| pair[0] < pair[1]),
            "a writer's edits keep its order"
        );
    }
    let ids: BTreeSet<_> = outcomes.into_iter().flatten().collect();
    assert_eq!(ids.len(), 240);
    let content = target(&snapshot(&replica)).2;
    assert!(content.ends_with("Shared café 🦀"));
    for writer in 0..12 {
        for edit in 0..20 {
            assert_eq!(content.matches(&format!("[{writer}-{edit}] ")).count(), 1);
        }
    }
    let pending = replica.pending().unwrap();
    assert_eq!(
        pending.iter().map(|edit| edit.id).collect::<BTreeSet<_>>(),
        ids
    );
    drop(replica);
    let reopened = Replica::open(&path).unwrap();
    assert_eq!(target(&snapshot(&reopened)).2, content);
    assert_eq!(reopened.pending().unwrap(), pending);
}

#[test]
fn seeded_unicode_edits_and_restarts_match_an_independent_text_model() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("model.sqlite");
    let mut text = "ab🚀ab🦀 é repeated repeated".to_owned();
    let source = onestore::create_section("model.one", &text, "Fixture").unwrap();
    let mut replica = Replica::create(&path, &source).unwrap();
    let (_, object, _) = target(&source);
    let mut random = 911_u64;
    let mut next = || {
        random ^= random << 13;
        random ^= random >> 7;
        random ^= random << 17;
        random
    };
    let mut acknowledged = Vec::new();
    for step in 0..512 {
        let boundaries: Vec<_> = text
            .char_indices()
            .map(|(at, _)| at)
            .chain([text.len()])
            .collect();
        let first = boundaries[next() as usize % boundaries.len()];
        let last = boundaries[next() as usize % boundaries.len()];
        let bytes = first.min(last)..first.max(last);
        let range = u32::try_from(text[..bytes.start].encode_utf16().count()).unwrap()
            ..u32::try_from(text[..bytes.end].encode_utf16().count()).unwrap();
        let replacement = ["", "🐈", "日本語", "repeated", "é", "ab🦀ab"][next() as usize % 6];
        let mut expected = text.clone();
        expected.replace_range(bytes, replacement);
        let acknowledgement = model_ops::save(&replica, object, |page| {
            model_ops::replace_text(page, object, range.clone(), replacement)
        })
        .unwrap();
        match acknowledgement {
            Some(id) => {
                assert_ne!(text, expected);
                assert!(acknowledged.last().is_none_or(|last| *last < id));
                acknowledged.push(id);
            }
            None => assert_eq!(text, expected),
        }
        text = expected;
        assert_eq!(
            target(&snapshot(&replica)).2,
            text,
            "seed 911, step {step}"
        );
        if step % 37 == 0 {
            let pending = replica.pending().unwrap();
            drop(replica);
            replica = Replica::open(&path).unwrap();
            assert_eq!(target(&snapshot(&replica)).2, text);
            assert_eq!(replica.pending().unwrap(), pending);
        }
    }
    assert!(
        acknowledged.len() > 256,
        "Most random edits change the text"
    );
    assert_eq!(
        replica
            .pending()
            .unwrap()
            .iter()
            .map(|edit| edit.id)
            .collect::<Vec<_>>(),
        acknowledged
    );
}

#[test]
fn twelve_local_clients_preserve_inserted_identities_and_dependent_edits() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("parallel.sqlite");
    let source = onestore::create_section("parallel.one", "Original", "Author").unwrap();
    let (sid, anchor, _) = target(&source);
    let cache = Replica::create(&path, &source).unwrap();
    let barrier = Barrier::new(12);
    let deadline = Instant::now() + Duration::from_secs(90);
    let italic = |format: &mut onestore::document::Format| format.italic = Some(true);
    let all = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..12)
            .map(|client| {
                let (cache, barrier) = (&cache, &barrier);
                scope.spawn(move || {
                    let mut ids = Vec::new();
                    let mut objects = Vec::new();
                    barrier.wait();
                    for sequence in 0..4 {
                        assert!(Instant::now() < deadline, "Client {client} stopped");
                        let content = if sequence == 0 {
                            format!("Client {client}")
                        } else {
                            format!("Paragraph {client}:{sequence}")
                        };
                        let mut inserted = None;
                        let id = model_ops::save(cache, anchor, |page| {
                            let text = if sequence == 0 {
                                model_ops::insert_outline(
                                    page,
                                    72.0,
                                    144.0 + client as f32 * 72.0,
                                    &content,
                                )
                                .2
                            } else {
                                model_ops::insert_after(page, *objects.last().unwrap(), &content).1
                            };
                            let end = content.encode_utf16().count() as u32;
                            model_ops::restyle(page, text, 0..end, |format| format.italic = None);
                            model_ops::restyle(page, text, 0..6, italic);
                            inserted = Some(text);
                        })
                        .unwrap()
                        .unwrap();
                        ids.push(id);
                        objects.push(inserted.unwrap());
                        if sequence == 0 {
                            continue;
                        }
                        let text = *objects.last().unwrap();
                        ids.push(
                            cache
                                .apply(model_ops::AUTHOR, typed(sid, text, "Edited "))
                                .unwrap(),
                        );
                    }
                    (ids, objects)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .collect::<Vec<_>>()
    });
    let ids: BTreeSet<_> = all
        .iter()
        .flat_map(|(ids, _)| ids.iter().copied())
        .collect();
    assert_eq!(ids.len(), 84);
    let image = snapshot(&cache);
    drop(cache);
    let cache = Replica::open(&path).unwrap();
    assert_eq!(server::pages(&snapshot(&cache)), server::pages(&image));
    assert_eq!(
        cache
            .pending()
            .unwrap()
            .iter()
            .map(|e| e.id)
            .collect::<BTreeSet<_>>(),
        ids
    );
    let store = Store::parse(&image).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let doc = Document::parse(&index).unwrap();
    let s = &doc.spaces[&sid];
    let v = &s.revisions[&s.contexts[&ExGuid::default()]];
    for (client, (_, objects)) in all.iter().enumerate() {
        for (sequence, id) in objects.iter().enumerate() {
            let wanted = if sequence == 0 {
                format!("Client {client}")
            } else {
                format!("Edited Paragraph {client}:{sequence}")
            };
            assert!(matches!(&v.nodes[id].kind,Kind::RichText{text,..} if *text==wanted));
            let runs = v.text_runs(*id).unwrap();
            assert_eq!(runs[0].format.italic, Some(true));
            assert_eq!(
                runs[0].text,
                if sequence == 0 {
                    "Client"
                } else {
                    "Edited Paragr"
                }
            );
            assert!(runs[1..].iter().all(|run| run.format.italic != Some(true)));
        }
    }
}

#[test]
fn unrecognized_persisted_ops_are_rejected_without_dropping_fields() {
    for kind in ["Text", "Create", "Pages", "Delete"] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("unknown.sqlite");
        let first = onestore::create_section("unknown.one", "Original", "Author").unwrap();
        let second = onestore::PageCreation::new(None, Some("Second"), "Author").unwrap();
        let source = ops::section_op(&first, SectionOp::Create(second.clone()))
            .unwrap()
            .as_bytes()
            .to_vec();
        let (space, oid, _) = target(&source);
        let store = Store::parse(&source).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let sid = Document::parse(&index).unwrap().pages().unwrap()[1].0;
        let cache = Replica::create(&path, &source).unwrap();
        let section = |op| {
            server::section_op(&cache, op);
        };
        match kind {
            "Text" => {
                cache.apply("Author", typed(space, oid, "New ")).unwrap();
            }
            "Create" => section(SectionOp::Create(
                onestore::PageCreation::new(None, Some("Created"), "Author").unwrap(),
            )),
            "Pages" => section(SectionOp::Pages(vec![
                onestore::PageEdit::set_level(sid, 2).unwrap(),
            ])),
            _ => section(SectionOp::Delete(vec![sid])),
        }
        drop(cache);
        let encoded: String = rusqlite::Connection::open(&path)
            .unwrap()
            .query_row("SELECT edit FROM edits", [], |r| r.get(0))
            .unwrap();
        for target in ["", "/ops/0", "/ops/0/Page/op/Text", "/ops/0/Section"] {
            let mut value: serde_json::Value = serde_json::from_str(&encoded).unwrap();
            let Some(object) = value.pointer_mut(target).and_then(|v| v.as_object_mut()) else {
                continue;
            };
            object.insert("future_option".into(), true.into());
            rusqlite::Connection::open(&path)
                .unwrap()
                .execute("UPDATE edits SET edit=?1", [value.to_string()])
                .unwrap();
            let before = fs::read(&path).unwrap();
            assert!(
                matches!(Replica::open(&path),Err(Error::Io(error))if error.kind()==ErrorKind::InvalidData),
                "{kind} {target}"
            );
            assert_eq!(fs::read(&path).unwrap(), before);
        }
    }
}
