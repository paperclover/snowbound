use notebook::{EditStatus, Error, Operation, Recovery, Replica};
use std::{
    fs, io,
    path::Path,
    sync::{Barrier, mpsc},
    time::Duration,
};

const FIXTURE: &str = "../../corpus/native-external-assets/notebook";
const LEGACY: &str = "../../corpus/offline-v4/live.sqlite";

fn copied_cache(root: &Path) -> Replica {
    let path = root.join("cache.sqlite");
    assert!(!path.exists());
    fs::copy(LEGACY, &path).unwrap();
    Replica::open(path).unwrap()
}

fn payload(size: usize) -> (String, Vec<u8>) {
    fs::read_dir(Path::new(FIXTURE).join("synthetic_onefiles"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find_map(|path| {
            let bytes = fs::read(&path).unwrap();
            (bytes.len() == size)
                .then(|| (path.file_name().unwrap().to_str().unwrap().into(), bytes))
        })
        .unwrap()
}

struct Payload(Option<io::Result<Vec<u8>>>);
impl notebook::discover::Source for Payload {
    fn entries(&mut self, _: &str, _: usize) -> io::Result<Vec<notebook::discover::Entry>> {
        panic!("Unexpected enumeration")
    }
    fn read(&mut self, _: &str, _: usize) -> io::Result<Vec<u8>> {
        self.0.take().expect("Unexpected repeated payload read")
    }
}

#[test]
fn downloaded_media_survives_migration_reopen_and_recovery_with_the_queue_intact() {
    let directory = tempfile::tempdir().unwrap();
    let original = fs::read(LEGACY).unwrap();
    let cache = copied_cache(directory.path());
    let working = cache.snapshot().unwrap();
    let remote = cache.remote_snapshot().unwrap();
    let pending = cache.pending().unwrap();
    assert_eq!(
        pending.iter().map(|edit| edit.id).collect::<Vec<_>>(),
        [1, 2]
    );
    let uncertain = cache.status(1).unwrap();
    assert!(matches!(
        uncertain,
        Some(EditStatus::AwaitingConfirmation { .. })
    ));
    let mut source = notebook::discover::Local::open(FIXTURE).unwrap();
    for size in [0, 1024] {
        let (name, bytes) = payload(size);
        assert!(cache.cached_asset(&name, size).unwrap().is_none());
        assert_eq!(
            cache
                .fetch_asset(&mut source, "synthetic.one", &name, size)
                .unwrap(),
            bytes
        );
        assert_eq!(
            cache
                .cached_asset(&name.to_ascii_lowercase(), size)
                .unwrap(),
            Some(bytes)
        );
    }
    let summary = cache.recovery_summary().unwrap();
    assert_eq!(
        (summary.cached_assets, summary.cached_asset_bytes),
        (2, 1024)
    );
    assert_eq!(cache.snapshot().unwrap(), working);
    assert_eq!(cache.remote_snapshot().unwrap(), remote);
    assert_eq!(cache.pending().unwrap(), pending);
    assert_eq!(cache.status(1).unwrap(), uncertain);
    cache
        .export_recovery(directory.path().join("recovery.sqlite"))
        .unwrap();
    drop(cache);
    let cache = Replica::open(directory.path().join("cache.sqlite")).unwrap();
    let recovery = Recovery::open(directory.path().join("recovery.sqlite")).unwrap();
    assert_eq!(recovery.summary().unwrap(), summary);
    assert_eq!(recovery.pending().unwrap(), pending);
    assert_eq!(recovery.status(1).unwrap(), uncertain);
    assert!(recovery.receipts().unwrap().is_empty());
    for size in [0, 1024] {
        let (name, bytes) = payload(size);
        assert_eq!(
            cache.cached_asset(&name, size).unwrap(),
            Some(bytes.clone())
        );
        assert_eq!(recovery.cached_asset(&name, size).unwrap(), Some(bytes));
    }
    assert_eq!(fs::read(LEGACY).unwrap(), original);
}

#[test]
fn an_original_version_four_archive_remains_read_only_and_has_no_cached_media() {
    let path = "../../corpus/offline-v4/recovery.sqlite";
    let before = fs::read(path).unwrap();
    let archive = Recovery::open(path).unwrap();
    assert_eq!(archive.pending().unwrap().len(), 2);
    assert!(matches!(
        archive.status(1).unwrap(),
        Some(EditStatus::AwaitingConfirmation { .. })
    ));
    assert_eq!(archive.summary().unwrap().cached_assets, 0);
    assert_eq!(archive.summary().unwrap().cached_asset_bytes, 0);
    assert!(archive.cached_asset(&payload(0).0, 0).unwrap().is_none());
    drop(archive);
    assert_eq!(fs::read(path).unwrap(), before);
}

#[test]
fn unsuccessful_refreshes_and_changed_identity_data_preserve_the_downloaded_payload() {
    let directory = tempfile::tempdir().unwrap();
    let cache = copied_cache(directory.path());
    let (name, bytes) = payload(1024);
    let mut source = Payload(Some(Ok(bytes.clone())));
    cache
        .fetch_asset(&mut source, "synthetic.one", &name, 1024)
        .unwrap();
    let before = fs::read(directory.path().join("cache.sqlite")).unwrap();
    let mut unavailable = Payload(Some(Err(io::ErrorKind::ConnectionReset.into())));
    assert!(
        matches!(cache.fetch_asset(&mut unavailable, "synthetic.one", &name, 1024),
        Err(Error::Discovery(notebook::discover::Error::Io { error, .. })) if error.kind() == io::ErrorKind::ConnectionReset)
    );
    for size in [0, 1024, 2048] {
        let mut changed = Payload(Some(Ok(vec![9; size])));
        assert!(matches!(
            cache.fetch_asset(&mut changed, "synthetic.one", &name, 2048),
            Err(Error::AssetChanged)
        ));
    }
    assert!(
        matches!(cache.cached_asset(&name, 1023), Err(Error::Io(error)) if error.kind() == io::ErrorKind::FileTooLarge)
    );
    assert_eq!(cache.cached_asset(&name, 1024).unwrap(), Some(bytes));
    assert_eq!(
        fs::read(directory.path().join("cache.sqlite")).unwrap(),
        before
    );
}

#[test]
fn unreferenced_payloads_are_rejected_before_io_or_local_changes() {
    let directory = tempfile::tempdir().unwrap();
    let cache = copied_cache(directory.path());
    let mut unused = Payload(None);
    assert!(
        matches!(cache.fetch_asset(&mut unused, "synthetic.one", "00000000-0000-0000-0000-000000000001.onebin", 100),
        Err(Error::Io(error)) if error.kind() == io::ErrorKind::InvalidInput)
    );
    assert!(
        cache
            .fetch_asset(&mut unused, "synthetic.one", "../payload.onebin", 100)
            .is_err()
    );
    assert_eq!(cache.recovery_summary().unwrap().cached_assets, 0);
}

#[test]
fn download_network_wait_does_not_block_local_edits() {
    struct Waiting {
        entered: mpsc::Sender<()>,
        released: mpsc::Receiver<()>,
        bytes: Vec<u8>,
    }
    impl notebook::discover::Source for Waiting {
        fn entries(&mut self, _: &str, _: usize) -> io::Result<Vec<notebook::discover::Entry>> {
            panic!("Unexpected enumeration")
        }
        fn read(&mut self, _: &str, _: usize) -> io::Result<Vec<u8>> {
            self.entered.send(()).unwrap();
            self.released.recv_timeout(Duration::from_secs(5)).unwrap();
            Ok(self.bytes.clone())
        }
    }
    let directory = tempfile::tempdir().unwrap();
    let cache = copied_cache(directory.path());
    let (name, bytes) = payload(1024);
    let (entered, waiting) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let mut source = Waiting {
        entered,
        released,
        bytes: bytes.clone(),
    };
    std::thread::scope(|scope| {
        let download = scope.spawn(|| {
            cache
                .fetch_asset(&mut source, "synthetic.one", &name, 1024)
                .unwrap()
        });
        waiting.recv_timeout(Duration::from_secs(5)).unwrap();
        let pending = cache.pending().unwrap();
        let Operation::Text(edit) = &pending[0].operation else {
            panic!()
        };
        let id = cache
            .edit_text(
                &cache.snapshot().unwrap(),
                pending[0].space,
                edit.object,
                0..0,
                "during download ",
            )
            .unwrap()
            .unwrap();
        release.send(()).unwrap();
        assert_eq!(download.join().unwrap(), bytes);
        assert_eq!(cache.status(id).unwrap(), Some(EditStatus::Pending));
    });
}

#[test]
fn concurrent_downloads_publish_one_immutable_cache_entry() {
    let directory = tempfile::tempdir().unwrap();
    let cache = copied_cache(directory.path());
    let (name, bytes) = payload(1024);
    let ready = Barrier::new(8);
    std::thread::scope(|scope| {
        for _ in 0..8 {
            scope.spawn(|| {
                let mut source = notebook::discover::Local::open(FIXTURE).unwrap();
                ready.wait();
                assert_eq!(
                    cache
                        .fetch_asset(&mut source, "synthetic.one", &name, 1024)
                        .unwrap(),
                    bytes
                );
            });
        }
    });
    assert_eq!(cache.recovery_summary().unwrap().cached_assets, 1);
}

#[test]
fn a_native_refresh_removing_the_reference_rejects_an_inflight_download() {
    struct Native;
    impl notebook::Remote for Native {
        fn read(&mut self) -> io::Result<Vec<u8>> {
            fs::read("../../corpus/native-external-assets/native/synthetic.one")
        }
        fn publish(&mut self, _: &onestore::PreparedEdit<'_>) -> Result<(), onestore::CommitError> {
            panic!("Unexpected publication")
        }
        fn confirm(&mut self, _: &[u8]) -> Result<(), onestore::CommitError> {
            panic!("Unexpected confirmation")
        }
    }
    struct Refresh<'a>(&'a Replica);
    impl notebook::discover::Source for Refresh<'_> {
        fn entries(&mut self, _: &str, _: usize) -> io::Result<Vec<notebook::discover::Entry>> {
            panic!("Unexpected enumeration")
        }
        fn read(&mut self, _: &str, _: usize) -> io::Result<Vec<u8>> {
            assert_eq!(self.0.sync_once(&mut Native).unwrap(), None);
            Ok(payload(1024).1)
        }
    }
    let root = tempfile::tempdir().unwrap();
    let source = fs::read(Path::new(FIXTURE).join("synthetic.one")).unwrap();
    let cache = Replica::create(root.path().join("cache.sqlite"), &source).unwrap();
    let (name, _) = payload(1024);
    assert!(
        matches!(cache.fetch_asset(&mut Refresh(&cache), "synthetic.one", &name, 1024),
        Err(Error::Io(error)) if error.kind() == io::ErrorKind::ResourceBusy)
    );
    assert!(cache.cached_asset(&name, 1024).unwrap().is_none());
    assert_eq!(
        cache.snapshot().unwrap(),
        fs::read("../../corpus/native-external-assets/native/synthetic.one").unwrap()
    );
}

#[test]
fn local_failure_and_bad_cached_bytes_never_become_successful_downloads() {
    let directory = tempfile::tempdir().unwrap();
    drop(copied_cache(directory.path()));
    let path = directory.path().join("cache.sqlite");
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection.execute_batch("CREATE TRIGGER fail_asset BEFORE INSERT ON assets BEGIN SELECT RAISE(ABORT,'Test asset failure'); END").unwrap();
    drop(connection);
    let (name, bytes) = payload(1024);
    let cache = Replica::open(&path).unwrap();
    let mut source = notebook::discover::Local::open(FIXTURE).unwrap();
    assert!(matches!(
        cache.fetch_asset(&mut source, "synthetic.one", &name, 1024),
        Err(Error::Database(_))
    ));
    assert!(cache.cached_asset(&name, 1024).unwrap().is_none());
    assert_eq!(cache.pending().unwrap().len(), 2);
    drop(cache);
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection.execute_batch("DROP TRIGGER fail_asset").unwrap();
    drop(connection);
    let cache = Replica::open(&path).unwrap();
    cache
        .fetch_asset(&mut source, "synthetic.one", &name, 1024)
        .unwrap();
    drop(cache);
    let connection = rusqlite::Connection::open(&path).unwrap();
    let mut damaged = bytes;
    damaged[0] ^= 1;
    connection
        .execute("UPDATE assets SET data=?1", [damaged])
        .unwrap();
    drop(connection);
    let cache = Replica::open(&path).unwrap();
    assert!(
        matches!(cache.cached_asset(&name, 1024), Err(Error::Io(error)) if error.kind() == io::ErrorKind::InvalidData)
    );
    assert!(
        matches!(cache.fetch_asset(&mut source, "synthetic.one", &name, 1024), Err(Error::Io(error)) if error.kind() == io::ErrorKind::InvalidData)
    );
    cache
        .export_recovery(directory.path().join("damaged.sqlite"))
        .unwrap();
    let archive = Recovery::open(directory.path().join("damaged.sqlite")).unwrap();
    assert!(archive.cached_asset(&name, 1024).is_err());
    assert_eq!(archive.pending().unwrap().len(), 2);
}

#[test]
fn abrupt_process_exit_retains_only_completed_downloads_and_archives() {
    const CHILD: &str = "ONESTORE_ASSET_EXIT_CASE";
    if let Ok(phase) = std::env::var(CHILD) {
        struct ExitDuringRead;
        impl notebook::discover::Source for ExitDuringRead {
            fn entries(&mut self, _: &str, _: usize) -> io::Result<Vec<notebook::discover::Entry>> {
                panic!("Unexpected enumeration")
            }
            fn read(&mut self, _: &str, _: usize) -> io::Result<Vec<u8>> {
                std::process::exit(83)
            }
        }
        let root = std::env::var("ONESTORE_ASSET_EXIT_ROOT").unwrap();
        let cache = copied_cache(Path::new(&root));
        let (name, bytes) = payload(1024);
        if phase == "download" {
            cache
                .fetch_asset(&mut ExitDuringRead, "synthetic.one", &name, bytes.len())
                .unwrap();
            panic!("Read returned after process exit");
        }
        cache
            .fetch_asset(
                &mut notebook::discover::Local::open(FIXTURE).unwrap(),
                "synthetic.one",
                &name,
                bytes.len(),
            )
            .unwrap();
        if phase == "archive" {
            cache
                .export_recovery(Path::new(&root).join("recovery.sqlite"))
                .unwrap();
        }
        std::process::exit(83);
    }
    for phase in ["download", "cached", "archive"] {
        let root = tempfile::tempdir().unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "abrupt_process_exit_retains_only_completed_downloads_and_archives",
            ])
            .env(CHILD, phase)
            .env("ONESTORE_ASSET_EXIT_ROOT", root.path())
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(83),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let cache = Replica::open(root.path().join("cache.sqlite")).unwrap();
        let (name, bytes) = payload(1024);
        let expected = (phase != "download").then_some(bytes);
        assert_eq!(cache.cached_asset(&name, 1024).unwrap(), expected);
        assert_eq!(cache.pending().unwrap().len(), 2);
        assert!(matches!(
            cache.status(1).unwrap(),
            Some(EditStatus::AwaitingConfirmation { .. })
        ));
        if phase == "archive" {
            let recovery = Recovery::open(root.path().join("recovery.sqlite")).unwrap();
            assert_eq!(recovery.cached_asset(&name, 1024).unwrap(), expected);
            assert_eq!(recovery.pending().unwrap(), cache.pending().unwrap());
            assert_eq!(recovery.status(1).unwrap(), cache.status(1).unwrap());
        }
    }
}

#[test]
#[cfg(feature = "smb")]
#[ignore = "requires a disposable Samba mirror at ONESTORE_SMB_NOTEBOOK"]
fn live_smb_downloads_survive_disconnect_and_cache_reopen() {
    let root = tempfile::tempdir().unwrap();
    let cache = copied_cache(root.path());
    let client = notebook::smb::Client::connect(
        &std::env::var("ONESTORE_SMB_LAB").unwrap(),
        "agent",
        notebook::smb::Credentials::default(),
        Duration::from_secs(5),
    )
    .unwrap();
    let notebook = std::env::var("ONESTORE_SMB_NOTEBOOK").unwrap();
    let mut source = notebook::discover::Smb::new(&client, &notebook).unwrap();
    for size in [0, 771, 1024] {
        let (name, bytes) = payload(size);
        assert_eq!(
            cache
                .fetch_asset(&mut source, "synthetic.one", &name, size)
                .unwrap(),
            bytes
        );
    }
    drop(client);
    cache
        .export_recovery(root.path().join("recovery.sqlite"))
        .unwrap();
    drop(cache);
    let cache = Replica::open(root.path().join("cache.sqlite")).unwrap();
    let recovery = Recovery::open(root.path().join("recovery.sqlite")).unwrap();
    for size in [0, 771, 1024] {
        let (name, bytes) = payload(size);
        assert_eq!(
            cache.cached_asset(&name, size).unwrap(),
            Some(bytes.clone())
        );
        assert_eq!(recovery.cached_asset(&name, size).unwrap(), Some(bytes));
    }
    assert_eq!(cache.pending().unwrap(), recovery.pending().unwrap());
    assert_eq!(cache.status(1).unwrap(), recovery.status(1).unwrap());
    assert_eq!(cache.recovery_summary().unwrap().cached_assets, 3);
}
