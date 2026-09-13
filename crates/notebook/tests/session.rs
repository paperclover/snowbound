use notebook::{
    ConflictKind, EditStatus, Recovery,
    session::{Event, Notebook, QueuedEdit, Save, Section},
};
use onestore::{ExGuid, PreparedEdit, page::Page};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

#[path = "support/model_ops.rs"]
mod model_ops;

fn first_text(page: &Page) -> ExGuid {
    page.objects
        .iter()
        .find_map(|object| match object {
            onestore::page::PageObject::Outline(outline) => outline
                .paragraphs
                .iter()
                .find_map(|p| p.text().map(|t| t.id)),
            _ => None,
        })
        .unwrap()
}

fn edited(page: &Page, text: &str) -> Page {
    let mut after = page.clone();
    let id = first_text(page);
    model_ops::replace_text(&mut after, id, 0..0, text);
    after
}

fn open(file: &Path, cache: &Path) -> (Section, Arc<AtomicUsize>) {
    let notified = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&notified);
    let section = Section::open(file, cache, move || {
        counter.fetch_add(1, Ordering::SeqCst);
    })
    .unwrap();
    (section, notified)
}

fn wait(section: &Section, mut accept: impl FnMut(&Event) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        for event in section.events() {
            if accept(&event) {
                return;
            }
        }
        assert!(
            Instant::now() < deadline,
            "the expected event did not arrive"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn published(section: &Section, id: u64) {
    wait(
        section,
        |event| matches!(event, Event::Attempt { id: n, status: EditStatus::Published { .. } } if *n == id),
    );
}

fn stored_page(file: &Path, space: ExGuid) -> Page {
    model_ops::page_of(&onestore::read_file(file).unwrap(), space)
}

/// The page title is derived from its content on read; an edited model keeps the old one.
fn assert_same(actual: Page, expected: &Page) {
    let mut expected = expected.clone();
    expected.title = actual.title.clone();
    assert_eq!(actual, expected);
}

#[test]
fn a_section_opens_through_its_replica_and_a_save_reaches_the_file_and_survives_relaunch() {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("notes.one");
    let cache = directory.path().join("cache");
    std::fs::write(
        &file,
        onestore::create_section("notes.one", "Original", "Author").unwrap(),
    )
    .unwrap();
    let (section, notified) = open(&file, &cache);
    let pages = section.pages().unwrap();
    assert_eq!(pages.len(), 1);
    let space = pages[0].0;
    let before = section.page(space).unwrap();
    assert_eq!(
        section.save(space, &before, &before, "Editor").unwrap(),
        Save::Unchanged
    );
    let after = edited(&before, "Saved ");
    let Save::Queued(id) = section.save(space, &before, &after, "Editor").unwrap() else {
        panic!()
    };
    assert_same(section.page(space).unwrap(), &after);
    published(&section, id);
    assert!(notified.load(Ordering::SeqCst) > 0);
    assert_same(stored_page(&file, space), &after);
    assert!(matches!(
        section.status(id).unwrap(),
        Some(EditStatus::Published { .. })
    ));
    section.close().unwrap();
    let (section, _) = open(&file, &cache);
    assert_same(section.page(space).unwrap(), &after);
    assert!(section.pending().unwrap().is_empty());
    assert!(matches!(
        section.status(id).unwrap(),
        Some(EditStatus::Published { .. })
    ));
    section.close().unwrap();
    if let Some(destination) = std::env::var_os("ONESTORE_SESSION_NATIVE_EXPORT") {
        let destination = std::path::PathBuf::from(destination);
        std::fs::create_dir(&destination).unwrap();
        let bytes = onestore::read_file(&file).unwrap();
        let identity = onestore::Store::parse(&bytes).unwrap().header.file_id;
        std::fs::write(destination.join("notes.one"), bytes).unwrap();
        std::fs::write(
            destination.join("Open Notebook.onetoc2"),
            onestore::create_table_of_contents("Open Notebook.onetoc2", &[("notes.one", identity)])
                .unwrap(),
        )
        .unwrap();
    }
}

#[test]
fn saves_wait_for_an_unreachable_file_and_publish_after_relaunch() {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("notes.one");
    let cache = directory.path().join("cache");
    std::fs::write(
        &file,
        onestore::create_section("notes.one", "Original", "Author").unwrap(),
    )
    .unwrap();
    let (section, _) = open(&file, &cache);
    let space = section.pages().unwrap()[0].0;
    let before = section.page(space).unwrap();
    use std::os::unix::fs::PermissionsExt;
    let permissions = std::fs::metadata(&file).unwrap().permissions();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o444)).unwrap();
    let after = edited(&before, "Offline ");
    let Save::Queued(id) = section.save(space, &before, &after, "Editor").unwrap() else {
        panic!()
    };
    wait(&section, |event| matches!(event, Event::Unreachable(_)));
    section.close().unwrap();
    assert_eq!(
        notebook::Replica::open(
            std::fs::read_dir(&cache)
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path()
        )
        .unwrap()
        .status(id)
        .unwrap(),
        Some(EditStatus::Pending)
    );
    let (section, _) = open(&file, &cache);
    assert_same(section.page(space).unwrap(), &after);
    assert_eq!(section.pending().unwrap().len(), 1);
    assert_eq!(stored_page(&file, space), before);
    std::fs::set_permissions(&file, permissions).unwrap();
    section.wake();
    published(&section, id);
    assert_same(stored_page(&file, space), &after);
    section.close().unwrap();
}

#[test]
fn an_external_change_refreshes_the_page_and_stales_a_save_from_the_old_model() {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("notes.one");
    let cache = directory.path().join("cache");
    std::fs::write(
        &file,
        onestore::create_section("notes.one", "Original", "Author").unwrap(),
    )
    .unwrap();
    let (section, _) = open(&file, &cache);
    let space = section.pages().unwrap()[0].0;
    let before = section.page(space).unwrap();
    let native = edited(&before, "Native ");
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let bytes = onestore::read_file(&file).unwrap();
        match PreparedEdit::page(&bytes, space, &native, "Native")
            .unwrap()
            .commit_file(&file)
        {
            Ok(()) => break,
            Err(error) if error.error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => panic!("{error:?}"),
        }
    }
    section.wake();
    let deadline = Instant::now() + Duration::from_secs(20);
    while section.page(space).unwrap().title == before.title {
        assert!(Instant::now() < deadline, "the refresh did not arrive");
        wait(&section, |event| matches!(event, Event::Refreshed));
    }
    assert_same(section.page(space).unwrap(), &native);
    let native = section.page(space).unwrap();
    let stale = edited(&before, "Local ");
    assert_eq!(
        section.save(space, &before, &stale, "Editor").unwrap(),
        Save::Stale
    );
    let after = edited(&native, "Local ");
    let Save::Queued(id) = section.save(space, &native, &after, "Editor").unwrap() else {
        panic!()
    };
    published(&section, id);
    assert_same(stored_page(&file, space), &after);
    section.close().unwrap();
}

#[test]
fn a_notebook_directory_lists_its_sections_and_opens_them() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("Personal");
    std::fs::create_dir_all(root.join("Group")).unwrap();
    for (name, text) in [
        ("Personal/First.one", "One"),
        ("Personal/Group/Second.one", "Two"),
    ] {
        std::fs::write(
            directory.path().join(name),
            onestore::create_section(
                Path::new(name).file_name().unwrap().to_str().unwrap(),
                text,
                "Author",
            )
            .unwrap(),
        )
        .unwrap();
    }
    let notebook = Notebook::open(&root, directory.path().join("cache")).unwrap();
    let catalog = notebook.catalog();
    assert_eq!(catalog.sections.len(), 1);
    assert_eq!(catalog.groups.len(), 1);
    assert_eq!(catalog.groups[0].sections.len(), 1);
    let path = catalog.groups[0].sections[0].path.clone();
    let section = notebook.section(&path, || {}).unwrap();
    let (space, _) = section.pages().unwrap()[0];
    let page = section.page(space).unwrap();
    let text = first_text(&page);
    assert_eq!(
        model_ops::paragraph_with(&page, text)
            .unwrap()
            .text()
            .unwrap()
            .text
            .text(),
        "Two"
    );
    section.close().unwrap();
}

#[test]
fn a_notebook_only_opens_discovered_section_paths() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("Notebook");
    let cache = directory.path().join("cache");
    std::fs::create_dir(&root).unwrap();
    let source = onestore::create_section("notes.one", "Original", "Author").unwrap();
    std::fs::write(root.join("notes.one"), &source).unwrap();
    let outside = directory.path().join("outside.one");
    std::fs::write(&outside, &source).unwrap();
    let notebook = Notebook::open(&root, &cache).unwrap();
    std::fs::write(root.join("added.one"), &source).unwrap();
    for path in ["../outside.one", outside.to_str().unwrap(), "added.one"] {
        assert!(matches!(
            notebook.section(path, || {}),
            Err(notebook::Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound
        ));
    }
    assert_eq!(std::fs::read_dir(&cache).unwrap().count(), 0);
    assert_eq!(std::fs::read(&outside).unwrap(), source);
}

#[cfg(unix)]
#[test]
fn a_catalog_section_replaced_by_an_outside_symlink_is_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("Notebook");
    let cache = directory.path().join("cache");
    std::fs::create_dir(&root).unwrap();
    let file = root.join("notes.one");
    let outside = directory.path().join("outside.one");
    let source = onestore::create_section("notes.one", "Original", "Author").unwrap();
    std::fs::write(&file, &source).unwrap();
    std::fs::write(&outside, &source).unwrap();
    let notebook = Notebook::open(&root, &cache).unwrap();
    std::fs::remove_file(&file).unwrap();
    std::os::unix::fs::symlink(&outside, &file).unwrap();
    assert!(matches!(
        notebook.section("notes.one", || {}),
        Err(notebook::Error::Io(error)) if error.kind() == std::io::ErrorKind::PermissionDenied
    ));
    assert_eq!(std::fs::read_dir(&cache).unwrap().count(), 0);
    assert_eq!(std::fs::read(&outside).unwrap(), source);
}

#[test]
fn an_absent_remote_does_not_prevent_local_relaunch_or_further_saves() {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("remote.one");
    let cache = directory.path().join("replica.sqlite");
    let source = onestore::create_section("remote.one", "Original", "Author").unwrap();
    let replica = notebook::Replica::create(&cache, &source).unwrap();
    let section = Section::resume(&file, replica, || {}).unwrap();
    let space = section.pages().unwrap()[0].0;
    let before = section.page(space).unwrap();
    let after = edited(&before, "First ");
    let Save::Queued(id) = section.save(space, &before, &after, "Editor").unwrap() else {
        panic!()
    };
    wait(
        &section,
        |event| matches!(event, Event::Unreachable(error) if error.kind() == std::io::ErrorKind::NotFound),
    );
    assert_eq!(section.status(id).unwrap(), Some(EditStatus::Pending));
    section.close().unwrap();
    assert!(!file.exists());

    let section = Section::resume(&file, notebook::Replica::open(&cache).unwrap(), || {}).unwrap();
    assert_same(section.page(space).unwrap(), &after);
    assert_eq!(section.status(id).unwrap(), Some(EditStatus::Pending));
    let before = section.page(space).unwrap();
    let after = edited(&before, "Second ");
    let Save::Queued(next) = section.save(space, &before, &after, "Editor").unwrap() else {
        panic!()
    };
    assert!(!file.exists());
    let restored = directory.path().join("restored.one");
    std::fs::write(&restored, &source).unwrap();
    std::fs::rename(restored, &file).unwrap();
    section.wake();
    published(&section, next);
    assert_same(stored_page(&file, space), &after);
    section.close().unwrap();
    let section = Section::resume(&file, notebook::Replica::open(&cache).unwrap(), || {}).unwrap();
    assert_same(section.page(space).unwrap(), &after);
    assert!(section.pending().unwrap().is_empty());
    section.close().unwrap();
}

#[test]
fn resuming_against_another_document_preserves_both_remote_and_pending_edit() {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("other.one");
    let cache = directory.path().join("replica.sqlite");
    let source = onestore::create_section("notes.one", "Original", "Author").unwrap();
    let replica = notebook::Replica::create(&cache, &source).unwrap();
    let section = Section::resume(&file, replica, || {}).unwrap();
    let space = section.pages().unwrap()[0].0;
    let before = section.page(space).unwrap();
    let after = edited(&before, "Local ");
    let Save::Queued(id) = section.save(space, &before, &after, "Editor").unwrap() else {
        panic!()
    };
    wait(&section, |event| matches!(event, Event::Unreachable(_)));
    section.close().unwrap();

    let other = onestore::create_section("other.one", "Unrelated", "Other").unwrap();
    std::fs::write(&file, &other).unwrap();
    let section = Section::resume(&file, notebook::Replica::open(&cache).unwrap(), || {}).unwrap();
    wait(&section, |event| matches!(event, Event::Failed(_)));
    assert_eq!(section.status(id).unwrap(), Some(EditStatus::Pending));
    assert_same(section.page(space).unwrap(), &after);
    assert_eq!(std::fs::read(&file).unwrap(), other);
    assert!(section.close().is_err());
    let replica = notebook::Replica::open(&cache).unwrap();
    assert_eq!(replica.status(id).unwrap(), Some(EditStatus::Pending));
    assert_eq!(std::fs::read(&file).unwrap(), other);
}

#[test]
fn offline_save_process() {
    let Some(root) = std::env::var_os("ONENOTE_SESSION_CHILD") else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    let replica = notebook::Replica::open(root.join("replica.sqlite")).unwrap();
    let section = Section::resume(root.join("absent.one"), replica, || {}).unwrap();
    let space = section.pages().unwrap()[0].0;
    let before = section.page(space).unwrap();
    let after = edited(&before, "Durable ");
    let Save::Queued(id) = section.save(space, &before, &after, "Editor").unwrap() else {
        panic!()
    };
    std::fs::write(root.join("acknowledged"), id.to_string()).unwrap();
    // Terminate without running Session or SQLite destructors after acknowledgement.
    std::process::exit(0);
}

#[test]
fn an_acknowledged_offline_save_survives_process_exit_without_cleanup() {
    let directory = tempfile::tempdir().unwrap();
    let cache = directory.path().join("replica.sqlite");
    let source = onestore::create_section("absent.one", "Original", "Author").unwrap();
    drop(notebook::Replica::create(&cache, &source).unwrap());
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "offline_save_process", "--nocapture"])
        .env("ONENOTE_SESSION_CHILD", directory.path())
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let id: u64 = std::fs::read_to_string(directory.path().join("acknowledged"))
        .unwrap()
        .parse()
        .unwrap();
    let file = directory.path().join("absent.one");
    assert!(!file.exists());
    let section = Section::resume(&file, notebook::Replica::open(&cache).unwrap(), || {}).unwrap();
    let space = section.pages().unwrap()[0].0;
    let original = model_ops::page_of(&source, space);
    let after = edited(&original, "Durable ");
    assert_same(section.page(space).unwrap(), &after);
    assert_eq!(section.status(id).unwrap(), Some(EditStatus::Pending));
    section.close().unwrap();
}

#[cfg(feature = "smb")]
#[test]
fn smb_connection_failure_keeps_the_session_locally_editable_and_retries() {
    let directory = tempfile::tempdir().unwrap();
    let cache = directory.path().join("replica.sqlite");
    let source = onestore::create_section("notes.one", "Original", "Author").unwrap();
    let replica = notebook::Replica::create(&cache, &source).unwrap();
    let attempts = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&attempts);
    let section = Section::resume_smb(
        "Folder/notes.one".into(),
        replica,
        1024 * 1024,
        move || {
            counter.fetch_add(1, Ordering::SeqCst);
            Err(std::io::ErrorKind::PermissionDenied.into())
        },
        || {},
    )
    .unwrap();
    wait(
        &section,
        |event| matches!(event, Event::Unreachable(error) if error.kind() == std::io::ErrorKind::PermissionDenied),
    );
    let space = section.pages().unwrap()[0].0;
    let before = section.page(space).unwrap();
    let after = edited(&before, "Offline SMB ");
    let Save::Queued(id) = section.save(space, &before, &after, "Editor").unwrap() else {
        panic!()
    };
    section.wake();
    wait(
        &section,
        |event| matches!(event, Event::Unreachable(error) if error.kind() == std::io::ErrorKind::PermissionDenied),
    );
    assert!(attempts.load(Ordering::SeqCst) >= 2);
    assert_eq!(section.status(id).unwrap(), Some(EditStatus::Pending));
    assert_same(section.page(space).unwrap(), &after);
    assert_eq!(section.file(), Path::new("Folder/notes.one"));
    section.close().unwrap();
    assert_eq!(
        notebook::Replica::open(&cache).unwrap().status(id).unwrap(),
        Some(EditStatus::Pending)
    );
}

#[cfg(feature = "smb")]
#[test]
#[ignore = "requires ONESTORE_SESSION_SMB address and a disposable session.one on its agent share"]
fn live_smb_session_save_publishes_and_reopens() {
    use notebook::smb::{Client, Credentials};
    let address = std::env::var("ONESTORE_SESSION_SMB").unwrap();
    let connect = move || {
        Client::connect(
            &address,
            "agent",
            Credentials::default(),
            Duration::from_secs(5),
        )
    };
    let client = connect().unwrap();
    let source = client.read("session.one", 1024 * 1024).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let cache = directory.path().join("replica.sqlite");
    let replica = notebook::Replica::create(&cache, &source).unwrap();
    let section = Section::resume_smb(
        "session.one".into(),
        replica,
        1024 * 1024,
        connect.clone(),
        || {},
    )
    .unwrap();
    let space = section.pages().unwrap()[0].0;
    let before = section.page(space).unwrap();
    let after = edited(&before, "Session SMB ");
    let Save::Queued(id) = section.save(space, &before, &after, "Editor").unwrap() else {
        panic!()
    };
    published(&section, id);
    let remote = client.read("session.one", 1024 * 1024).unwrap();
    assert_same(model_ops::page_of(&remote, space), &after);
    section.close().unwrap();
    let section = Section::resume_smb(
        "session.one".into(),
        notebook::Replica::open(&cache).unwrap(),
        1024 * 1024,
        connect,
        || {},
    )
    .unwrap();
    assert_same(section.page(space).unwrap(), &after);
    assert!(section.pending().unwrap().is_empty());
    assert!(matches!(
        section.status(id).unwrap(),
        Some(EditStatus::Published { .. })
    ));
    section.close().unwrap();
}

#[cfg(feature = "smb")]
#[test]
fn dropping_during_connection_keeps_cache_owned_until_the_worker_finishes() {
    use std::sync::mpsc;
    let directory = tempfile::tempdir().unwrap();
    let cache = directory.path().join("replica.sqlite");
    let source = onestore::create_section("notes.one", "Original", "Author").unwrap();
    let replica = notebook::Replica::create(&cache, &source).unwrap();
    let (entered, connecting) = mpsc::channel();
    let (release, stalled) = mpsc::channel();
    let section = Section::resume_smb(
        "notes.one".into(),
        replica,
        1024 * 1024,
        move || {
            entered.send(()).unwrap();
            stalled.recv_timeout(Duration::from_secs(10)).unwrap();
            Err(std::io::ErrorKind::TimedOut.into())
        },
        || {},
    )
    .unwrap();
    connecting.recv_timeout(Duration::from_secs(5)).unwrap();
    let space = section.pages().unwrap()[0].0;
    let before = section.page(space).unwrap();
    let after = edited(&before, "Saved during connection ");
    let Save::Queued(id) = section.save(space, &before, &after, "Editor").unwrap() else {
        panic!()
    };
    let (dropped, finished) = mpsc::channel();
    let owner = std::thread::spawn(move || {
        drop(section);
        dropped.send(()).unwrap();
    });
    finished.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(matches!(
        notebook::Replica::open(&cache),
        Err(notebook::Error::Database(error))
            if error.sqlite_error_code() == Some(rusqlite::ErrorCode::DatabaseBusy)
    ));
    release.send(()).unwrap();
    owner.join().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let replica = loop {
        match notebook::Replica::open(&cache) {
            Ok(replica) => break replica,
            Err(notebook::Error::Database(error))
                if error.sqlite_error_code() == Some(rusqlite::ErrorCode::DatabaseBusy) =>
            {
                assert!(Instant::now() < deadline, "worker retained cache ownership");
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("{error}"),
        }
    };
    assert_eq!(replica.status(id).unwrap(), Some(EditStatus::Pending));
    assert_same(
        model_ops::page_of(&replica.snapshot().unwrap(), space),
        &after,
    );
    assert!(connecting.try_recv().is_err());
}

#[test]
fn a_conflicting_save_is_reviewed_against_the_remote_page_and_archived_for_recovery() {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("notes.one");
    let cache = directory.path().join("cache");
    std::fs::write(
        &file,
        onestore::create_section("notes.one", "Original", "Author").unwrap(),
    )
    .unwrap();
    let (section, _) = open(&file, &cache);
    let space = section.pages().unwrap()[0].0;
    let before = section.page(space).unwrap();
    let text = first_text(&before);
    use std::os::unix::fs::PermissionsExt;
    let permissions = std::fs::metadata(&file).unwrap().permissions();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o444)).unwrap();
    let mut local = before.clone();
    model_ops::replace_text(&mut local, text, 0..8, "Local");
    let Save::Queued(id) = section.save(space, &before, &local, "Editor").unwrap() else {
        panic!()
    };
    wait(&section, |event| matches!(event, Event::Unreachable(_)));
    std::fs::set_permissions(&file, permissions).unwrap();
    let mut native = before.clone();
    model_ops::replace_text(&mut native, text, 0..8, "Native");
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let bytes = onestore::read_file(&file).unwrap();
        match PreparedEdit::page(&bytes, space, &native, "Native")
            .unwrap()
            .commit_file(&file)
        {
            Ok(()) => break,
            Err(error) if error.error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => panic!("{error:?}"),
        }
    }
    section.wake();
    wait(
        &section,
        |event| matches!(event, Event::Attempt { id: n, status: EditStatus::Conflict(ConflictKind::ContentChanged) } if *n == id),
    );
    let conflicts = section.conflicts().unwrap();
    assert_eq!(
        conflicts,
        [(
            QueuedEdit {
                id,
                space,
                status: EditStatus::Conflict(ConflictKind::ContentChanged)
            },
            ConflictKind::ContentChanged
        )]
    );
    assert_same(section.page(space).unwrap(), &local);
    let remote = section.remote_page(space).unwrap();
    assert_same(remote.clone(), &native);
    let archive = directory.path().join("review.sqlite");
    section.export_recovery(&archive).unwrap();
    let recovery = Recovery::open(&archive).unwrap();
    assert_eq!(recovery.pending().unwrap().len(), 1);
    assert_eq!(
        recovery.status(id).unwrap(),
        Some(EditStatus::Conflict(ConflictKind::ContentChanged))
    );
    let mut reviewed = remote.clone();
    model_ops::replace_text(&mut reviewed, text, 0..6, "Native and local");
    section.review(id, &reviewed).unwrap();
    assert_eq!(section.status(id).unwrap(), Some(EditStatus::Pending));
    published(&section, id);
    assert_same(stored_page(&file, space), &reviewed);
    assert!(section.queue().unwrap().is_empty());
    section.close().unwrap();
}

#[path = "support/server.rs"]
mod server;

/// An uncertain attempt survives a restart as `AwaitingConfirmation`; after exporting the
/// archive the user either continues from a reviewed page or abandons the branch. Neither
/// path records a receipt for the uncertain attempt.
#[test]
fn an_uncertain_attempt_is_released_after_restart_by_review() {
    for continued in [true, false] {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("notes.one");
        let cache = directory.path().join("cache.sqlite");
        let source = onestore::create_section("notes.one", "Original", "Author").unwrap();
        std::fs::write(&file, &source).unwrap();
        let replica = notebook::Replica::create(&cache, &source).unwrap();
        let space = space_of(&source);
        let page = model_ops::page_of(&source, space);
        let text = first_text(&page);
        let local = edited(&page, "Uncertain ");
        let id = replica
            .save(&source, space, &local, "Author")
            .unwrap()
            .unwrap();
        let mut faulty = server::Server::new(&source);
        faulty.fault = server::Fault::UnknownBefore;
        assert!(replica.sync_once(&mut faulty).is_err());
        assert!(matches!(
            replica.status(id).unwrap(),
            Some(EditStatus::AwaitingConfirmation { .. })
        ));
        drop(replica);

        let notified = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&notified);
        let section = Section::resume(&file, notebook::Replica::open(&cache).unwrap(), move || {
            counter.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();
        wait(
            &section,
            |event| matches!(event, Event::Attempt { id: n, status: EditStatus::AwaitingConfirmation { .. } } if *n == id),
        );
        assert_eq!(
            section
                .queue()
                .unwrap()
                .iter()
                .map(|edit| edit.id)
                .collect::<Vec<_>>(),
            [id]
        );
        assert_same(section.page(space).unwrap(), &local);
        let archive = directory.path().join("review.sqlite");
        if continued {
            let mut reviewed = section.remote_page(space).unwrap();
            model_ops::replace_text(&mut reviewed, text, 0..0, "Reviewed ");
            section.release(id, &archive, Some(&reviewed)).unwrap();
            assert_eq!(section.status(id).unwrap(), Some(EditStatus::Pending));
            published(&section, id);
            assert_same(stored_page(&file, space), &reviewed);
        } else {
            section.release(id, &archive, None).unwrap();
            assert_eq!(
                section.status(id).unwrap(),
                Some(EditStatus::Archived {
                    archive: archive.to_string_lossy().into_owned()
                })
            );
            assert_same(section.page(space).unwrap(), &page);
            assert_eq!(stored_page(&file, space), page);
        }
        let recovery = Recovery::open(&archive).unwrap();
        assert_eq!(recovery.pending().unwrap().len(), 1);
        assert!(matches!(
            recovery.status(id).unwrap(),
            Some(EditStatus::AwaitingConfirmation { .. })
        ));
        assert!(section.queue().unwrap().is_empty());
        assert!(
            section
                .release(id, directory.path().join("again.sqlite"), None)
                .is_err()
        );
        section.close().unwrap();
    }
}

fn space_of(source: &[u8]) -> ExGuid {
    let store = onestore::Store::parse(source).unwrap();
    let index = onestore::RevisionIndex::parse(&store).unwrap();
    onestore::document::Document::parse(&index)
        .unwrap()
        .pages()
        .unwrap()[0]
        .0
}
