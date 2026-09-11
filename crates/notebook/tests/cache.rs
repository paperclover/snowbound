use notebook::{Error, Operation, Replica};
use onestore::{
    ExGuid, RevisionIndex, Store,
    document::{Document, Kind},
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

fn text_of(page: &onestore::page::Page, text: ExGuid) -> String {
    model_ops::paragraph_with(page, text)
        .unwrap()
        .text()
        .unwrap()
        .text
        .text()
        .to_owned()
}

fn intent(replica: &Replica, at: usize) -> notebook::PageIntent {
    match &replica.pending().unwrap()[at].operation {
        Operation::Page(intent) => intent.clone(),
        other => panic!("{other:?}"),
    }
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
    let edited = replica.snapshot().unwrap();
    assert_eq!(target(&edited).2, "café 🐈 日本語");
    let intents = replica.pending().unwrap();
    assert_eq!(intents.len(), 1);
    assert_eq!((intents[0].id, intents[0].space), (first, sid));
    let first_intent = intent(&replica, 0);
    assert_eq!(
        first_intent.text_change(),
        Some((oid, "café 🦀".into(), 5..7, "🐈 日本語".into()))
    );
    assert_eq!(first_intent.author, model_ops::AUTHOR);
    drop(replica);
    let replica = Replica::open(&path).unwrap();
    assert_eq!(replica.snapshot().unwrap(), edited);
    assert_eq!(replica.pending().unwrap(), intents);
    let second = model_ops::save(&replica, oid, |page| {
        model_ops::replace_text(page, oid, 0..0, "Recovered ")
    })
    .unwrap()
    .unwrap();
    assert_eq!(second, first, "an unattempted save is replaced in place");
    let coalesced = intent(&replica, 0);
    assert_eq!(coalesced.before, first_intent.before);
    assert_eq!(text_of(&coalesced.after, oid), "Recovered café 🐈 日本語");
    assert_eq!(
        target(&replica.snapshot().unwrap()).2,
        "Recovered café 🐈 日本語"
    );
}

#[test]
fn failed_saves_preserve_both_intent_queue_and_working_image() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("section.sqlite");
    let source = onestore::create_section("section.one", "café 🦀", "Fixture").unwrap();
    let replica = Replica::create(&path, &source).unwrap();
    let (sid, oid, _) = target(&source);
    let (_, mut page) = model_ops::locate(&source, oid);
    let mut empty = Outline {
        id: onestore::page::text::new_id().unwrap(),
        title: false,
        min_width: None,
        layout: Default::default(),
        indents: Vec::new(),
        paragraphs: Vec::new(),
        unsupported: Vec::new(),
    };
    empty.layout.x = Some(72.0);
    empty.layout.y = Some(400.0);
    page.objects.push(PageObject::Outline(empty));
    assert!(replica.save(&source, sid, &page, "Author").is_err());
    let (_, mut page) = model_ops::locate(&source, oid);
    model_ops::replace_text(&mut page, oid, 0..0, "Elsewhere ");
    let foreign = ExGuid::default();
    assert!(replica.save(&source, foreign, &page, "Author").is_err());
    assert_eq!(replica.snapshot().unwrap(), source);
    assert!(replica.pending().unwrap().is_empty());
    drop(replica);
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection.execute_batch("CREATE TRIGGER fail_image BEFORE UPDATE ON replica BEGIN SELECT RAISE(ABORT, 'Injected image update failure'); END;").unwrap();
    drop(connection);
    let replica = Replica::open(&path).unwrap();
    assert!(matches!(
        model_ops::save(&replica, oid, |page| model_ops::replace_text(
            page,
            oid,
            0..0,
            "lost? "
        )),
        Err(Error::Database(_))
    ));
    drop(replica);
    let replica = Replica::open(&path).unwrap();
    assert_eq!(replica.snapshot().unwrap(), source);
    assert!(replica.pending().unwrap().is_empty());
}

#[test]
fn concurrent_recovery_exports_capture_one_complete_acknowledged_queue() {
    use notebook::Recovery;

    let directory = tempfile::tempdir().unwrap();
    let source = onestore::create_section("recovery.one", "base", "Fixture").unwrap();
    let (_, object, _) = target(&source);
    let replica = Replica::create(directory.path().join("live.sqlite"), &source).unwrap();
    let start = Barrier::new(4);
    std::thread::scope(|scope| {
        for writer in 0..3 {
            let (replica, start) = (&replica, &start);
            scope.spawn(move || {
                start.wait();
                for edit in 0..20 {
                    loop {
                        let marker = format!("[{writer}:{edit}] ");
                        match model_ops::save(replica, object, |page| {
                            model_ops::replace_text(page, object, 0..0, &marker)
                        }) {
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
            let snapshot = target(&archive.snapshot().unwrap()).2;
            match pending.as_slice() {
                [] => assert_eq!(snapshot, "base"),
                [single] => {
                    let Operation::Page(intent) = &single.operation else {
                        panic!()
                    };
                    let (id, before, range, replacement) = intent.text_change().unwrap();
                    assert_eq!((id, before.as_str(), range), (object, "base", 0..0));
                    assert_eq!(snapshot, format!("{replacement}base"));
                    assert_eq!(text_of(&intent.after, object), snapshot);
                }
                more => panic!("saves to one page coalesce: {more:?}"),
            }
            assert_eq!(archive.remote_snapshot().unwrap(), source);
            assert_eq!(
                archive.summary().unwrap().queued_edits,
                pending.len() as u64
            );
            assert!(archive.receipts().unwrap().is_empty());
        }
    });
    assert_eq!(replica.pending().unwrap().len(), 1);
    let content = target(&replica.snapshot().unwrap()).2;
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
        assert_eq!(replica.snapshot().unwrap(), source);
    }
    drop(replica);
    let current: u32 = rusqlite::Connection::open(&path)
        .unwrap()
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    for sql in [
        "PRAGMA application_id=0".to_owned(),
        format!(
            "PRAGMA application_id=1330529615; PRAGMA user_version={}",
            current - 1
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
fn twelve_local_editors_reject_stale_images_and_coalesce_into_one_save() {
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
                    let (_, mut page) = model_ops::locate(source, oid);
                    model_ops::replace_text(&mut page, oid, 0..0, &format!("[initial-{writer}] "));
                    barrier.wait();
                    let mut ids = Vec::new();
                    let first_won = match replica.save(source, sid, &page, model_ops::AUTHOR) {
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
                            let marker = format!("[{writer}-{edit}] ");
                            match model_ops::save(replica, oid, |page| {
                                model_ops::replace_text(page, oid, 0..0, &marker)
                            }) {
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
    assert_eq!(ids.len(), 1, "every save replaced the unattempted head");
    let final_bytes = replica.snapshot().unwrap();
    let content = target(&final_bytes).2;
    let pending = replica.pending().unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].id, *ids.first().unwrap());
    let (id, before, range, replacement) = intent(&replica, 0).text_change().unwrap();
    assert_eq!((id, before.as_str(), range), (oid, "Shared café 🦀", 0..0));
    assert_eq!(content, format!("{replacement}Shared café 🦀"));
    assert_eq!(content.matches("[initial-").count(), 1);
    for writer in 0..12 {
        for edit in 0..20 {
            assert_eq!(content.matches(&format!("[{writer}-{edit}] ")).count(), 1);
        }
    }
    let (_, mut stale) = model_ops::locate(&source, oid);
    model_ops::replace_text(&mut stale, oid, 0..0, "stale ");
    assert!(
        matches!(replica.save(&source, sid, &stale, "Author"), Err(Error::Io(error)) if error.kind() == ErrorKind::ResourceBusy)
    );
    drop(replica);
    let reopened = Replica::open(&path).unwrap();
    assert_eq!(reopened.snapshot().unwrap(), final_bytes);
    assert_eq!(reopened.pending().unwrap(), pending);
}

#[test]
fn seeded_unicode_edits_and_restarts_match_an_independent_text_model() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("model.sqlite");
    let original = "ab🚀ab🦀 é repeated repeated".to_owned();
    let mut text = original.clone();
    let source = onestore::create_section("model.one", &text, "Fixture").unwrap();
    let mut replica = Replica::create(&path, &source).unwrap();
    let (space, object, _) = target(&source);
    let mut random = 911_u64;
    let mut next = || {
        random ^= random << 13;
        random ^= random >> 7;
        random ^= random << 17;
        random
    };
    let mut acknowledged = 0;
    let mut pending = Vec::new();
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
        let before = replica.snapshot().unwrap();
        let acknowledgement = model_ops::save(&replica, object, |page| {
            model_ops::replace_text(page, object, range.clone(), replacement)
        })
        .unwrap();
        if let Some(id) = acknowledgement {
            assert_ne!(text, expected);
            acknowledged += 1;
            pending = replica.pending().unwrap();
            assert_eq!(pending.len(), 1);
            assert_eq!((pending[0].id, pending[0].space), (1, space));
            let saved = intent(&replica, 0);
            assert_eq!(id, 1);
            assert_eq!(text_of(&saved.before, object), original);
            assert_eq!(text_of(&saved.after, object), expected);
            if step % 37 == 0 {
                let (_, mut stale) = model_ops::locate(&before, object);
                model_ops::replace_text(&mut stale, object, 0..0, "stale");
                assert!(
                    matches!(replica.save(&before, space, &stale, "Author"), Err(Error::Io(error)) if error.kind() == ErrorKind::ResourceBusy)
                );
            }
        } else {
            assert_eq!(text, expected);
        }
        text = expected;
        let source = replica.snapshot().unwrap();
        assert_eq!(target(&source).2, text, "seed 911, step {step}");
        if step % 37 == 0 {
            drop(replica);
            replica = Replica::open(&path).unwrap();
            assert_eq!(replica.snapshot().unwrap(), source);
            assert_eq!(replica.pending().unwrap(), pending);
        }
    }
    assert!(acknowledged > 256, "Most random edits change the text");
    let source = replica.snapshot().unwrap();
    drop(replica);
    let replica = Replica::open(&path).unwrap();
    assert_eq!(replica.pending().unwrap(), pending);
    assert_eq!(replica.snapshot().unwrap(), source);
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
                        let content = if sequence == 0 {
                            format!("Client {client}")
                        } else {
                            format!("Paragraph {client}:{sequence}")
                        };
                        let mut inserted = None;
                        loop {
                            assert!(
                                Instant::now() < deadline,
                                "Client {client} stopped at insertion {sequence}"
                            );
                            let result = model_ops::save(cache, anchor, |page| {
                                let text = if sequence == 0 {
                                    model_ops::insert_outline(
                                        page,
                                        72.0,
                                        144.0 + client as f32 * 72.0,
                                        &content,
                                    )
                                    .2
                                } else {
                                    model_ops::insert_after(
                                        page,
                                        *objects.last().unwrap(),
                                        &content,
                                    )
                                    .1
                                };
                                let end = content.encode_utf16().count() as u32;
                                model_ops::restyle(page, text, 0..end, |format| {
                                    format.italic = None
                                });
                                model_ops::restyle(page, text, 0..6, italic);
                                inserted = Some(text);
                            });
                            match result {
                                Ok(Some(id)) => {
                                    ids.push(id);
                                    objects.push(inserted.unwrap());
                                    break;
                                }
                                Err(Error::Io(e)) if e.kind() == ErrorKind::ResourceBusy => {}
                                other => panic!("{other:?}"),
                            }
                        }
                        if sequence == 0 {
                            continue;
                        }
                        let text = *objects.last().unwrap();
                        loop {
                            assert!(
                                Instant::now() < deadline,
                                "Client {client} stopped at text {sequence}"
                            );
                            match model_ops::save(cache, text, |page| {
                                model_ops::replace_text(page, text, 0..0, "Edited ")
                            }) {
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
    assert_eq!(all.iter().map(|(ids, _)| ids.len()).sum::<usize>(), 84);
    assert_eq!(ids.len(), 1);
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
    for operation in ["Page", "CreatePage", "Pages", "DeletePages"] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("unknown.sqlite");
        let first = onestore::create_section("unknown.one", "Original", "Author").unwrap();
        let second = onestore::PageCreation::new(None, Some("Second"), "Author").unwrap();
        let source = onestore::PreparedEdit::create_page(&first, &second)
            .unwrap()
            .as_bytes()
            .to_vec();
        let (_, oid, _) = target(&source);
        let store = Store::parse(&source).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let sid = Document::parse(&index).unwrap().pages().unwrap()[1].0;
        let cache = Replica::create(&path, &source).unwrap();
        match operation {
            "Page" => {
                model_ops::save(&cache, oid, |page| {
                    model_ops::replace_text(page, oid, 0..0, "New ")
                })
                .unwrap()
                .unwrap();
            }
            "CreatePage" => {
                let page = onestore::PageCreation::new(None, Some("Created"), "Author").unwrap();
                cache.create_page(&source, &page).unwrap().unwrap();
            }
            "Pages" => {
                cache
                    .pages(&source, &[onestore::PageEdit::set_level(sid, 2).unwrap()])
                    .unwrap()
                    .unwrap();
            }
            _ => {
                cache.delete_pages(&source, &[sid]).unwrap().unwrap();
            }
        }
        drop(cache);
        let db = rusqlite::Connection::open(&path).unwrap();
        let encoded: String = db
            .query_row("SELECT operation FROM edits", [], |r| r.get(0))
            .unwrap();
        let mut value: serde_json::Value = serde_json::from_str(&encoded).unwrap();
        if value[operation].is_object() {
            value[operation]["future_option"] = true.into();
        } else {
            value["future_option"] = true.into();
        }
        db.execute("UPDATE edits SET operation=?1", [value.to_string()])
            .unwrap();
        drop(db);
        let before = fs::read(&path).unwrap();
        assert!(
            matches!(Replica::open(&path),Err(Error::Io(error))if error.kind()==ErrorKind::InvalidData),
            "{operation}"
        );
        assert_eq!(fs::read(&path).unwrap(), before);
    }
}
