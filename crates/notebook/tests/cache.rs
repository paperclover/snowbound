use notebook::{Error, Replica};
use onestore::{
    ExGuid, RevisionIndex, Store,
    document::{Document, Kind},
};
use std::{
    collections::BTreeSet,
    fs,
    io::ErrorKind,
    sync::Barrier,
    time::{Duration, Instant},
};

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

#[test]
fn cache_reopen_preserves_exact_images_and_intents() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("section.sqlite");
    let source = onestore::create_section("section.one", "café 🦀", "Fixture").unwrap();
    let replica = Replica::create(&path, &source).unwrap();
    assert_eq!(replica.snapshot().unwrap(), source);
    assert!(replica.pending().unwrap().is_empty());
    let (sid, oid, _) = target(&source);
    assert_eq!(
        replica.edit_text(&source, sid, oid, 0..0, "").unwrap(),
        None
    );
    assert_eq!(
        replica.edit_text(&source, sid, oid, 0..4, "café").unwrap(),
        None
    );
    let first = replica
        .edit_text(&source, sid, oid, 5..7, "🐈 日本語")
        .unwrap()
        .unwrap();
    let edited = replica.snapshot().unwrap();
    assert_eq!(target(&edited).2, "café 🐈 日本語");
    let intents = replica.pending().unwrap();
    assert_eq!(intents.len(), 1);
    assert_eq!(intents[0].id, first);
    let notebook::Operation::Text(first_edit) = &intents[0].operation else {
        panic!()
    };
    assert_eq!(first_edit.before, "café 🦀");
    assert_eq!(first_edit.range, 5..7);
    assert_eq!(first_edit.replacement, "🐈 日本語");
    assert_eq!((intents[0].space, first_edit.object), (sid, oid));
    drop(replica);
    let replica = Replica::open(&path).unwrap();
    assert_eq!(replica.snapshot().unwrap(), edited);
    assert_eq!(replica.pending().unwrap(), intents);
    let second = replica
        .edit_text(&edited, sid, oid, 0..0, "Recovered ")
        .unwrap()
        .unwrap();
    assert!(second > first);
    let notebook::Operation::Text(second_edit) = &replica.pending().unwrap()[1].operation else {
        panic!()
    };
    assert_eq!(second_edit.before, "café 🐈 日本語");
}

#[test]
fn failed_edits_preserve_both_intent_queue_and_working_image() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("section.sqlite");
    let source = onestore::create_section("section.one", "café 🦀", "Fixture").unwrap();
    let replica = Replica::create(&path, &source).unwrap();
    let (sid, oid, _) = target(&source);
    for (range, replacement) in [(6..7, "X"), (0..u32::MAX, "X"), (0..1, "\n")] {
        assert!(
            replica
                .edit_text(&source, sid, oid, range, replacement)
                .is_err()
        );
        assert_eq!(replica.snapshot().unwrap(), source);
        assert!(replica.pending().unwrap().is_empty());
    }
    drop(replica);
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection.execute_batch("CREATE TRIGGER fail_image BEFORE UPDATE ON replica BEGIN SELECT RAISE(ABORT, 'Injected image update failure'); END;").unwrap();
    drop(connection);
    let replica = Replica::open(&path).unwrap();
    assert!(matches!(
        replica.edit_text(&source, sid, oid, 0..0, "lost? "),
        Err(Error::Database(_))
    ));
    drop(replica);
    let replica = Replica::open(&path).unwrap();
    assert_eq!(replica.snapshot().unwrap(), source);
    assert!(replica.pending().unwrap().is_empty());
}

#[test]
fn concurrent_recovery_exports_capture_one_complete_acknowledged_queue() {
    use notebook::{Operation, Recovery};

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
                    loop {
                        let source = replica.snapshot().unwrap();
                        match replica.edit_text(
                            &source,
                            space,
                            object,
                            0..0,
                            &format!("[{writer}:{edit}] "),
                        ) {
                            Ok(Some(_)) => break,
                            Err(Error::Io(error)) if error.kind() == ErrorKind::ResourceBusy => {
                                continue;
                            }
                            other => panic!("Unexpected local edit: {other:?}"),
                        }
                    }
                }
            });
        }
        start.wait();
        for n in 0..12 {
            let path = directory.path().join(format!("recovery-{n}.sqlite"));
            replica.export_recovery(&path).unwrap();
            let archive = Recovery::open(path).unwrap();
            let pending = archive.pending().unwrap();
            let mut expected = String::new();
            for edit in pending.iter().rev() {
                let Operation::Text(edit) = &edit.operation else {
                    panic!()
                };
                assert_eq!(edit.range, 0..0);
                expected.push_str(&edit.replacement);
            }
            expected.push_str("base");
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
        assert_eq!(replica.snapshot().unwrap(), source);
    }
    drop(replica);
    for sql in [
        "PRAGMA application_id=0",
        "PRAGMA application_id=1330529615; PRAGMA user_version=99",
    ] {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection.execute_batch(sql).unwrap();
        drop(connection);
        let before = fs::read(&path).unwrap();
        assert!(Replica::open(&path).is_err());
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
fn twelve_local_editors_reject_stale_ranges_and_preserve_every_acknowledgement() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("section.sqlite");
    let source = onestore::create_section("section.one", "Shared café 🦀", "Fixture").unwrap();
    let replica = Replica::create(&path, &source).unwrap();
    let (sid, oid, _) = target(&source);
    let barrier = Barrier::new(12);
    let deadline = Instant::now() + Duration::from_secs(60);
    let outcomes = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..12)
            .map(|writer| {
                let (replica, source, barrier) = (&replica, &source, &barrier);
                scope.spawn(move || {
                    barrier.wait();
                    let first =
                        replica.edit_text(source, sid, oid, 0..0, &format!("[initial-{writer}] "));
                    let mut ids = Vec::new();
                    let first_won = match first {
                        Ok(Some(id)) => {
                            ids.push(id);
                            true
                        }
                        Err(Error::Io(error)) if error.kind() == ErrorKind::ResourceBusy => false,
                        other => panic!("Unexpected first edit: {other:?}"),
                    };
                    for edit in 0..20 {
                        loop {
                            assert!(
                                Instant::now() < deadline,
                                "Writer {writer} stopped progressing at {edit}"
                            );
                            let source = replica.snapshot().unwrap();
                            match replica.edit_text(
                                &source,
                                sid,
                                oid,
                                0..0,
                                &format!("[{writer}-{edit}] "),
                            ) {
                                Ok(Some(id)) => {
                                    ids.push(id);
                                    break;
                                }
                                Err(Error::Io(error))
                                    if error.kind() == ErrorKind::ResourceBusy => {}
                                other => panic!("Unexpected edit: {other:?}"),
                            }
                        }
                    }
                    (first_won, ids)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(outcomes.iter().filter(|(won, _)| *won).count(), 1);
    let ids: BTreeSet<_> = outcomes.into_iter().flat_map(|(_, ids)| ids).collect();
    assert_eq!(ids.len(), 241);
    let final_bytes = replica.snapshot().unwrap();
    let content = target(&final_bytes).2;
    let mut expected = "Shared café 🦀".to_owned();
    for pending in replica.pending().unwrap() {
        let notebook::Operation::Text(edit) = pending.operation else {
            panic!()
        };
        assert_eq!(edit.before, expected);
        assert_eq!(edit.range, 0..0);
        expected.insert_str(0, &edit.replacement);
    }
    assert_eq!(content, expected);
    for writer in 0..12 {
        for edit in 0..20 {
            assert_eq!(content.matches(&format!("[{writer}-{edit}] ")).count(), 1);
        }
    }
    assert_eq!(
        replica
            .pending()
            .unwrap()
            .iter()
            .map(|edit| edit.id)
            .collect::<BTreeSet<_>>(),
        ids
    );
    assert!(
        matches!(replica.edit_text(&source, sid, oid, 0..0, ""), Err(Error::Io(error)) if error.kind() == ErrorKind::ResourceBusy)
    );
    drop(replica);
    let reopened = Replica::open(&path).unwrap();
    assert_eq!(reopened.snapshot().unwrap(), final_bytes);
    assert_eq!(reopened.pending().unwrap().len(), 241);
}

#[test]
fn seeded_unicode_edits_and_restarts_match_an_independent_text_model() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("model.sqlite");
    let mut text = "ab🚀ab🦀 é repeated repeated".to_owned();
    let mut source = onestore::create_section("model.one", &text, "Fixture").unwrap();
    let mut replica = Replica::create(&path, &source).unwrap();
    let (space, object, _) = target(&source);
    let mut random = 911_u64;
    let mut next = || {
        random ^= random << 13;
        random ^= random >> 7;
        random ^= random << 17;
        random
    };
    let mut intents = Vec::new();
    for step in 0..1024 {
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
        let replacement = ["", "🐈", "日本語", "repeated", "é", "ab🦀ab"][next() as usize % 6];
        let mut expected = text.clone();
        expected.replace_range(bytes, replacement);
        let acknowledgement = replica
            .edit_text(&source, space, object, range.clone(), replacement)
            .unwrap();
        if let Some(id) = acknowledgement {
            assert_ne!(text, expected);
            assert!(
                intents
                    .last()
                    .is_none_or(|edit: &notebook::PendingEdit| edit.id < id)
            );
            intents.push(notebook::PendingEdit {
                id,
                space,
                operation: notebook::Operation::Text(notebook::TextEdit {
                    object,
                    before: text,
                    range,
                    replacement: replacement.into(),
                }),
            });
            assert!(
                matches!(replica.edit_text(&source, space, object, 0..0, "stale"), Err(Error::Io(error)) if error.kind() == ErrorKind::ResourceBusy)
            );
        } else {
            assert_eq!(text, expected);
        }
        text = expected;
        source = replica.snapshot().unwrap();
        assert_eq!(target(&source).2, text, "seed 911, step {step}");
        if step % 37 == 0 {
            drop(replica);
            replica = Replica::open(&path).unwrap();
            assert_eq!(replica.snapshot().unwrap(), source);
            assert_eq!(replica.pending().unwrap(), intents);
        }
    }
    assert!(
        intents.len() > 512,
        "The history checkpoint boundary was not exercised"
    );
    drop(replica);
    let replica = Replica::open(&path).unwrap();
    assert_eq!(replica.pending().unwrap(), intents);
    assert_eq!(replica.snapshot().unwrap(), source);
}

#[test]
#[ignore = "migrates a fresh copy of a retained version-two or version-three cache"]
fn migrate_retained_cache_copy() {
    use notebook::{EditStatus, Operation, PendingEdit, TextEdit};
    let source = std::path::PathBuf::from(std::env::var_os("ONESTORE_MIGRATION_SOURCE").unwrap());
    let output = std::path::PathBuf::from(std::env::var_os("ONESTORE_MIGRATION_OUTPUT").unwrap());
    assert!(source.is_absolute() && output.is_absolute());
    let mut original = fs::File::open(&source).unwrap();
    let mut destination = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&output)
        .unwrap();
    std::io::copy(&mut original, &mut destination).unwrap();
    destination.sync_all().unwrap();
    drop(destination);
    let db = rusqlite::Connection::open(&output).unwrap();
    let version = db
        .pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
        .unwrap();
    assert!(matches!(version, 2 | 3));
    if std::env::var_os("ONESTORE_MIGRATION_ROLLBACK").is_some() {
        assert_eq!(version, 2);
        db.pragma_update(None, "foreign_keys", false).unwrap();
        db.execute(
            "INSERT INTO attempt VALUES (1,999999,'{00000001-0000-0000-0000-000000000000},1')",
            [],
        )
        .unwrap();
        drop(db);
        let before = fs::read(&output).unwrap();
        assert!(
            matches!(Replica::open(&output), Err(Error::Database(rusqlite::Error::SqliteFailure(error, _))) if error.extended_code == 787)
        );
        assert_eq!(fs::read(&output).unwrap(), before);
        let db = rusqlite::Connection::open(&output).unwrap();
        assert_eq!(
            db.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
                .unwrap(),
            2
        );
        println!("migration: rollback preserved {} bytes", before.len());
        return;
    }
    if std::env::var_os("ONESTORE_MIGRATION_CONFLICT").is_some() {
        db.execute("INSERT INTO conflicts SELECT min(id),0 FROM edits", [])
            .unwrap();
    }
    let (base, working): (Vec<u8>, Vec<u8>) = db
        .query_row("SELECT base,working FROM replica", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    let sequence: i64 = db
        .query_row(
            "SELECT seq FROM sqlite_sequence WHERE name='edits'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let pending: Vec<PendingEdit> =
        if version == 2 {
            let mut query = db
        .prepare("SELECT id,space,object,before_text,start,end,replacement FROM edits ORDER BY id")
        .unwrap();
            let pending: Vec<PendingEdit> = query
                .query_map([], |r| {
                    Ok(PendingEdit {
                        id: u64::try_from(r.get::<_, i64>(0)?).unwrap(),
                        space: r.get::<_, String>(1)?.parse().unwrap(),
                        operation: Operation::Text(TextEdit {
                            object: r.get::<_, String>(2)?.parse().unwrap(),
                            before: r.get(3)?,
                            range: r.get(4)?..r.get(5)?,
                            replacement: r.get(6)?,
                        }),
                    })
                })
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            drop(query);
            pending
        } else {
            let mut query = db
                .prepare("SELECT id,space,operation FROM edits ORDER BY id")
                .unwrap();
            query
                .query_map([], |r| {
                    Ok(PendingEdit {
                        id: u64::try_from(r.get::<_, i64>(0)?).unwrap(),
                        space: r.get::<_, String>(1)?.parse().unwrap(),
                        operation: serde_json::from_str(&r.get::<_, String>(2)?).unwrap(),
                    })
                })
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
    let mut states = std::collections::BTreeMap::new();
    for edit in &pending {
        states.insert(edit.id, EditStatus::Pending);
    }
    for table in ["receipts", "attempt"] {
        let mut query = db
            .prepare(&format!("SELECT edit_id,revision FROM {table}"))
            .unwrap();
        for row in query
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
            .unwrap()
        {
            let (id, rid) = row.unwrap();
            let revision = rid.parse().unwrap();
            states.insert(
                u64::try_from(id).unwrap(),
                if table == "receipts" {
                    EditStatus::Published { revision }
                } else {
                    EditStatus::AwaitingConfirmation { revision }
                },
            );
        }
    }
    let mut query = db.prepare("SELECT edit_id,kind FROM conflicts").unwrap();
    for row in query
        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, u32>(1)?)))
        .unwrap()
    {
        let (id, kind) = row.unwrap();
        assert_eq!(kind, 0);
        states.insert(
            u64::try_from(id).unwrap(),
            EditStatus::Conflict(notebook::ConflictKind::TextChanged),
        );
    }
    drop(query);
    drop(db);
    let cache = Replica::open(&output).unwrap();
    assert_eq!(cache.snapshot().unwrap(), working);
    assert_eq!(cache.remote_snapshot().unwrap(), base);
    assert_eq!(cache.pending().unwrap(), pending);
    for (id, status) in &states {
        assert_eq!(cache.status(*id).unwrap(), Some(*status));
    }
    drop(cache);
    let cache = Replica::open(&output).unwrap();
    assert_eq!(cache.pending().unwrap(), pending);
    let store = Store::parse(&working).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let doc = Document::parse(&index).unwrap();
    let (sid, page) = doc.pages().unwrap()[0];
    let insertion = onestore::Insertion::outline(
        page,
        144.0,
        720.0,
        "After cache migration",
        "Migration author",
    )
    .unwrap();
    let next = cache.insert(&working, sid, &insertion).unwrap().unwrap();
    assert_eq!(next, u64::try_from(sequence + 1).unwrap());
    let updated = cache.snapshot().unwrap();
    drop(cache);
    let cache = Replica::open(&output).unwrap();
    assert_eq!(cache.snapshot().unwrap(), updated);
    assert_eq!(
        cache.pending().unwrap().last().unwrap().operation,
        Operation::Insert(insertion)
    );
    println!(
        "migration: {}",
        serde_json::json!({"pending":pending.len(),"retained_statuses":states.len(),"next_id":next,"base_bytes":base.len(),"working_bytes":working.len()})
    );
}

#[test]
fn twelve_local_clients_preserve_inserted_identities_and_dependent_edits() {
    use onestore::Insertion;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("parallel.sqlite");
    let source = onestore::create_section("parallel.one", "Original", "Author").unwrap();
    let store = Store::parse(&source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let doc = Document::parse(&index).unwrap();
    let (sid, page) = doc.pages().unwrap()[0];
    let cache = Replica::create(&path, &source).unwrap();
    let barrier = Barrier::new(12);
    let deadline = Instant::now() + Duration::from_secs(90);
    let all = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..12)
            .map(|client| {
                let (cache, barrier) = (&cache, &barrier);
                scope.spawn(move || {
                    let outline = Insertion::outline(
                        page,
                        72.0,
                        144.0 + client as f32 * 72.0,
                        &format!("Client {client}"),
                        "Author",
                    )
                    .unwrap();
                    let mut ids = Vec::new();
                    let mut objects = Vec::new();
                    barrier.wait();
                    for sequence in 0..4 {
                        let insertion = if sequence == 0 {
                            outline.clone()
                        } else {
                            Insertion::paragraph(
                                outline.object(),
                                None,
                                &format!("Paragraph {client}:{sequence}"),
                                "Author",
                            )
                            .unwrap()
                        }
                        .with_formatting(0..6, &[onestore::TextAttribute::Italic(true)])
                        .unwrap();
                        loop {
                            assert!(
                                Instant::now() < deadline,
                                "Client {client} stopped at insertion {sequence}"
                            );
                            let snapshot = cache.snapshot().unwrap();
                            match cache.insert(&snapshot, sid, &insertion) {
                                Ok(Some(id)) => {
                                    ids.push(id);
                                    objects.push(insertion.text_object());
                                    break;
                                }
                                Err(Error::Io(e)) if e.kind() == ErrorKind::ResourceBusy => {}
                                other => panic!("{other:?}"),
                            }
                        }
                        if sequence == 0 {
                            continue;
                        }
                        loop {
                            assert!(
                                Instant::now() < deadline,
                                "Client {client} stopped at text {sequence}"
                            );
                            let snapshot = cache.snapshot().unwrap();
                            match cache.edit_text(
                                &snapshot,
                                sid,
                                insertion.text_object(),
                                0..0,
                                "Edited ",
                            ) {
                                Ok(Some(id)) => {
                                    ids.push(id);
                                    break;
                                }
                                Err(Error::Io(e)) if e.kind() == ErrorKind::ResourceBusy => {}
                                other => panic!("{other:?}"),
                            }
                        }
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
    let snapshot = cache.snapshot().unwrap();
    drop(cache);
    let cache = Replica::open(&path).unwrap();
    assert_eq!(cache.snapshot().unwrap(), snapshot);
    assert_eq!(
        cache
            .pending()
            .unwrap()
            .iter()
            .map(|e| e.id)
            .collect::<BTreeSet<_>>(),
        ids
    );
    let store = Store::parse(&snapshot).unwrap();
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
fn unrecognized_persisted_operations_are_rejected_without_dropping_fields() {
    for operation in ["Text", "Insert", "Format", "Split", "Join", "Outline"] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("unknown.sqlite");
        let mut source = onestore::create_section("unknown.one", "Original", "Author").unwrap();
        let (sid, oid, _) = target(&source);
        let split = onestore::ParagraphSplit::new(oid, 3, "Author").unwrap();
        if operation == "Join" {
            source = onestore::PreparedEdit::split(&source, sid, &split)
                .unwrap()
                .as_bytes()
                .to_vec();
        }
        let cache = Replica::create(&path, &source).unwrap();
        if operation == "Insert" {
            let store = Store::parse(&source).unwrap();
            let index = RevisionIndex::parse(&store).unwrap();
            let doc = Document::parse(&index).unwrap();
            let (_, page) = doc.pages().unwrap()[0];
            let insertion =
                onestore::Insertion::outline(page, 144.0, 144.0, "Inserted", "Author").unwrap();
            cache.insert(&source, sid, &insertion).unwrap();
        } else if operation == "Outline" {
            let store = Store::parse(&source).unwrap();
            let index = RevisionIndex::parse(&store).unwrap();
            let doc = Document::parse(&index).unwrap();
            let space = &doc.spaces[&sid];
            let view = &space.revisions[&space.contexts[&ExGuid::default()]];
            let paragraph = *view
                .nodes
                .iter()
                .find(|(_, node)| node.content == [oid])
                .unwrap()
                .0;
            cache
                .outline(
                    &source,
                    sid,
                    paragraph,
                    onestore::OutlineEdit::Collapsed(true),
                )
                .unwrap();
        } else if operation == "Text" {
            cache.edit_text(&source, sid, oid, 0..0, "New ").unwrap();
        } else if operation == "Split" {
            cache.split(&source, sid, &split).unwrap();
        } else if operation == "Join" {
            cache
                .join(
                    &source,
                    sid,
                    &onestore::ParagraphJoin::new(oid, split.text_object(), "Author").unwrap(),
                )
                .unwrap();
        } else {
            cache
                .format(
                    &source,
                    sid,
                    oid,
                    0..4,
                    &[onestore::TextAttribute::Bold(true)],
                )
                .unwrap();
        }
        drop(cache);
        let db = rusqlite::Connection::open(&path).unwrap();
        let encoded: String = db
            .query_row("SELECT operation FROM edits", [], |r| r.get(0))
            .unwrap();
        let mut value: serde_json::Value = serde_json::from_str(&encoded).unwrap();
        value[operation]["future_option"] = true.into();
        db.execute("UPDATE edits SET operation=?1", [value.to_string()])
            .unwrap();
        if matches!(operation, "Text" | "Insert") {
            db.execute_batch("ALTER TABLE attempt RENAME COLUMN revisions TO revision; DROP TABLE assets; DROP TABLE conflicts; CREATE TABLE conflicts (edit_id INTEGER PRIMARY KEY REFERENCES edits(id) ON DELETE CASCADE, kind INTEGER NOT NULL CHECK(kind BETWEEN 0 AND 2)) STRICT; PRAGMA user_version=3;").unwrap();
        }
        drop(db);
        let before = fs::read(&path).unwrap();
        assert!(
            matches!(Replica::open(&path),Err(Error::Io(error))if error.kind()==ErrorKind::InvalidData)
        );
        assert_eq!(fs::read(&path).unwrap(), before);
    }
}

#[test]
fn prior_schema_migrations_retain_queue_evidence_assets_and_enable_content_conflicts() {
    for (version, ceiling) in [(5, 3), (6, 4), (7, 5), (8, 6), (9, 6)] {
        use notebook::{ConflictKind, EditStatus, Recovery};
        use sha2::{Digest, Sha256};
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("previous.sqlite");
        let source = onestore::create_section("migration.one", "Original", "Author").unwrap();
        let (sid, oid, _) = target(&source);
        let cache = Replica::create(&path, &source).unwrap();
        let first = cache
            .edit_text(&source, sid, oid, 0..0, "New ")
            .unwrap()
            .unwrap();
        let second = cache
            .format(
                &cache.snapshot().unwrap(),
                sid,
                oid,
                0..3,
                &[onestore::TextAttribute::Bold(true)],
            )
            .unwrap()
            .unwrap();
        let queue = cache.pending().unwrap();
        let local = cache.snapshot().unwrap();
        let store = Store::parse(&local).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let revision = index.spaces[&sid].labels[&(ExGuid::default(), 1)];
        drop(cache);
        let db = rusqlite::Connection::open(&path).unwrap();
        let current_version: u32 = db
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        let asset = "00112233-4455-6677-8899-aabbccddeeff.onebin";
        let data = b"retained media";
        db.execute(
            "INSERT INTO assets VALUES (?1,?2,?3)",
            rusqlite::params![asset, data.as_slice(), Sha256::digest(data).as_slice()],
        )
        .unwrap();
        db.execute_batch(&format!(
            "ALTER TABLE attempt RENAME COLUMN revisions TO revision;
            DROP TABLE conflicts; CREATE TABLE conflicts (
            edit_id INTEGER PRIMARY KEY REFERENCES edits(id) ON DELETE CASCADE,
            kind INTEGER NOT NULL CHECK(kind BETWEEN 0 AND {ceiling})) STRICT;
            PRAGMA user_version={version};
            UPDATE sqlite_sequence SET seq=1000 WHERE name='edits';"
        ))
        .unwrap();
        db.execute(
            "INSERT INTO conflicts VALUES (?1,3)",
            [i64::try_from(second).unwrap()],
        )
        .unwrap();
        db.execute(
            "INSERT INTO attempt VALUES (1,?1,?2)",
            rusqlite::params![i64::try_from(first).unwrap(), revision.to_string()],
        )
        .unwrap();
        db.execute(
            "INSERT INTO receipts VALUES (999,?1)",
            [revision.to_string()],
        )
        .unwrap();
        drop(db);
        let archive = directory.path().join("legacy-recovery.sqlite");
        std::fs::copy(&path, &archive).unwrap();
        let archive_db = rusqlite::Connection::open(&archive).unwrap();
        archive_db
            .pragma_update(None, "application_id", 0x4f4e4552)
            .unwrap();
        drop(archive_db);
        let original_archive = std::fs::read(&archive).unwrap();
        let recovery = Recovery::open(&archive).unwrap();
        assert_eq!(
            recovery.status(first).unwrap(),
            Some(EditStatus::AwaitingConfirmation { revision })
        );
        assert_eq!(recovery.pending().unwrap(), queue);
        drop(recovery);
        assert!(std::fs::read(&archive).unwrap() == original_archive);
        let cache = Replica::open(&path).unwrap();
        assert_eq!(cache.pending().unwrap(), queue);
        assert_eq!(
            cache.cached_asset(asset, 1024).unwrap(),
            Some(data.to_vec())
        );
        assert!(cache.snapshot().unwrap() == local);
        assert!(cache.remote_snapshot().unwrap() == source);
        assert_eq!(
            cache.status(first).unwrap(),
            Some(EditStatus::AwaitingConfirmation { revision })
        );
        assert_eq!(
            cache.status(second).unwrap(),
            Some(EditStatus::Conflict(ConflictKind::FormattingChanged))
        );
        assert_eq!(
            cache.status(999).unwrap(),
            Some(EditStatus::Published { revision })
        );
        let split = onestore::ParagraphSplit::new(oid, 3, "Author").unwrap();
        assert_eq!(cache.split(&local, sid, &split).unwrap(), Some(1001));
        let pending = cache.pending().unwrap();
        let archive = directory.path().join("recovery.sqlite");
        cache.export_recovery(&archive).unwrap();
        let recovery = Recovery::open(&archive).unwrap();
        assert_eq!(recovery.pending().unwrap(), pending);
        assert_eq!(
            recovery.cached_asset(asset, 1024).unwrap(),
            Some(data.to_vec())
        );
        assert_eq!(
            recovery.status(first).unwrap(),
            cache.status(first).unwrap()
        );
        assert_eq!(recovery.receipts().unwrap().get(&999), Some(&revision));
        drop(cache);
        let db = rusqlite::Connection::open(&path).unwrap();
        assert_eq!(
            db.pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
                .unwrap(),
            current_version
        );
        db.execute("INSERT INTO conflicts VALUES (1001,6)", [])
            .unwrap();
        drop(db);
        let cache = Replica::open(&path).unwrap();
        assert_eq!(
            cache.status(1001).unwrap(),
            Some(EditStatus::Conflict(ConflictKind::ContentChanged))
        );
    }
}
