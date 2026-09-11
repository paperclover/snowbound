//! The background worker: waking, backoff, ownership and error propagation.

use notebook::{ConflictKind, EditStatus, Error, Remote, Replica};
use onestore::{
    CommitError, CommitState, ExGuid, PreparedEdit, RevisionIndex, Store, document::Document,
};
use std::io;

#[path = "support/server.rs"]
mod server;
use server::*;
#[path = "support/model_ops.rs"]
mod model_ops;

use std::{
    ops::Range,
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

/// Saves a replacement of `range` in the page holding `text`.
fn save(cache: &Replica, text: ExGuid, range: Range<u32>, replacement: &str) -> Option<u64> {
    model_ops::save(cache, text, |page| {
        model_ops::replace_text(page, text, range, replacement)
    })
    .unwrap()
}

/// The text of the paragraph holding `object`, wherever the section's pages hold it.
fn content(bytes: &[u8], object: ExGuid) -> String {
    let (_, page) = model_ops::locate(bytes, object);
    model_ops::paragraph_with(&page, object)
        .unwrap()
        .text()
        .unwrap()
        .text
        .text()
        .to_owned()
}

fn pages(bytes: &[u8]) -> usize {
    let store = Store::parse(bytes).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    Document::parse(&index).unwrap().pages().unwrap().len()
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
    let (_, oid, _) = text(&source);
    let cache = Arc::new(Replica::create(&path, &source).unwrap());
    let id = save(&cache, oid, 1..2, "🦀").unwrap();
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
    let cached = cache.snapshot().unwrap();
    // Confirmation changes reader-notification fields without changing the committed graph.
    assert_eq!(cached[..212], server.durable[..212]);
    assert_eq!(cached[252..], server.durable[252..]);
    let cached_generation = Store::parse(&cached).unwrap().header.generation;
    let remote_generation = Store::parse(&server.durable).unwrap().header.generation;
    if cached_generation == remote_generation {
        assert_eq!(cached, server.durable);
    } else {
        assert_eq!(cached_generation.checked_add(1), Some(remote_generation));
    }
}

#[test]
fn local_saves_wake_an_idle_worker_and_publish_every_writer_marker() {
    let dir = tempfile::tempdir().unwrap();
    let source = onestore::create_section("worker.one", "abc", "Fixture").unwrap();
    let (_, oid, _) = text(&source);
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
    let queued = std::thread::scope(|scope| {
        (0..12)
            .map(|writer| {
                let cache = &cache;
                scope.spawn(move || {
                    let marker = format!("[{writer}] ");
                    loop {
                        match model_ops::save(cache, oid, |page| {
                            model_ops::replace_text(page, oid, 0..0, &marker)
                        }) {
                            Ok(Some(id)) => break id,
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
            .collect::<std::collections::BTreeSet<_>>()
    });
    resume_tx.send(()).unwrap();
    let mut published = std::collections::BTreeSet::new();
    while !cache.pending().unwrap().is_empty() {
        if let Some((id, status)) = observed_rx.recv_timeout(Duration::from_secs(5)).unwrap() {
            assert!(matches!(status, EditStatus::Published { .. }), "{status:?}");
            published.insert(id);
        }
    }
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "Edits waited for the hourly poll"
    );
    worker.stop().unwrap();
    assert_eq!(published, queued);
    let content = text(&cache.snapshot().unwrap()).2;
    for writer in 0..12 {
        assert_eq!(content.matches(&format!("[{writer}] ")).count(), 1);
    }
    assert!(content.ends_with("abc"));
    let server = server.lock().unwrap();
    assert_eq!(text(&server.durable).2, content);
    assert_eq!(server.publications, queued.len());
    assert_eq!(cache.snapshot().unwrap(), server.durable);
}

#[test]
fn dropping_during_publication_is_nonblocking_and_retains_ownership_until_recovery_is_recorded() {
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
    let (_, oid, _) = text(&source);
    let cache = Arc::new(Replica::create(&path, &source).unwrap());
    let id = save(&cache, oid, 0..0, "L ").unwrap();
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
    let (_, oid, _) = text(&source);
    let cache = Replica::create(&path, &source).unwrap();
    let id = save(&cache, oid, 0..0, "L ").unwrap();
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
        let cache = Arc::new(Replica::create(dir.path().join("cache.sqlite"), &source).unwrap());
        let id = save(&cache, oid, 1..2, "L").unwrap();
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
                assert_eq!(status, EditStatus::Conflict(ConflictKind::ContentChanged));
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
    let id = save(&cache, oid, 1..2, "L").unwrap();
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
        Some((id, EditStatus::Conflict(ConflictKind::ContentChanged)))
    );
    model_ops::review(&cache, id, oid, |page| {
        model_ops::replace_text(page, oid, 1..2, "L")
    })
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
    let (_, oid, _) = text(&source);
    let cache = Arc::new(Replica::create(dir.path().join("cache.sqlite"), &source).unwrap());
    let id = save(&cache, oid, 0..0, "L ").unwrap();
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
    let (_, oid, _) = text(&source);
    let cache = Arc::new(Replica::create(dir.path().join("cache.sqlite"), &source).unwrap());
    let first = save(&cache, oid, 0..0, "L ").unwrap();
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
                    let page =
                        onestore::PageCreation::new(None, Some("Queued"), "Fixture").unwrap();
                    let second = observed
                        .create_page(&observed.snapshot().unwrap(), &page)
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
    assert_eq!(content(&cache.snapshot().unwrap(), oid), "L abc");
    assert_eq!(pages(&cache.snapshot().unwrap()), 2);
}
