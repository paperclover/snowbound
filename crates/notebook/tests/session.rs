#[path = "../../onestore/tests/support/ops.rs"]
mod ops;
use notebook::{
    EditStatus, Recovery, Resolution,
    session::{Event, Notebook, Section, SyncStatus},
};
use onestore::{
    ExGuid,
    op::{Edit, Op, PageOp},
    page::Page,
};
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

/// Replaces `range` of the page's first body text as the editor does: the edit's id once
/// it is durable.
fn typed(
    section: &Section,
    space: ExGuid,
    page: &Page,
    range: std::ops::Range<u32>,
    text: &str,
) -> u64 {
    let op = PageOp::Text {
        text: first_text(page),
        range,
        with: text.into(),
    };
    let edit = Edit {
        at: model_ops::now(),
        ops: vec![Op::Page { space, op }],
    };
    section.replica().apply("Editor", edit).unwrap()
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
    let deadline = Instant::now() + Duration::from_secs(20);
    while !matches!(
        section.status(id).unwrap(),
        Some(EditStatus::Published { .. })
    ) {
        assert!(Instant::now() < deadline, "the edit was not published");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn status_until(section: &Section, accept: impl Fn(&SyncStatus) -> bool) -> SyncStatus {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let status = section.sync_status().unwrap();
        if accept(&status) {
            return status;
        }
        assert!(Instant::now() < deadline, "the status stayed {status:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
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
fn a_section_is_shared_between_threads() {
    fn shared<T: Send + Sync>() {}
    shared::<Section>();
    shared::<notebook::Replica>();
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
    let after = edited(&before, "Saved ");
    let id = typed(&section, space, &before, 0..0, "Saved ");
    assert_same(section.page(space).unwrap(), &after);
    published(&section, id);
    // The worker reports the step after writing its receipt.
    let deadline = Instant::now() + Duration::from_secs(20);
    while notified.load(Ordering::SeqCst) == 0 {
        assert!(Instant::now() < deadline, "the host was not woken");
        std::thread::sleep(Duration::from_millis(10));
    }
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
    let id = typed(&section, space, &before, 0..0, "Offline ");
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
fn an_external_change_reloads_the_page_and_later_edits_apply_to_it() {
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
        match ops::save(&bytes, space, &native)
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
        |event| matches!(event, Event::Changed(spaces) if spaces.contains(&space)),
    );
    assert_same(section.page(space).unwrap(), &native);
    // An edit made on the page shown before the change applies to the page as it is now.
    let id = typed(&section, space, &before, 0..0, "Local ");
    let after = edited(&native, "Local ");
    published(&section, id);
    assert_same(stored_page(&file, space), &after);
    section.close().unwrap();
}

#[test]
fn edits_apply_in_order_without_waiting_and_finish_before_close() {
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
    let original = section.page(space).unwrap();
    let text = first_text(&original);
    let apply = |word: &str| {
        let op = PageOp::Text {
            text,
            range: 0..0,
            with: word.into(),
        };
        let edit = Edit {
            at: model_ops::now(),
            ops: vec![Op::Page { space, op }],
        };
        section.apply("Editor", edit).unwrap();
    };
    let mut expected = original.clone();
    for word in ["one ", "two ", "three ", "four ", "five "] {
        apply(word);
        expected = edited(&expected, word);
    }
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let mut stored = section.page(space).unwrap();
        stored.title = expected.title.clone();
        if stored == expected {
            break;
        }
        assert!(Instant::now() < deadline, "the edits did not arrive");
        std::thread::sleep(Duration::from_millis(20));
    }
    while !section.pending().unwrap().is_empty() {
        assert!(Instant::now() < deadline, "the edits were not published");
        std::thread::sleep(Duration::from_millis(20));
    }
    let native = edited(&section.page(space).unwrap(), "Native ");
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let bytes = onestore::read_file(&file).unwrap();
        match ops::save(&bytes, space, &native)
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
        |event| matches!(event, Event::Changed(spaces) if spaces.contains(&space)),
    );
    apply("Local ");
    section.close().unwrap();
    let (section, _) = open(&file, &cache);
    assert_same(section.page(space).unwrap(), &edited(&native, "Local "));
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
    let (space, _, _) = section.pages().unwrap()[0];
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
    let id = typed(&section, space, &before, 0..0, "First ");
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
    let next = typed(&section, space, &before, 0..0, "Second ");
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
    let id = typed(&section, space, &before, 0..0, "Local ");
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
    let id = typed(&section, space, &before, 0..0, "Durable ");
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
    let id = typed(&section, space, &before, 0..0, "Offline SMB ");
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
    let id = typed(&section, space, &before, 0..0, "Session SMB ");
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
            stalled.recv_timeout(Duration::from_secs(120)).unwrap();
            Err(std::io::ErrorKind::TimedOut.into())
        },
        || {},
    )
    .unwrap();
    connecting.recv_timeout(Duration::from_secs(120)).unwrap();
    let space = section.pages().unwrap()[0].0;
    let before = section.page(space).unwrap();
    let after = edited(&before, "Saved during connection ");
    let id = typed(&section, space, &before, 0..0, "Saved during connection ");
    let (dropped, finished) = mpsc::channel();
    let owner = std::thread::spawn(move || {
        drop(section);
        dropped.send(()).unwrap();
    });
    finished.recv_timeout(Duration::from_secs(60)).unwrap();
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
        model_ops::page_of(&server::snapshot(&replica), space),
        &after,
    );
    assert!(connecting.try_recv().is_err());
}

#[test]
fn a_conflicting_save_keeps_the_native_page_and_a_conflict_page_the_session_deletes() {
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
    let id = typed(&section, space, &before, 0..8, "Local");
    wait(&section, |event| matches!(event, Event::Unreachable(_)));
    std::fs::set_permissions(&file, permissions).unwrap();
    let mut native = before.clone();
    model_ops::replace_text(&mut native, text, 0..8, "Native");
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let bytes = onestore::read_file(&file).unwrap();
        match ops::save(&bytes, space, &native)
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
        |event| matches!(event, Event::Attempt { id: n, status: EditStatus::Published { .. } } if *n > id),
    );
    assert!(matches!(
        section.status(id).unwrap(),
        Some(EditStatus::Published { .. })
    ));
    // As OneNote does, the native version stays the page and the local one is kept beside it.
    assert_same(section.page(space).unwrap(), &native);
    assert_same(stored_page(&file, space), &native);
    let listed = section.conflicts().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].0, space);
    let conflict = &listed[0].1[0];
    assert_eq!(conflict.user, "Editor");
    let kept = section.page(conflict.space).unwrap();
    assert_eq!(
        model_ops::paragraph_with(&kept, first_text(&kept))
            .and_then(|paragraph| paragraph.text())
            .map(|text| text.text.text().to_owned()),
        Some("Local".to_owned())
    );
    assert_eq!(
        server::conflicts(&onestore::read_file(&file).unwrap())[0].1,
        [("Editor".to_owned(), vec!["Local".to_owned()])]
    );
    // Merged by hand, the conflict page is deleted.
    let deleted = section.delete_pages(&[conflict.space]).unwrap();
    published(&section, deleted);
    assert!(section.conflicts().unwrap().is_empty());
    assert!(server::conflicts(&onestore::read_file(&file).unwrap()).is_empty());
    assert!(section.pending().unwrap().is_empty());
    section.close().unwrap();
}

#[path = "support/server.rs"]
mod server;

/// An uncertain attempt survives a restart as `AwaitingConfirmation`; after exporting the
/// archive the user either publishes the local edits again or abandons the branch. Neither
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
        let local = edited(&page, "Uncertain ");
        let id = model_ops::save_as(&replica, space, &local, "Author")
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
                .pending()
                .unwrap()
                .iter()
                .map(|edit| edit.id)
                .collect::<Vec<_>>(),
            [id]
        );
        assert_same(section.page(space).unwrap(), &local);
        let archive = directory.path().join("review.sqlite");
        if continued {
            section.release(id, &archive, Resolution::Mine).unwrap();
            assert_eq!(section.status(id).unwrap(), Some(EditStatus::Pending));
            published(&section, id);
            assert_same(stored_page(&file, space), &local);
        } else {
            section.release(id, &archive, Resolution::Theirs).unwrap();
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
        assert!(section.pending().unwrap().is_empty());
        assert!(
            section
                .release(
                    id,
                    directory.path().join("again.sqlite"),
                    Resolution::Theirs
                )
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

/// Restoring and deleting page versions go through the queue and reach the file as OneNote
/// 2010 stores them (`corpus/page-versions`); a publication that changes only a page's
/// versions confirms by the history revision it wrote.
#[test]
fn page_versions_restore_and_delete_through_the_session() {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("History.one");
    let cache = directory.path().join("cache");
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../corpus/page-versions/native/step-02/notebook/History.one"),
        &file,
    )
    .unwrap();
    let (section, _) = open(&file, &cache);
    let listed = section.versions().unwrap();
    let (space, versions) = listed[0].clone();
    assert_eq!(versions.len(), 1);
    let standing = section.page(space).unwrap();
    let old = section.version(space, versions[0].context).unwrap();
    let restored = section
        .restore_version(space, versions[0].context, "Editor")
        .unwrap();
    published(&section, restored);
    assert_eq!(section.page(space).unwrap().objects, old.objects);
    let stored = |file: &Path| {
        let arena = onestore::Arena::default();
        let mut stored =
            onestore::Section::open(&arena, onestore::read_file(file).unwrap()).unwrap();
        let versions = stored.versions().unwrap();
        (stored.page(space).unwrap(), versions)
    };
    let (page, after) = stored(&file);
    assert_eq!(page.objects, old.objects);
    assert_eq!(after[0].1.len(), 2);
    assert_eq!(after[0].1[0].author.as_deref(), Some("Other Person"));
    assert_eq!(
        section.version(space, after[0].1[0].context).unwrap(),
        standing
    );
    section.close().unwrap();

    // After a relaunch, deleting every version publishes the history alone.
    let (section, _) = open(&file, &cache);
    let all: Vec<ExGuid> = section.versions().unwrap()[0]
        .1
        .iter()
        .map(|version| version.context)
        .collect();
    let deleted = section.delete_versions(&[(space, all)]).unwrap();
    published(&section, deleted);
    assert!(section.versions().unwrap().is_empty());
    let (page, after) = stored(&file);
    assert!(after.is_empty());
    assert_eq!(page.objects, old.objects);
    section.close().unwrap();
}

#[test]
fn working_offline_queues_edits_until_sync_now_or_working_online() {
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
    let reached = status_until(&section, |status| status.synced.is_some());
    assert!(reached.error.is_none());
    assert_eq!(reached.queued, 0);

    section.set_offline(true);
    let id = typed(&section, space, &before, 0..0, "Offline ");
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(section.status(id).unwrap(), Some(EditStatus::Pending));
    assert_eq!(section.sync_status().unwrap().queued, 1);
    assert_eq!(stored_page(&file, space), before);

    // Sync Now publishes while working offline.
    section.wake();
    published(&section, id);
    let after = edited(&before, "Offline ");
    assert_same(stored_page(&file, space), &after);
    let synced = status_until(&section, |status| status.queued == 0);
    assert!(synced.synced > reached.synced);

    let next = typed(&section, space, &after, 0..0, "Again ");
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(section.status(next).unwrap(), Some(EditStatus::Pending));
    section.set_offline(false);
    published(&section, next);
    status_until(&section, |status| status.queued == 0);
    section.close().unwrap();
}

#[test]
fn a_missing_section_file_reports_its_error_until_it_returns() {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("notes.one");
    let away = directory.path().join("away.one");
    let cache = directory.path().join("cache");
    std::fs::write(
        &file,
        onestore::create_section("notes.one", "Original", "Author").unwrap(),
    )
    .unwrap();
    let (section, notified) = open(&file, &cache);
    status_until(&section, |status| status.synced.is_some());
    std::fs::rename(&file, &away).unwrap();
    section.wake();
    let failed = status_until(&section, |status| status.error.is_some());
    assert_eq!(
        failed.error.map(|error| error.kind()),
        Some(std::io::ErrorKind::NotFound)
    );
    let before = notified.load(Ordering::SeqCst);
    std::fs::rename(&away, &file).unwrap();
    section.wake();
    let back = status_until(&section, |status| status.error.is_none());
    assert!(back.synced > failed.synced);
    assert!(notified.load(Ordering::SeqCst) > before);
    section.close().unwrap();
}

/// A notebook folder holding `First.one` and `Second.one`, and a cache beside it.
fn two_sections(directory: &Path) -> Notebook {
    let root = directory.join("Shared");
    std::fs::create_dir(&root).unwrap();
    for name in ["First", "Second"] {
        let file = format!("{name}.one");
        std::fs::write(
            root.join(&file),
            onestore::create_section(&file, name, "Author").unwrap(),
        )
        .unwrap();
    }
    Notebook::open(&root, directory.join("cache")).unwrap()
}

fn until(what: &str, mut accept: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !accept() {
        assert!(Instant::now() < deadline, "{what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_closed_sections_queued_edits_publish_in_the_background_when_online() {
    let directory = tempfile::tempdir().unwrap();
    let notebook = two_sections(directory.path());
    let file = directory.path().join("Shared/Second.one");
    let background = notebook
        .background(Duration::from_millis(20), || {})
        .unwrap();
    background.set_offline(true);
    background.watch(notebook.replicas());
    // The round started with the background ends before the edit exists.
    std::thread::sleep(Duration::from_millis(200));
    let section = notebook.section("Second.one", || {}).unwrap();
    section.set_offline(true);
    let space = section.pages().unwrap()[0].0;
    let before = section.page(space).unwrap();
    typed(&section, space, &before, 0..0, "Closed ");
    section.close().unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        stored_page(&file, space),
        before,
        "working offline publishes nothing"
    );
    background.set_offline(false);
    let mut after = edited(&before, "Closed ");
    until("the closed section published", || {
        let stored = stored_page(&file, space);
        after.title = stored.title.clone();
        stored == after
    });
    until("the status shows nothing waiting", || {
        background.status().iter().any(|(path, status)| {
            path == "Second.one" && status.synced.is_some() && status.queued == 0
        })
    });
    drop(background);
    let mut reopened = None;
    until("the background released the replica", || {
        reopened = notebook.section("Second.one", || {}).ok();
        reopened.is_some()
    });
    let reopened = reopened.unwrap();
    assert!(reopened.pending().unwrap().is_empty());
    reopened.close().unwrap();
}

#[test]
fn a_remote_change_to_a_closed_section_is_noticed_and_rebases_its_replica() {
    let directory = tempfile::tempdir().unwrap();
    let notebook = two_sections(directory.path());
    // Second has a replica from being opened once; First has never been opened.
    notebook
        .section("Second.one", || {})
        .unwrap()
        .close()
        .unwrap();
    let notified = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&notified);
    let background = notebook
        .background(Duration::from_millis(20), move || {
            counter.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();
    background.watch(notebook.replicas());
    until("both sections were reached", || {
        background
            .status()
            .iter()
            .all(|(_, status)| status.synced.is_some() && status.error.is_none())
    });
    assert!(background.changed().is_empty());
    let mut changed = Vec::new();
    for name in ["First.one", "Second.one"] {
        let file = directory.path().join("Shared").join(name);
        let bytes = onestore::read_file(&file).unwrap();
        let space = notebook::session::stored_pages(&bytes).unwrap()[0].space;
        let native = edited(&model_ops::page_of(&bytes, space), "Native ");
        ops::save(&bytes, space, &native)
            .unwrap()
            .commit_file(&file)
            .unwrap();
        changed.push((name, space, native));
    }
    let mut noticed = Vec::new();
    until("both changes were noticed", || {
        noticed.extend(background.changed());
        changed
            .iter()
            .all(|(name, ..)| noticed.iter().any(|path| path == name))
    });
    assert!(notified.load(Ordering::SeqCst) > 0);
    drop(background);
    let (_, space, native) = &changed[1];
    let replica = notebook.replica_path("Second.one").unwrap();
    let mut opened = None;
    until("the background released the replica", || {
        opened = notebook::Replica::open(&replica).ok();
        opened.is_some()
    });
    assert_same(opened.unwrap().page(*space).unwrap(), native);
}
