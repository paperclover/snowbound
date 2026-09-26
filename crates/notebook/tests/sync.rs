//! Replica protocol: durable receipts, retired attempts, conflict resolution and contention.

use notebook::{ConflictKind, EditStatus, Error, Remote, Replica, Resolution};
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
        fn stamp(&mut self) -> io::Result<Option<onestore::Stamp>> {
            self.server.stamp()
        }
        fn publish(&mut self, transaction: &Transaction) -> Result<(), CommitError> {
            self.server.publish(transaction)
        }
        fn confirm(&mut self, snapshot: &[u8]) -> Result<(), CommitError> {
            self.server.confirm(snapshot)
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
        onestore::replace_text(&remote.server.visible, sid, object, 0..0, "Native ").unwrap();
    remote.server.visible.clone_from(&native);
    assert_eq!(cache.sync_once(&mut remote).unwrap().edit, None);
    assert_eq!(remote.reads, 1);
    assert_eq!(cache.snapshot().unwrap(), native);
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
    let working = cache.snapshot().unwrap();
    let remote = cache.remote_snapshot().unwrap();
    let pending = cache.pending().unwrap();
    let uncertain = cache.status(attempted).unwrap();
    let source_file = std::fs::read(&path).unwrap();
    let summary = RecoverySummary {
        queued_edits: 4,
        conflicts: 0,
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
fn recovery_archive_retains_conflict_images_and_rejects_foreign_or_future_archives() {
    use notebook::Recovery;

    let directory = tempfile::tempdir().unwrap();
    let source = onestore::create_section("recovery.one", "Original", "Fixture").unwrap();
    let (space, object, _) = text(&source);
    let cache = Replica::create(directory.path().join("live.sqlite"), &source).unwrap();
    let id = save(&cache, object, 0..8, "Local").unwrap();
    let changed = onestore::replace_text(&source, space, object, 0..8, "Remote").unwrap();
    let mut server = Server::new(&changed);
    let outcome = cache.sync_once(&mut server).unwrap().edit.unwrap();
    assert_eq!(
        outcome,
        (id, EditStatus::Conflict(ConflictKind::ContentChanged))
    );
    let path = directory.path().join("conflict.sqlite");
    cache.export_recovery(&path).unwrap();
    let archive = Recovery::open(&path).unwrap();
    assert_eq!(text(&archive.snapshot().unwrap()).2, "Local");
    assert_eq!(archive.remote_snapshot().unwrap(), changed);
    assert_eq!(archive.status(id).unwrap(), Some(outcome.1.clone()));
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
    assert_eq!(cache.status(id).unwrap(), Some(outcome.1.clone()));
    assert_eq!(text(&cache.snapshot().unwrap()).2, "Local");
}

#[test]
fn disjoint_remote_changes_merge_and_persist_the_remote_receipt() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.sqlite");
    let source = onestore::create_section("sync.one", "ab🦀cd", "Fixture").unwrap();
    let (sid, oid, _) = text(&source);
    let cache = Replica::create(&path, &source).unwrap();
    let id = save(&cache, oid, 2..4, "🐈").unwrap();
    let remote = onestore::replace_text(&source, sid, oid, 0..6, "Xab🦀cYd").unwrap();
    let mut server = Server::new(&remote);
    let outcome = cache.sync_once(&mut server).unwrap().edit.unwrap();
    assert_eq!(outcome.0, id);
    assert!(matches!(outcome.1, EditStatus::Published { .. }));
    assert_eq!(text(&server.durable).2, "Xab🐈cYd");
    assert_eq!(cache.snapshot().unwrap(), server.durable);
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

#[test]
fn overlapping_changes_preserve_both_images_and_survive_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.sqlite");
    let source = onestore::create_section("sync.one", "abc", "Fixture").unwrap();
    let (sid, oid, _) = text(&source);
    let cache = Replica::create(&path, &source).unwrap();
    let id = save(&cache, oid, 1..2, "L").unwrap();
    let local = cache.snapshot().unwrap();
    let remote = onestore::replace_text(&source, sid, oid, 1..2, "R").unwrap();
    let mut server = Server::new(&remote);
    assert_eq!(
        cache.sync_once(&mut server).unwrap().edit,
        Some((id, EditStatus::Conflict(ConflictKind::ContentChanged)))
    );
    assert_eq!(server.publications, 0);
    drop(cache);
    let cache = Replica::open(&path).unwrap();
    assert_eq!(pages(&cache.snapshot().unwrap()), pages(&local));
    assert_eq!(cache.remote_snapshot().unwrap(), remote);
    assert_eq!(cache.pending().unwrap().len(), 1);
    assert_eq!(
        cache.status(id).unwrap(),
        Some(EditStatus::Conflict(ConflictKind::ContentChanged))
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
            self.wait("read");
            self.server.read()
        }
        fn publish(&mut self, transaction: &Transaction) -> Result<(), CommitError> {
            self.wait("publish");
            self.server.publish(transaction)
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
            assert!(
                matches!(cache.resolve(first, Resolution::Theirs), Err(Error::Io(error)) if error.kind() == io::ErrorKind::WouldBlock)
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
        let expected = text(&cache.snapshot().unwrap()).2;
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
        assert_eq!(cache.snapshot().unwrap(), server.durable);
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
        let before = pages(&cache.snapshot().unwrap());
        assert!(
            matches!(cache.sync_once(&mut server),Err(Error::Io(error)) if error.kind()==io::ErrorKind::InvalidInput)
        );
        assert_eq!(pages(&cache.snapshot().unwrap()), before);
        assert_eq!(cache.remote_snapshot().unwrap(), source);
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

#[test]
fn a_conflict_holds_later_edits_until_resolved_either_way_and_survives_reopen() {
    for resolution in [Resolution::Mine, Resolution::Theirs] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.sqlite");
        let source = onestore::create_section("resolve.one", "abc", "Fixture").unwrap();
        let (sid, oid, _) = text(&source);
        let cache = Replica::create(&path, &source).unwrap();
        let first = save(&cache, oid, 1..2, "L").unwrap();
        let remote = onestore::replace_text(&source, sid, oid, 0..3, "aRcZ").unwrap();
        let mut server = Server::new(&remote);
        let conflict = EditStatus::Conflict(ConflictKind::ContentChanged);
        assert_eq!(
            cache.sync_once(&mut server).unwrap().edit,
            Some((first, conflict.clone()))
        );
        let mut dependent = None;
        for n in 0..12 {
            dependent = save(&cache, oid, 0..0, &format!("[{n}] "));
        }
        let dependent = dependent.unwrap();
        assert_ne!(dependent, first);
        assert_eq!(cache.status(dependent).unwrap(), Some(conflict.clone()));
        let local = content(&cache.snapshot().unwrap(), oid);
        assert!(local.ends_with("[0] aLc"), "{local}");
        let pending = cache.pending().unwrap();
        assert_eq!(pending.len(), 13);
        assert_eq!(
            cache.sync_once(&mut server).unwrap().edit,
            Some((dependent, conflict.clone())),
            "an unchanged remote leaves the conflict as it was"
        );
        assert_eq!(server.publications, 0);
        drop(cache);
        let cache = Replica::open(&path).unwrap();
        assert_eq!(cache.pending().unwrap(), pending);
        assert_eq!(content(&cache.snapshot().unwrap(), oid), local);
        assert_eq!(
            cache.conflict().unwrap(),
            Some(notebook::Conflict {
                id: dependent,
                space: sid,
                kind: ConflictKind::ContentChanged
            })
        );
        cache.resolve(first, resolution).unwrap();
        assert_eq!(cache.conflict().unwrap(), None);
        assert_eq!(cache.remote_snapshot().unwrap(), remote);
        let expected = match resolution {
            Resolution::Mine => local.clone(),
            Resolution::Theirs => "aRcZ".to_owned(),
        };
        assert_eq!(content(&cache.snapshot().unwrap(), oid), expected);
        drop(cache);
        let cache = Replica::open(&path).unwrap();
        match resolution {
            Resolution::Mine => {
                assert!(matches!(
                    cache.sync_once(&mut server).unwrap().edit,
                    Some((_, EditStatus::Published { .. }))
                ));
                assert_eq!(server.publications, 1);
                for edit in &pending {
                    assert!(matches!(
                        cache.status(edit.id).unwrap(),
                        None | Some(EditStatus::Published { .. })
                    ));
                }
                assert!(matches!(
                    cache.status(first).unwrap(),
                    Some(EditStatus::Published { .. })
                ));
            }
            Resolution::Theirs => {
                // The emptied edits are acknowledged once the remote image is flushed.
                assert!(matches!(
                    cache.sync_once(&mut server).unwrap().edit,
                    Some((id, EditStatus::Published { .. })) if id == dependent
                ));
                assert_eq!((server.publications, server.confirmations), (0, 1));
                assert!(matches!(
                    cache.status(first).unwrap(),
                    Some(EditStatus::Published { .. })
                ));
            }
        }
        assert_eq!(content(&server.durable, oid), expected);
        assert_eq!(pages(&cache.snapshot().unwrap()), pages(&server.durable));
        assert!(cache.pending().unwrap().is_empty());
    }
}

#[test]
fn resolution_refuses_edits_that_are_not_in_conflict() {
    let dir = tempfile::tempdir().unwrap();
    let source = onestore::create_section("resolve.one", "abc", "Fixture").unwrap();
    let (sid, oid, _) = text(&source);
    let cache = Replica::create(dir.path().join("cache.sqlite"), &source).unwrap();
    let id = save(&cache, oid, 1..2, "L").unwrap();
    let refused = |cache: &Replica, id| matches!(cache.resolve(id, Resolution::Mine), Err(Error::Io(error)) if error.kind() == io::ErrorKind::InvalidInput);
    assert!(refused(&cache, id));
    let remote = onestore::replace_text(&source, sid, oid, 0..3, "🦀Rc").unwrap();
    let mut server = Server::new(&remote);
    assert!(matches!(
        cache.sync_once(&mut server).unwrap().edit,
        Some((_, EditStatus::Conflict(_)))
    ));
    let pending = cache.pending().unwrap();
    assert!(refused(&cache, id + 1));
    assert_eq!(cache.pending().unwrap(), pending);
    cache.resolve(id, Resolution::Mine).unwrap();
    server.fault = Fault::UnknownBefore;
    assert!(
        matches!(cache.sync_once(&mut server), Err(Error::Remote(error)) if error.state == CommitState::Unknown)
    );
    let attempted = cache.status(id).unwrap();
    assert!(matches!(
        attempted,
        Some(EditStatus::AwaitingConfirmation { .. })
    ));
    assert!(refused(&cache, id));
    assert_eq!(cache.status(id).unwrap(), attempted);
    assert_eq!(server.publications, 1);
}

#[test]
fn a_failed_resolution_leaves_the_conflict_as_it_was() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.sqlite");
    let source = onestore::create_section("resolve.one", "abc", "Fixture").unwrap();
    let (sid, oid, _) = text(&source);
    let cache = Replica::create(&path, &source).unwrap();
    let id = save(&cache, oid, 1..2, "L").unwrap();
    let remote = onestore::replace_text(&source, sid, oid, 0..3, "XaRc").unwrap();
    let mut server = Server::new(&remote);
    cache.sync_once(&mut server).unwrap();
    let local = pages(&cache.snapshot().unwrap());
    let pending = cache.pending().unwrap();
    let status = cache.status(id).unwrap();
    drop(cache);
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection.execute_batch("CREATE TRIGGER fail_clear BEFORE DELETE ON batches BEGIN SELECT RAISE(ABORT, 'test resolution failure'); END;").unwrap();
    drop(connection);
    let cache = Replica::open(&path).unwrap();
    assert!(matches!(
        cache.resolve(id, Resolution::Mine),
        Err(Error::Database(_))
    ));
    drop(cache);
    let cache = Replica::open(&path).unwrap();
    assert_eq!(cache.pending().unwrap(), pending);
    assert_eq!(cache.status(id).unwrap(), status);
    assert_eq!(pages(&cache.snapshot().unwrap()), local);
    assert_eq!(cache.remote_snapshot().unwrap(), remote);
    assert_eq!(server.publications, 0);
}

#[test]
fn seeded_conflicts_resolve_to_the_remote_text_and_later_edits_publish_once() {
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
        let id = save(&cache, oid, start..end, "λ🦊μ").unwrap();
        save(&cache, oid, start + 1..start + 3, "🐕").unwrap();
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
                cache.sync_once(&mut server).unwrap().edit,
                Some((_, EditStatus::Conflict(ConflictKind::ContentChanged)))
            ),
            "case {case}, seed {seed}"
        );
        let at_remote = start + prefix.len() as u32;
        let reviewed = model_ops::review(&cache, id, oid, |page| {
            model_ops::replace_text(page, oid, at_remote..at_remote + 4, "λ🐕μ")
        })
        .unwrap()
        .unwrap();
        assert!(
            matches!(cache.sync_once(&mut server).unwrap().edit, Some((actual, EditStatus::Published { .. })) if actual == reviewed),
            "case {case}, seed {seed}"
        );
        let mut expected = original.clone();
        expected.splice(at..at + 1, "λ🐕μ".chars());
        let expected = prefix + &expected.iter().collect::<String>() + &suffix;
        assert_eq!(
            text(&server.durable).2,
            expected,
            "case {case}, seed {seed}"
        );
        assert_eq!(server.publications, 1);
        assert!(cache.pending().unwrap().is_empty());
    }
}

#[test]
fn remote_changes_after_resolution_are_never_overwritten() {
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
        fn publish(&mut self, transaction: &Transaction) -> Result<(), CommitError> {
            self.server.publish(transaction)
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
        let id = save(&cache, oid, 1..2, "L").unwrap();
        let remote = onestore::replace_text(&source, sid, oid, 1..2, "R").unwrap();
        let mut remote = ChangedAfterRead {
            server: Server::new(&remote),
            change: None,
        };
        assert!(matches!(
            cache.sync_once(&mut remote).unwrap().edit,
            Some((_, EditStatus::Conflict(_)))
        ));
        let id = model_ops::review(&cache, id, oid, |page| {
            model_ops::replace_text(page, oid, 1..2, "L")
        })
        .unwrap()
        .unwrap();
        let local = content(&cache.snapshot().unwrap(), oid);
        assert_eq!(local, "aLc");
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
            cache.sync_once(&mut remote).unwrap().edit,
            Some((id, EditStatus::Conflict(ConflictKind::ContentChanged)))
        );
        assert_eq!(remote.server.durable, changed);
        assert_eq!(remote.server.visible, changed);
        assert_eq!(remote.server.publications, usize::from(after_read));
        assert_eq!(content(&cache.snapshot().unwrap(), oid), local);
        assert!(cache.pending().unwrap()[0].edit.ops.is_empty());
        assert!(matches!(
            &cache.pending().unwrap()[1].edit.ops[..],
            [onestore::op::Op::Page { op: onestore::op::PageOp::Text { range, with, .. }, .. }]
                if *range == (1..2) && with == "L"
        ));
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
    let published_image = cache.snapshot().unwrap();
    cache
        .export_recovery(directory.path().join("published.sqlite"))
        .unwrap();
    drop(cache);

    server.visible.clone_from(&source);
    server.durable.clone_from(&source);
    let cache = Replica::open(&path).unwrap();
    assert_eq!(cache.sync_once(&mut server).unwrap().edit, None);
    assert_eq!(cache.snapshot().unwrap(), source);
    assert_eq!(cache.remote_snapshot().unwrap(), source);
    assert_eq!(cache.status(published).unwrap(), Some(receipt.clone()));
    assert_eq!(server.publications, 1);
    assert!(cache.pending().unwrap().is_empty());
    let archive = notebook::Recovery::open(directory.path().join("published.sqlite")).unwrap();
    assert_eq!(archive.snapshot().unwrap(), published_image);
    assert_eq!(archive.status(published).unwrap(), Some(receipt.clone()));
    assert_eq!(text(&archive.snapshot().unwrap()).2, "Published Original");
    drop(cache);
    let cache = Replica::open(path).unwrap();
    assert_eq!(text(&cache.snapshot().unwrap()).2, "Original");
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
        let end = u32::try_from(text(&cache.snapshot().unwrap()).2.encode_utf16().count()).unwrap();
        let queued = save(&cache, object, end..end, " Local").unwrap();
        if uncertain {
            server.fault = fault;
            assert!(cache.sync_once(&mut server).is_err());
        }
        let prior_status = cache.status(queued).unwrap();
        let local = cache.snapshot().unwrap();
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
            assert_eq!(pages(&cache.snapshot().unwrap()), pages(&local));
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

#[test]
fn restore_conflicts_keep_dependent_work_until_mine_publishes_it() {
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
    let conflict = EditStatus::Conflict(ConflictKind::ContentChanged);
    assert_eq!(
        cache.sync_once(&mut server).unwrap().edit,
        Some((head, conflict.clone()))
    );
    let end = u32::try_from(text(&cache.snapshot().unwrap()).2.encode_utf16().count()).unwrap();
    let dependent = save(&cache, object, end..end, " Later").unwrap();
    assert_ne!(head, dependent);
    let local = text(&cache.snapshot().unwrap()).2;
    assert_eq!(local, "Revised Original Later");
    let pending = cache.pending().unwrap();
    assert_eq!(pending.len(), 2);
    cache
        .export_recovery(directory.path().join("conflicted.sqlite"))
        .unwrap();
    drop(cache);
    let cache = Replica::open(&path).unwrap();
    assert_eq!(cache.status(head).unwrap(), Some(conflict.clone()));
    assert_eq!(cache.status(dependent).unwrap(), Some(conflict.clone()));
    assert_eq!(cache.pending().unwrap(), pending);
    let changed = onestore::replace_text(&source, space, object, 8..8, " Remote").unwrap();
    server.visible.clone_from(&changed);
    server.durable.clone_from(&changed);
    assert_eq!(
        cache.sync_once(&mut server).unwrap().edit,
        Some((dependent, conflict.clone()))
    );
    assert_eq!(cache.remote_snapshot().unwrap(), changed);
    assert_eq!(server.publications, 1);
    cache.resolve(head, Resolution::Mine).unwrap();
    drop(cache);
    let cache = Replica::open(&path).unwrap();
    assert!(matches!(
        cache.sync_once(&mut server).unwrap().edit,
        Some((_, EditStatus::Published { .. }))
    ));
    assert_eq!(text(&server.durable).2, local);
    assert!(matches!(
        cache.status(head).unwrap(),
        Some(EditStatus::Published { .. })
    ));
    assert_eq!(cache.status(published).unwrap(), Some(receipt.clone()));
    let archive = notebook::Recovery::open(directory.path().join("conflicted.sqlite")).unwrap();
    assert_eq!(text(&archive.snapshot().unwrap()).2, local);
    assert_eq!(archive.pending().unwrap(), pending);
    assert_eq!(archive.status(head).unwrap(), Some(conflict.clone()));
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
