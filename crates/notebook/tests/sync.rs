//! Replica protocol: durable receipts, retired attempts, conflict pages and contention.

use notebook::{EditStatus, Error, Remote, Replica};
use onestore::{CommitError, CommitState, ExGuid, Transaction};
use std::{io, ops::Range};

#[path = "support/server.rs"]
mod server;
use server::*;
#[path = "support/model_ops.rs"]
mod model_ops;

/// Saves a replacement of `range` in the page holding `text`.
fn save(cache: &Replica, text: ExGuid, range: Range<u32>, replacement: &str) -> Option<u64> {
    model_ops::save(cache, text, |page| {
        model_ops::replace_text(page, text, range, replacement)
    })
    .unwrap()
}

#[test]
fn an_unchanged_stamp_publishes_and_settles_without_reading_the_remote() {
    struct Counted {
        server: Server,
        reads: usize,
    }
    impl Remote for Counted {
        fn read(&mut self) -> io::Result<Vec<u8>> {
            self.reads += 1;
            self.server.read()
        }
        fn stamp(&mut self) -> io::Result<onestore::Stamp> {
            self.server.stamp()
        }
        fn publish(&mut self, transaction: &Transaction) -> Result<(), CommitError> {
            self.server.publish(transaction)
        }
        fn confirm(&mut self, base: &onestore::Stamp) -> Result<(), CommitError> {
            self.server.confirm(base)
        }
    }
    let directory = tempfile::tempdir().unwrap();
    let source = onestore::create_section("stamp.one", "Original", "Fixture").unwrap();
    let (sid, object, _) = text(&source);
    let cache = Replica::create(directory.path().join("cache.sqlite"), &source).unwrap();
    let id = save(&cache, object, 0..0, "Local ").unwrap();
    let mut remote = Counted {
        server: Server::new(&source),
        reads: 0,
    };
    assert!(matches!(
        cache.sync_once(&mut remote).unwrap().edit,
        Some((published, EditStatus::Published { .. })) if published == id
    ));
    assert_eq!(cache.sync_once(&mut remote).unwrap().edit, None);
    assert_eq!(remote.reads, 0);
    // Another writer's commit moves the header, so the next step reads the file.
    let native =
        typed(&remote.server.visible, sid, object, 0..0, "Native ");
    remote.server.visible.clone_from(&native);
    assert_eq!(cache.sync_once(&mut remote).unwrap().edit, None);
    assert_eq!(remote.reads, 1);
    assert_eq!(snapshot(&cache), native);
}

#[test]
fn recovery_archive_preserves_the_queue_uncertainty_and_receipts_without_becoming_a_writer() {
    use notebook::{Recovery, RecoverySummary};

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("live.sqlite");
    let archive_path = directory.path().join("recovery.sqlite");
    let source = onestore::create_section("recovery.one", "Original", "Fixture").unwrap();
    let (_, object, _) = text(&source);
    let cache = Replica::create(&path, &source).unwrap();
    let published = save(&cache, object, 0..0, "Published ").unwrap();
    let mut server = Server::new(&source);
    let (_, receipt) = cache.sync_once(&mut server).unwrap().edit.unwrap();
    let attempted = save(&cache, object, 0..0, "Uncertain ").unwrap();
    server.fault = Fault::UnknownBefore;
    assert!(matches!(
        cache.sync_once(&mut server),
        Err(Error::Remote(CommitError {
            state: CommitState::Unknown,
            ..
        }))
    ));
    save(&cache, object, 0..0, "Queued ").unwrap();
    let created = onestore::PageCreation::new(None, Some("Recovery 🦀"), "Fixture").unwrap();
    section_op(&cache, onestore::op::SectionOp::Create(created));
    save(&cache, object, 0..0, "Last ").unwrap();
    let working = snapshot(&cache);
    let remote = remote_snapshot(&cache);
    let pending = cache.pending().unwrap();
    let uncertain = cache.status(attempted).unwrap();
    let source_file = std::fs::read(&path).unwrap();
    let summary = RecoverySummary {
        queued_edits: 4,
        uncertain_edits: 1,
        published_receipts: 1,
        base_bytes: remote.len() as u64,
        remote_bytes: 0,
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
    assert_eq!(pages(&archive.snapshot().unwrap()), pages(&working));
    assert_eq!(archive.remote_snapshot().unwrap(), remote);
    assert_eq!(archive.pending().unwrap(), pending);
    assert_eq!(archive.status(published).unwrap(), Some(receipt.clone()));
    assert_eq!(archive.status(attempted).unwrap(), uncertain);
    for edit in &pending {
        assert_eq!(
            archive.status(edit.id).unwrap(),
            cache.status(edit.id).unwrap()
        );
    }
    let EditStatus::Published { revision } = receipt.clone() else {
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
    save(&cache, object, 0..0, "Later ").unwrap();
    assert_eq!(pages(&archive.snapshot().unwrap()), pages(&working));
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
fn recovery_archive_retains_the_queue_and_rejects_foreign_or_future_archives() {
    use notebook::Recovery;

    let directory = tempfile::tempdir().unwrap();
    let source = onestore::create_section("recovery.one", "Original", "Fixture").unwrap();
    let (_, object, _) = text(&source);
    let cache = Replica::create(directory.path().join("live.sqlite"), &source).unwrap();
    let id = save(&cache, object, 0..8, "Local").unwrap();
    let path = directory.path().join("queued.sqlite");
    cache.export_recovery(&path).unwrap();
    let archive = Recovery::open(&path).unwrap();
    assert_eq!(text(&archive.snapshot().unwrap()).2, "Local");
    assert_eq!(archive.remote_snapshot().unwrap(), source);
    assert_eq!(archive.status(id).unwrap(), Some(EditStatus::Pending));
    assert_eq!(archive.summary().unwrap().queued_edits, 1);
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
    assert_eq!(cache.status(id).unwrap(), Some(EditStatus::Pending));
    assert_eq!(text(&snapshot(&cache)).2, "Local");
}

#[test]
fn disjoint_remote_changes_merge_and_persist_the_remote_receipt() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.sqlite");
    let source = onestore::create_section("sync.one", "ab🦀cd", "Fixture").unwrap();
    let (sid, oid, _) = text(&source);
    let cache = Replica::create(&path, &source).unwrap();
    let id = save(&cache, oid, 2..4, "🐈").unwrap();
    let remote = typed(&source, sid, oid, 0..6, "Xab🦀cYd");
    let mut server = Server::new(&remote);
    let outcome = cache.sync_once(&mut server).unwrap().edit.unwrap();
    assert_eq!(outcome.0, id);
    assert!(matches!(outcome.1, EditStatus::Published { .. }));
    assert_eq!(text(&server.durable).2, "Xab🐈cYd");
    assert_eq!(snapshot(&cache), server.durable);
    assert!(cache.pending().unwrap().is_empty());
    drop(cache);
    let cache = Replica::open(&path).unwrap();
    assert_eq!(cache.status(id).unwrap(), Some(outcome.1.clone()));
    assert_eq!(cache.sync_once(&mut server).unwrap().edit, None);
    assert_eq!(server.publications, 1);
    assert_eq!(cache.status(id + 1).unwrap(), None);
    let next = save(&cache, oid, 0..0, "Later ").unwrap();
    assert!(next > id);
}

/// As OneNote 2010 does, the remote's version stays the page and the local one becomes a
/// conflict page under it, published with the rest of the queue.
#[test]
fn overlapping_changes_keep_both_versions_and_survive_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.sqlite");
    let source = onestore::create_section("sync.one", "abc", "Fixture").unwrap();
    let (sid, oid, _) = text(&source);
    let cache = Replica::create(&path, &source).unwrap();
    let id = save(&cache, oid, 1..2, "L").unwrap();
    let remote = typed(&source, sid, oid, 1..2, "R");
    let mut server = Server::new(&remote);
    assert!(matches!(
        cache.sync_once(&mut server).unwrap().edit,
        Some((newest, EditStatus::Published { .. })) if newest > id
    ));
    assert_eq!(server.publications, 1);
    assert_eq!(content(&server.durable, oid), "aRc");
    assert_eq!(
        conflicts(&server.durable),
        [(sid, vec![(model_ops::AUTHOR.to_owned(), vec!["aLc".to_owned()])])]
    );
    assert_eq!(snapshot(&cache), server.durable);
    assert!(cache.pending().unwrap().is_empty());
    drop(cache);
    let cache = Replica::open(&path).unwrap();
    assert!(matches!(
        cache.status(id).unwrap(),
        Some(EditStatus::Published { .. })
    ));
    assert_eq!(cache.conflicts().unwrap()[0].0, sid);
    assert_eq!(cache.sync_once(&mut server).unwrap().edit, None);
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
        let (_, oid, _) = text(&source);
        let cache = Replica::create(&path, &source).unwrap();
        let id = save(&cache, oid, 0..0, "Once ").unwrap();
        let mut server = Server::new(&source);
        server.fault = fault;
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            cache.sync_once(&mut server)
        }));
        let attempted = cache.status(id).unwrap().unwrap();
        assert!(matches!(attempted, EditStatus::AwaitingConfirmation { .. }));
        drop(cache);
        let cache = Replica::open(&path).unwrap();
        let result = cache.sync_once(&mut server).unwrap().edit.unwrap();
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
    let (_, oid, _) = text(&source);
    let cache = Replica::create(&path, &source).unwrap();
    let id = save(&cache, oid, 0..0, "Once ").unwrap();
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
        cache.sync_once(&mut server).unwrap().edit,
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
    let (_, oid, _) = text(&source);
    let cache = Replica::create(dir.path().join("cache.sqlite"), &source).unwrap();
    let id = save(&cache, oid, 0..0, "Once ").unwrap();
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
        let (_, oid, _) = text(&source);
        let cache = Replica::create(&path, &source).unwrap();
        let id = save(&cache, oid, 0..0, "Once ").unwrap();
        let mut server = Server::new(&source);
        server.fault = fault;
        assert!(matches!(
            cache.sync_once(&mut server),
            Err(Error::Remote(_))
        ));
        if matches!(fault, Fault::Before) {
            assert_eq!(cache.status(id).unwrap(), Some(EditStatus::Pending));
            assert!(matches!(
                cache.sync_once(&mut server).unwrap().edit,
                Some((_, EditStatus::Published { .. }))
            ));
            assert_eq!(server.publications, 2);
        } else {
            assert!(matches!(
                cache.status(id).unwrap(),
                Some(EditStatus::Published { .. })
            ));
            assert_eq!(cache.sync_once(&mut server).unwrap().edit, None);
            assert_eq!(server.publications, 1);
        }
        assert_eq!(text(&server.durable).2, "Once abc");
    }
}

#[test]
fn database_failures_before_and_after_publication_preserve_recovery_state() {
    for (table, event) in [
        ("attempt", "UPDATE OF attempted ON batches"),
        ("receipts", "INSERT ON receipts"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.sqlite");
        let source = onestore::create_section("sync.one", "abc", "Fixture").unwrap();
        let (_, oid, _) = text(&source);
        let cache = Replica::create(&path, &source).unwrap();
        let id = save(&cache, oid, 0..0, "Once ").unwrap();
        drop(cache);
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch(&format!("CREATE TRIGGER interrupted BEFORE {event} BEGIN SELECT RAISE(ABORT,'Injected cache failure'); END;")).unwrap();
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
            cache.sync_once(&mut server).unwrap().edit,
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
            self.server.read()
        }
        fn stamp(&mut self) -> io::Result<onestore::Stamp> {
            self.wait("read");
            self.server.stamp()
        }
        fn publish(&mut self, transaction: &Transaction) -> Result<(), CommitError> {
            self.wait("publish");
            self.server.publish(transaction)
        }
        fn confirm(&mut self, base: &onestore::Stamp) -> Result<(), CommitError> {
            self.wait("confirm");
            self.server.confirm(base)
        }
    }
    for phase in ["read", "publish", "confirm"] {
        let dir = tempfile::tempdir().unwrap();
        let source = onestore::create_section("sync.one", "abc", "Fixture").unwrap();
        let (sid, oid, _) = text(&source);
        let cache = Replica::create(dir.path().join("cache.sqlite"), &source).unwrap();
        let first = save(&cache, oid, 0..0, "First ").unwrap();
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
                assert!(matches!(
                    result,
                    Ok(notebook::Synced {
                        edit: Some((_, EditStatus::Published { .. })),
                        ..
                    })
                ));
                assert!(matches!(
                    cache.status(first).unwrap(),
                    Some(EditStatus::Published { .. })
                ));
                paused.server
            });
            entered_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            assert!(
                matches!(cache.sync_once(&mut Server::new(&source)),Err(Error::Io(error)) if error.kind()==io::ErrorKind::WouldBlock)
            );
            let started = std::time::Instant::now();
            let handles: Vec<_> = (0..12)
                .map(|writer| {
                    let cache = &cache;
                    scope.spawn(move || {
                        // Ops, unlike models, commute with the other writers' edits.
                        cache
                            .apply(
                                model_ops::AUTHOR,
                                onestore::op::Edit {
                                    at: model_ops::now(),
                                    ops: vec![onestore::op::Op::Page {
                                        space: sid,
                                        op: onestore::op::PageOp::Text {
                                            text: oid,
                                            range: 0..0,
                                            with: format!("[{writer}] "),
                                        },
                                    }],
                                },
                            )
                            .unwrap()
                    })
                })
                .collect();
            let ids: std::collections::BTreeSet<_> = handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .collect();
            assert_eq!(ids.len(), 12, "every local edit is its own");
            assert!(
                started.elapsed() < std::time::Duration::from_secs(2),
                "Local edits waited for remote {phase}"
            );
            resume_tx.send(()).unwrap();
            running.join().unwrap()
        });
        // Edits arriving while a batch publishes form the next batch.
        assert_eq!(
            cache.pending().unwrap().len(),
            if phase == "read" { 0 } else { 12 }
        );
        let expected = text(&snapshot(&cache)).2;
        for writer in 0..12 {
            assert_eq!(expected.matches(&format!("[{writer}] ")).count(), 1);
        }
        while !cache.pending().unwrap().is_empty() {
            assert!(matches!(
                cache.sync_once(&mut server).unwrap().edit,
                Some((_, EditStatus::Published { .. }))
            ));
        }
        assert_eq!(server.publications, if phase == "read" { 1 } else { 2 });
        assert_eq!(text(&server.durable).2, expected);
        assert_eq!(snapshot(&cache), server.durable);
    }
}

#[test]
fn unrelated_remote_files_never_replace_a_local_cache() {
    let dir = tempfile::tempdir().unwrap();
    let source = onestore::create_section("sync.one", "abc", "Fixture").unwrap();
    let (_, oid, _) = text(&source);
    let cache = Replica::create(dir.path().join("cache.sqlite"), &source).unwrap();
    let other = onestore::create_section("other.one", "abc", "Fixture").unwrap();
    let mut server = Server::new(&other);
    for pending in [false, true] {
        if pending {
            save(&cache, oid, 0..0, "Local ").unwrap();
        }
        let before = pages(&snapshot(&cache));
        assert!(
            matches!(cache.sync_once(&mut server),Err(Error::Io(error)) if error.kind()==io::ErrorKind::InvalidInput)
        );
        assert_eq!(pages(&snapshot(&cache)), before);
        assert_eq!(remote_snapshot(&cache), source);
        assert_eq!(server.publications, 0);
    }
}

/// The text object's content in an image.
fn content(bytes: &[u8], text: ExGuid) -> String {
    let (_, page) = model_ops::locate(bytes, text);
    model_ops::paragraph_with(&page, text)
        .unwrap()
        .text()
        .unwrap()
        .text
        .text()
        .to_owned()
}

/// A conflict never holds the queue: later edits of the page apply to the remote's version.
#[test]
fn edits_after_a_conflict_apply_to_the_remote_version_and_survive_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.sqlite");
    let source = onestore::create_section("resolve.one", "abc", "Fixture").unwrap();
    let (sid, oid, _) = text(&source);
    let cache = Replica::create(&path, &source).unwrap();
    save(&cache, oid, 1..2, "L").unwrap();
    let remote = typed(&source, sid, oid, 0..3, "aRcZ");
    let mut server = Server::new(&remote);
    assert!(matches!(
        cache.sync_once(&mut server).unwrap().edit,
        Some((_, EditStatus::Published { .. }))
    ));
    assert_eq!(content(&snapshot(&cache), oid), "aRcZ");
    let mut expected = "aRcZ".to_owned();
    for n in 0..12 {
        save(&cache, oid, 0..0, &format!("[{n}] ")).unwrap();
        expected.insert_str(0, &format!("[{n}] "));
    }
    let pending = cache.pending().unwrap();
    drop(cache);
    let cache = Replica::open(&path).unwrap();
    assert_eq!(cache.pending().unwrap(), pending);
    assert!(matches!(
        cache.sync_once(&mut server).unwrap().edit,
        Some((id, EditStatus::Published { .. })) if id == pending.last().unwrap().id
    ));
    assert_eq!(server.publications, 2);
    assert_eq!(content(&server.durable, oid), expected);
    assert_eq!(
        conflicts(&server.durable),
        [(sid, vec![(model_ops::AUTHOR.to_owned(), vec!["aLc".to_owned()])])]
    );
    assert!(cache.pending().unwrap().is_empty());
}

/// A rebase that cannot be stored leaves the queue, and every page, as it was.
#[test]
fn a_failed_rebase_leaves_the_queue_as_it_was() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.sqlite");
    let source = onestore::create_section("resolve.one", "abc", "Fixture").unwrap();
    let (sid, oid, _) = text(&source);
    let cache = Replica::create(&path, &source).unwrap();
    let id = save(&cache, oid, 1..2, "L").unwrap();
    let remote = typed(&source, sid, oid, 0..3, "XaRc");
    let mut server = Server::new(&remote);
    let local = pages(&snapshot(&cache));
    let pending = cache.pending().unwrap();
    drop(cache);
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection.execute_batch("CREATE TRIGGER fail_clear BEFORE DELETE ON batches BEGIN SELECT RAISE(ABORT, 'test rebase failure'); END;").unwrap();
    drop(connection);
    let cache = Replica::open(&path).unwrap();
    assert!(matches!(
        cache.sync_once(&mut server),
        Err(Error::Database(_))
    ));
    drop(cache);
    let cache = Replica::open(&path).unwrap();
    assert_eq!(cache.pending().unwrap(), pending);
    assert_eq!(cache.status(id).unwrap(), Some(EditStatus::Pending));
    assert_eq!(pages(&snapshot(&cache)), local);
    assert_eq!(server.publications, 0);
}

#[test]
fn seeded_conflicts_keep_the_remote_text_and_later_edits_publish_once() {
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
        save(&cache, oid, start..end, "λ🦊μ").unwrap();
        save(&cache, oid, start + 1..start + 3, "🐕").unwrap();
        let local = content(&snapshot(&cache), oid);
        let mut remote_text = original.clone();
        remote_text.splice(at..at + 1, "Ω🐈π".chars());
        let remote_text = prefix.clone() + &remote_text.iter().collect::<String>() + &suffix;
        let remote = typed(
            &source,
            sid,
            oid,
            0..original.iter().map(|ch| ch.len_utf16() as u32).sum(),
            &remote_text,
        );
        let mut server = Server::new(&remote);
        assert!(
            matches!(
                cache.sync_once(&mut server).unwrap().edit,
                Some((_, EditStatus::Published { .. }))
            ),
            "case {case}, seed {seed}"
        );
        assert_eq!(content(&server.durable, oid), remote_text, "case {case}, seed {seed}");
        assert_eq!(
            conflicts(&server.durable),
            [(sid, vec![(model_ops::AUTHOR.to_owned(), vec![local])])],
            "case {case}, seed {seed}"
        );
        let at_remote = start + prefix.len() as u32;
        let later = save(&cache, oid, at_remote..at_remote + 4, "λ🐕μ").unwrap();
        assert!(
            matches!(cache.sync_once(&mut server).unwrap().edit, Some((actual, EditStatus::Published { .. })) if actual == later),
            "case {case}, seed {seed}"
        );
        let mut expected = original.clone();
        expected.splice(at..at + 1, "λ🐕μ".chars());
        let expected = prefix + &expected.iter().collect::<String>() + &suffix;
        assert_eq!(content(&server.durable, oid), expected, "case {case}, seed {seed}");
        assert_eq!(server.publications, 2);
        assert!(cache.pending().unwrap().is_empty());
    }
}

#[test]
fn remote_changes_after_a_conflict_are_never_overwritten() {
    struct ChangedAfterStamp {
        server: Server,
        change: Option<Vec<u8>>,
    }
    impl Remote for ChangedAfterStamp {
        fn read(&mut self) -> io::Result<Vec<u8>> {
            self.server.read()
        }
        fn stamp(&mut self) -> io::Result<onestore::Stamp> {
            let stamp = self.server.stamp()?;
            if let Some(changed) = self.change.take() {
                self.server.visible = changed.clone();
                self.server.durable = changed;
            }
            Ok(stamp)
        }
        fn publish(&mut self, transaction: &Transaction) -> Result<(), CommitError> {
            self.server.publish(transaction)
        }
        fn confirm(&mut self, base: &onestore::Stamp) -> Result<(), CommitError> {
            self.server.confirm(base)
        }
    }
    for after_read in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let source = onestore::create_section("resolve.one", "abc", "Fixture").unwrap();
        let (sid, oid, _) = text(&source);
        let cache = Replica::create(dir.path().join("cache.sqlite"), &source).unwrap();
        save(&cache, oid, 1..2, "L").unwrap();
        let remote = typed(&source, sid, oid, 1..2, "R");
        let mut remote = ChangedAfterStamp {
            server: Server::new(&remote),
            change: None,
        };
        assert!(matches!(
            cache.sync_once(&mut remote).unwrap().edit,
            Some((_, EditStatus::Published { .. }))
        ));
        // The local version again, typed over the remote's.
        let id = save(&cache, oid, 1..2, "L").unwrap();
        assert_eq!(content(&snapshot(&cache), oid), "aLc");
        let changed = typed(&remote.server.visible, sid, oid, 1..2, "Q");
        if after_read {
            remote.change = Some(changed.clone());
            assert!(
                matches!(cache.sync_once(&mut remote), Err(Error::Remote(error)) if error.state == CommitState::NotCommitted)
            );
            assert_eq!(cache.status(id).unwrap(), Some(EditStatus::Pending));
        } else {
            remote.server.visible = changed.clone();
            remote.server.durable = changed.clone();
        }
        assert!(matches!(
            cache.sync_once(&mut remote).unwrap().edit,
            Some((_, EditStatus::Published { .. }))
        ));
        assert_eq!(content(&remote.server.durable, oid), "aQc");
        let versions = [(model_ops::AUTHOR.to_owned(), vec!["aLc".to_owned()])];
        assert_eq!(
            conflicts(&remote.server.durable),
            [(sid, [versions.clone(), versions].concat())]
        );
        assert!(cache.pending().unwrap().is_empty());
    }
}

#[test]
fn remote_restore_retains_historical_receipts_without_replaying_them() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("cache.sqlite");
    let source = onestore::create_section("restore.one", "Original", "Fixture").unwrap();
    let (_, object, _) = text(&source);
    let cache = Replica::create(&path, &source).unwrap();
    let mut server = Server::new(&source);
    let published = save(&cache, object, 0..0, "Published ").unwrap();
    let (_, receipt) = cache.sync_once(&mut server).unwrap().edit.unwrap();
    assert!(matches!(receipt, EditStatus::Published { .. }));
    let published_image = snapshot(&cache);
    cache
        .export_recovery(directory.path().join("published.sqlite"))
        .unwrap();
    drop(cache);

    server.visible.clone_from(&source);
    server.durable.clone_from(&source);
    let cache = Replica::open(&path).unwrap();
    assert_eq!(cache.sync_once(&mut server).unwrap().edit, None);
    assert_eq!(snapshot(&cache), source);
    assert_eq!(remote_snapshot(&cache), source);
    assert_eq!(cache.status(published).unwrap(), Some(receipt.clone()));
    assert_eq!(server.publications, 1);
    assert!(cache.pending().unwrap().is_empty());
    let archive = notebook::Recovery::open(directory.path().join("published.sqlite")).unwrap();
    assert_eq!(archive.snapshot().unwrap(), published_image);
    assert_eq!(archive.status(published).unwrap(), Some(receipt.clone()));
    assert_eq!(text(&archive.snapshot().unwrap()).2, "Published Original");
    drop(cache);
    let cache = Replica::open(path).unwrap();
    assert_eq!(text(&snapshot(&cache)).2, "Original");
    assert_eq!(cache.status(published).unwrap(), Some(receipt.clone()));
}

#[test]
fn remote_restore_rebases_unsent_work_but_never_replays_an_uncertain_attempt() {
    for fault in [Fault::None, Fault::UnknownBefore, Fault::UnknownAfter] {
        let uncertain = !matches!(fault, Fault::None);
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("cache.sqlite");
        let source = onestore::create_section("restore.one", "Original", "Fixture").unwrap();
        let (_, object, _) = text(&source);
        let cache = Replica::create(&path, &source).unwrap();
        let mut server = Server::new(&source);
        let published = save(&cache, object, 0..0, "Published ").unwrap();
        let (_, receipt) = cache.sync_once(&mut server).unwrap().edit.unwrap();
        let end = u32::try_from(text(&snapshot(&cache)).2.encode_utf16().count()).unwrap();
        let queued = save(&cache, object, end..end, " Local").unwrap();
        if uncertain {
            server.fault = fault;
            assert!(cache.sync_once(&mut server).is_err());
        }
        let prior_status = cache.status(queued).unwrap();
        let local = snapshot(&cache);
        let pending = cache.pending().unwrap();
        cache
            .export_recovery(directory.path().join("before-restore.sqlite"))
            .unwrap();
        drop(cache);
        server.visible.clone_from(&source);
        server.durable.clone_from(&source);
        let attempts = server.publications;
        let cache = Replica::open(&path).unwrap();
        let (_, status) = cache.sync_once(&mut server).unwrap().edit.unwrap();
        assert_eq!(cache.status(published).unwrap(), Some(receipt.clone()));
        if uncertain {
            assert!(matches!(status, EditStatus::AwaitingConfirmation { .. }));
            assert_eq!(Some(status.clone()), prior_status);
            assert_eq!(server.publications, attempts);
            assert_eq!(server.visible, source);
            assert_eq!(pages(&snapshot(&cache)), pages(&local));
            assert_eq!(cache.pending().unwrap(), pending);
        } else {
            assert!(matches!(status, EditStatus::Published { .. }));
            assert_eq!(server.publications, attempts + 1);
            assert_eq!(text(&server.visible).2, "Original Local");
            assert!(cache.pending().unwrap().is_empty());
        }
        let archive =
            notebook::Recovery::open(directory.path().join("before-restore.sqlite")).unwrap();
        assert_eq!(pages(&archive.snapshot().unwrap()), pages(&local));
        assert_eq!(archive.pending().unwrap(), pending);
        assert_eq!(archive.status(queued).unwrap(), prior_status);
        drop(cache);
        let cache = Replica::open(&path).unwrap();
        assert_eq!(cache.status(queued).unwrap(), Some(status.clone()));
        assert_eq!(cache.status(published).unwrap(), Some(receipt.clone()));
    }
}

/// A remote restored to an older image conflicts with local work on what it undid: the
/// restored page stays and the local version becomes its conflict page.
#[test]
fn a_remote_restore_keeps_local_work_on_a_conflict_page() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("cache.sqlite");
    let source = onestore::create_section("restore.one", "Original", "Fixture").unwrap();
    let (space, object, _) = text(&source);
    let cache = Replica::create(&path, &source).unwrap();
    let mut server = Server::new(&source);
    let published = save(&cache, object, 0..0, "Published ").unwrap();
    let (_, receipt) = cache.sync_once(&mut server).unwrap().edit.unwrap();
    let head = save(&cache, object, 0..9, "Revised").unwrap();
    server.visible.clone_from(&source);
    server.durable.clone_from(&source);
    assert!(matches!(
        cache.sync_once(&mut server).unwrap().edit,
        Some((_, EditStatus::Published { .. }))
    ));
    assert_eq!(content(&server.durable, object), "Original");
    assert_eq!(
        conflicts(&server.durable),
        [(
            space,
            vec![(model_ops::AUTHOR.to_owned(), vec!["Revised Original".to_owned()])]
        )]
    );
    drop(cache);
    let cache = Replica::open(&path).unwrap();
    assert!(matches!(
        cache.status(head).unwrap(),
        Some(EditStatus::Published { .. })
    ));
    assert_eq!(cache.status(published).unwrap(), Some(receipt));
    assert!(cache.pending().unwrap().is_empty());
}

#[test]
fn cache_open_validates_the_tail_before_head_only_synchronization() {
    for damage in ["edit='{}'", "edit=replace(edit, 'Create', 'Delete')"] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("cache.sqlite");
        let source = onestore::create_section("tail.one", "Original", "Fixture").unwrap();
        let (_, object, _) = text(&source);
        let cache = Replica::create(&path, &source).unwrap();
        save(&cache, object, 0..0, "Head ").unwrap();
        let page = onestore::PageCreation::new(None, Some("Tail"), "Fixture").unwrap();
        let tail = section_op(&cache, onestore::op::SectionOp::Create(page));
        assert_eq!(cache.pending().unwrap().len(), 2);
        drop(cache);
        let database = rusqlite::Connection::open(&path).unwrap();
        database
            .execute(
                &format!("UPDATE edits SET {damage} WHERE id=?1"),
                [i64::try_from(tail).unwrap()],
            )
            .unwrap();
        drop(database);
        let damaged = std::fs::read(&path).unwrap();
        assert!(
            matches!(Replica::open(&path), Err(Error::Io(error)) if error.kind() == io::ErrorKind::InvalidData)
        );
        assert_eq!(std::fs::read(&path).unwrap(), damaged);
    }
}
