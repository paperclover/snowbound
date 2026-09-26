use super::*;
use onestore::{
    ExGuid, RevisionIndex, Store, Transaction,
    document::{Document, Kind},
    op::{Edit, Op, PageOp},
};
use std::{
    fs,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

mod faults;

#[test]
#[ignore = "requires an owned Samba directory and ONESTORE_SMB_DIRECTORY_ORACLE from its local filesystem"]
fn live_directory() {
    let client = client();
    let bytes = fs::read(std::env::var("ONESTORE_SMB_DIRECTORY_ORACLE").unwrap()).unwrap();
    let oracle: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let root = oracle["path"].as_str().unwrap();
    let expected = oracle["entries"].as_array().unwrap();
    let entries = client.read_dir(root, expected.len()).unwrap();
    assert_eq!(entries.len(), expected.len());
    for expected in expected {
        let entry = entries
            .iter()
            .find(|entry| entry.name == expected["name"])
            .unwrap();
        let directory = expected["directory"].as_bool().unwrap();
        assert_eq!(entry.attributes & 0x10 != 0, directory, "{}", entry.name);
        if !directory {
            assert_eq!(
                entry.size,
                expected["size"].as_u64().unwrap(),
                "{}",
                entry.name
            );
        }
    }
    assert_eq!(
        client.read_dir(root, entries.len() - 1).unwrap_err().kind(),
        io::ErrorKind::FileTooLarge
    );
    assert_eq!(client.read_dir(root, entries.len()).unwrap(), entries);
    assert!(
        client
            .read_dir(&format!("{root}/empty"), 0)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        client
            .read_dir(&format!("{root}/missing"), 1)
            .unwrap_err()
            .kind(),
        io::ErrorKind::NotFound
    );
    assert_eq!(
        client
            .read_dir(&format!("{root}/file.one"), 1)
            .unwrap_err()
            .kind(),
        io::ErrorKind::NotADirectory
    );
    assert_eq!(
        client
            .read_dir(&format!("{root}/denied"), 1)
            .unwrap_err()
            .kind(),
        io::ErrorKind::PermissionDenied
    );
    assert!(
        client
            .read_dir("", 10_000)
            .unwrap()
            .iter()
            .any(|entry| entry.name == root)
    );
}

#[test]
#[ignore = "requires an owned Samba directory through a proxy that interrupts a later directory page or close"]
fn live_directory_interruption() {
    let client = client();
    let root = std::env::var("ONESTORE_SMB_DIRECTORY").unwrap();
    let started = Instant::now();
    assert!(client.read_dir(&root, 10_000).is_err());
    assert!(started.elapsed() < Duration::from_secs(10));
    assert_eq!(
        client.read_dir(&root, 10_000).unwrap_err().kind(),
        io::ErrorKind::NotConnected
    );
}

fn client() -> Client {
    Client::connect(
        &std::env::var("ONESTORE_SMB_LAB").unwrap(),
        "agent",
        Credentials::default(),
        Duration::from_secs(5),
    )
    .unwrap()
}
fn create(client: &Client, path: &str, bytes: &[u8]) {
    let response: CreateResponse = client
        .request(
            Command::Create,
            CreateRequest {
                requested_oplock_level: OplockLevel::None,
                impersonation_level: ImpersonationLevel::Impersonation,
                desired_access: FileAccessMask::new(0xc0000000),
                file_attributes: 0,
                share_access: ShareAccess(7),
                create_disposition: CreateDisposition::FileCreate,
                create_options: 0x42,
                name: path.to_owned(),
                create_contexts: Vec::new(),
            },
        )
        .unwrap();
    let mut file = File::new(client, &response);
    let mut offset = 0;
    while offset < bytes.len() {
        offset += file.write_at(offset as u64, &bytes[offset..]).unwrap();
    }
    file.flush().unwrap();
    file.close().unwrap();
}
/// The transaction replacing `range` of the first text in `image` with `with`, as typed now.
fn replaced(image: &[u8], range: std::ops::Range<u32>, with: &str) -> Transaction {
    let (space, text, _) = text(image);
    let arena = onestore::Arena::default();
    let mut section = onestore::Section::open(&arena, image.to_vec()).unwrap();
    let at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64
        / 100
        + 116_444_736_000_000_000;
    section
        .apply(
            "Fixture",
            &Edit {
                at,
                ops: vec![Op::Page {
                    space,
                    op: PageOp::Text {
                        text,
                        range,
                        with: with.to_owned(),
                    },
                }],
            },
        )
        .unwrap();
    section.seal().unwrap().unwrap()
}

/// `image` after `transaction`.
fn applied(image: &[u8], transaction: &Transaction) -> Vec<u8> {
    let mut image = image.to_vec();
    transaction.apply(&mut image).unwrap();
    image
}

fn text(bytes: &[u8]) -> (ExGuid, ExGuid, String) {
    let store = Store::parse(bytes).unwrap();
    assert!(store.checksum_mismatches.is_empty());
    let index = RevisionIndex::parse(&store).unwrap();
    index.validate_current().unwrap();
    let doc = Document::parse(&index).unwrap();
    doc.spaces
        .iter()
        .find_map(|(sid, space)| {
            let revision = space.active().unwrap();
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
#[ignore = "requires disposable Samba and an existing ONESTORE_SMB_EVIDENCE directory"]
fn live_coordination() {
    let output = std::env::var("ONESTORE_SMB_EVIDENCE").unwrap();
    let writer = client();
    let path = format!(
        "adapter-{}.one",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let source = onestore::create_section(&path, "Before café 🦀", "Fixture").unwrap();
    create(&writer, &path, &source);
    assert_eq!(writer.read(&path, 1 << 20).unwrap(), source);
    let readers: Vec<_> = (0..12).map(|_| client()).collect();
    let mut held: Vec<_> = readers
        .iter()
        .map(|client| {
            client
                .open(&path, false)
                .unwrap()
                .coordinate(&path, false, &[])
                .unwrap()
        })
        .collect();
    let (_, _, before) = text(&source);
    let replacement = "After café 🦀";
    let everything = 0..before.encode_utf16().count() as u32;
    writer
        .commit_transaction(&path, &replaced(&source, everything.clone(), replacement))
        .unwrap();
    let after = writer.read(&path, 1 << 20).unwrap();
    assert_eq!(text(&after).2, replacement);
    for file in &mut held {
        let snapshot = onestore::read_snapshot(|offset, out| file.read_at(offset, out), 1 << 20)
            .unwrap()
            .unwrap();
        assert_eq!(snapshot, after);
    }
    let error = writer
        .commit_transaction(&path, &replaced(&source, 0..1, "X"))
        .unwrap_err();
    assert_eq!(error.state, CommitState::NotCommitted);
    assert_eq!(error.error.kind(), io::ErrorKind::ResourceBusy);
    assert_eq!(writer.read(&path, 1 << 20).unwrap(), after);
    drop(held);

    let stale = writer.open(&path, true).unwrap();
    let maintenance = client();
    let guard = maintenance.open(&path, false).unwrap();
    let _: LockResponse = maintenance
        .request(
            Command::Lock,
            LockRequest {
                file_id: guard.id.unwrap(),
                lock_sequence: 0,
                locks: vec![
                    LockElement {
                        offset: 0xfffffffc,
                        length: 1,
                        flags: 0x12,
                    },
                    LockElement {
                        offset: 0xffffeffc,
                        length: 4096,
                        flags: 0x12,
                    },
                ],
            },
        )
        .unwrap();
    let replacement_path = format!("{path}.replacement");
    create(&maintenance, &replacement_path, &source);
    let mut connection = maintenance
        .connection
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .clone();
    maintenance
        .runtime
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .block_on(
            maintenance
                .tree
                .rename(&mut connection, &path, &format!("{path}.old")),
        )
        .unwrap();
    maintenance
        .runtime
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .block_on(
            maintenance
                .tree
                .rename(&mut connection, &replacement_path, &path),
        )
        .unwrap();
    drop(connection);
    guard.close().unwrap();
    let error = stale.coordinate(&path, true, &[]).err().unwrap();
    assert_eq!(error.kind(), io::ErrorKind::ResourceBusy);
    assert_eq!(writer.read(&path, 1 << 20).unwrap(), source);

    let retiring = client();
    let file = retiring
        .open(&path, true)
        .unwrap()
        .coordinate(&path, true, &[])
        .unwrap();
    retiring.retire();
    assert_eq!(
        retiring.read(&path, 1 << 20).unwrap_err().kind(),
        io::ErrorKind::NotConnected
    );
    drop(file);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match writer
            .open(&path, true)
            .and_then(|file| file.coordinate(&path, true, &[]))
        {
            Ok(file) => {
                file.close().unwrap();
                break;
            }
            Err(error)
                if error.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(10))
            }
            Err(error) => panic!("retired connection retained locks: {error}"),
        }
    }

    let mut unfinished = writer
        .open(&path, true)
        .unwrap()
        .coordinate(&path, true, &[])
        .unwrap();
    assert_eq!(
        unfinished
            .write_at(source.len() as u64, b"unpublished")
            .unwrap(),
        11
    );
    unfinished.flush().unwrap();
    unfinished.close().unwrap();
    let snapshot = writer.read(&path, 1 << 20).unwrap();
    assert_eq!(snapshot.len(), source.len() + 11);
    assert_eq!(text(&snapshot).2, before);
    writer
        .commit_transaction(&path, &replaced(&snapshot, everything, "Recovered café 🦀"))
        .unwrap();
    let recovered = writer.read(&path, 1 << 20).unwrap();
    assert_eq!(text(&recovered).2, "Recovered café 🦀");
    let spare = client();
    tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(async {
            drop(spare);
        });
    fs::write(std::path::Path::new(&output).join("source.one"), &source).unwrap();
    fs::write(std::path::Path::new(&output).join("committed.one"), &after).unwrap();
    fs::write(
        std::path::Path::new(&output).join("recovered.one"),
        &recovered,
    )
    .unwrap();
    println!(
        "{}",
        serde_json::json!({"path":path,"readers":12,"committed_while_readers_held":true,"fresh_reads":12,"stale_snapshot_rejected":true,"replaced_handle_rejected":true,"retirement_releases_locks":true,"unpublished_tail_recovered":true,"drop_inside_runtime":true})
    );
}

/// Polling reads only the header, even while maintenance excludes snapshot readers, and a
/// transaction commits against the header it was built on.
#[test]
#[ignore = "requires ONESTORE_SMB_LAB pointing to disposable Samba"]
fn live_stamp_and_transaction() {
    let poller = client();
    let source = onestore::create_section("stamp.one", "Stamped", "Author").unwrap();
    let path = format!(
        "stamp-{}.one",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    create(&poller, &path, &source);
    assert_eq!(
        poller.stamp(&path).unwrap(),
        onestore::Stamp::of(&source).unwrap()
    );
    let maintenance = client();
    let guard = maintenance.open(&path, false).unwrap();
    guard.lock(0xfffffffb, 0x12).unwrap();
    assert_eq!(
        poller.read(&path, 1 << 20).unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
    assert_eq!(
        poller.stamp(&path).unwrap(),
        onestore::Stamp::of(&source).unwrap()
    );
    guard.close().unwrap();
    let transaction = replaced(&source, 0..0, "Changed ");
    let edited = applied(&source, &transaction);
    poller.commit_transaction(&path, &transaction).unwrap();
    assert_eq!(
        poller.stamp(&path).unwrap(),
        onestore::Stamp::of(&edited).unwrap()
    );
    assert_eq!(poller.read(&path, 1 << 20).unwrap(), edited);
    let stale = poller.commit_transaction(&path, &transaction).unwrap_err();
    assert_eq!(stale.state, CommitState::NotCommitted);
    assert_eq!(stale.error.kind(), io::ErrorKind::ResourceBusy);
    assert_eq!(poller.read(&path, 1 << 20).unwrap(), edited);
    let base = onestore::Stamp::of(&edited).unwrap();
    poller.confirm(&path, &base).unwrap();
    let confirmed = poller.read(&path, 1 << 20).unwrap();
    assert_eq!(confirmed[..212], edited[..212]);
    assert_ne!(confirmed[212..252], edited[212..252]);
    assert_eq!(confirmed[252..], edited[252..]);
    let stale = poller.confirm(&path, &base).unwrap_err();
    assert_eq!(stale.state, CommitState::NotCommitted);
    assert_eq!(stale.error.kind(), io::ErrorKind::ResourceBusy);
    poller.delete(&path).unwrap();
}

#[test]
#[ignore = "requires ONESTORE_SMB_LAB pointing to disposable Samba"]
fn live_storage_inspection() {
    let reader = client();
    for (index, fixture) in [
        "native-encrypted/encrypted-01/notebook/synthetic.one",
        "native-encrypted/cold-encrypted-02/notebook/Open Notebook.one",
    ]
    .into_iter()
    .enumerate()
    {
        let source = fs::read(format!("../../corpus/{fixture}")).unwrap();
        let path = format!(
            "inspection-{index}-{}.one",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        create(&reader, &path, &source);
        assert_eq!(reader.read_storage(&path, source.len()).unwrap(), source);
        assert_eq!(
            reader.read(&path, source.len()).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        let maintenance = client();
        let guard = maintenance.open(&path, false).unwrap();
        guard.lock(0xfffffffb, 0x12).unwrap();
        assert_eq!(
            reader.read_storage(&path, source.len()).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        guard.close().unwrap();
        assert_eq!(reader.read_storage(&path, source.len()).unwrap(), source);
    }
}

#[test]
#[ignore = "requires ONESTORE_SMB_LAB pointing to disposable Samba"]
fn live_assets() {
    let reader = client();
    let writer = client();
    for size in [0, 1, 65535, 65536, 65537, 1048577] {
        let bytes: Vec<_> = (0..size).map(|index| (index % 251) as u8).collect();
        let path = format!(
            "asset-{size}-{}.onebin",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        create(&writer, &path, &bytes);
        assert_eq!(reader.read_asset(&path, size).unwrap(), bytes);
        if size != 0 {
            assert_eq!(
                reader.read_asset(&path, size - 1).unwrap_err().kind(),
                io::ErrorKind::FileTooLarge
            );
            assert_eq!(reader.read_asset(&path, size).unwrap(), bytes);
        }
        let writing = writer.open(&path, true).unwrap();
        assert_eq!(
            reader.read_asset(&path, size).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        writing.close().unwrap();
        let asset = reader.open_shared(&path, false, 1).unwrap();
        assert_eq!(
            writer.open(&path, true).err().unwrap().kind(),
            io::ErrorKind::WouldBlock
        );
        let other = writer.open_shared(&path, false, 1).unwrap();
        other.close().unwrap();
        asset.close().unwrap();
        writer.open(&path, true).unwrap().close().unwrap();
        assert_eq!(reader.read_asset(&path, size).unwrap(), bytes);
    }
    assert_eq!(
        reader
            .read_asset("absent-payload.onebin", 0)
            .unwrap_err()
            .kind(),
        io::ErrorKind::NotFound
    );
}

#[test]
#[ignore = "requires an owned Samba fixture and maintenance controller"]
fn live_reader_hold() {
    let client = client();
    let path = std::env::var("ONESTORE_SMB_PATH").unwrap();
    let output = std::path::PathBuf::from(std::env::var("ONESTORE_SMB_HOLD").unwrap());
    assert!(!output.join("ready").exists() && !output.join("release").exists());
    let mut file = client
        .open(&path, false)
        .unwrap()
        .coordinate(&path, false, &[])
        .unwrap();
    fs::write(output.join("ready"), b"held").unwrap();
    let deadline = Instant::now() + Duration::from_secs(300);
    let mut accepted = 0;
    let mut retries = 0;
    while !output.join("release").exists() {
        assert!(
            Instant::now() < deadline,
            "maintenance controller timed out"
        );
        match onestore::read_snapshot(|offset, out| file.read_at(offset, out), 256 << 20).unwrap() {
            Some(_) => accepted += 1,
            None => retries += 1,
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    file.close().unwrap();
    assert!(accepted > 0);
    fs::write(
        output.join("released.json"),
        serde_json::to_vec(&serde_json::json!({"accepted": accepted, "retries": retries})).unwrap(),
    )
    .unwrap();
}

#[test]
fn read_limits_respect_negotiation_and_available_credits() {
    for dialect in Dialect::ALL {
        for large_mtu in [false, true] {
            for max_read_size in [65536, 65537, 131072, 1048576, u32::MAX] {
                let params = NegotiatedParams {
                    dialect: *dialect,
                    max_read_size,
                    max_write_size: 65536,
                    max_transact_size: 65536,
                    server_guid: Default::default(),
                    signing_required: false,
                    capabilities: Capabilities(if large_mtu {
                        Capabilities::LARGE_MTU
                    } else {
                        0
                    }),
                    gmac_negotiated: false,
                    cipher: None,
                    compression_supported: false,
                };
                for credits in [0, 1, 2, 3, 15, 16, 17, u16::MAX] {
                    for requested in [1, 1024, 65535, 65536, 65537, 131072, 1048576, usize::MAX] {
                        let size = read_size(&params, credits, requested);
                        assert!(size > 0 && size <= requested && size <= max_read_size as usize);
                        assert!(size <= 1048576);
                        assert!(size.div_ceil(65536) <= usize::from(credits.max(1)));
                        if *dialect == Dialect::Smb2_0_2 || !large_mtu {
                            assert!(size <= 65536);
                        } else if credits >= 16 && max_read_size >= 1048576 && requested >= 1048576
                        {
                            assert_eq!(size, 1048576);
                        }
                    }
                }
            }
        }
    }
}

/// Notebook structure over the share: sections and groups created, renamed, coloured,
/// ordered and deleted through `Notebook::open_smb`, seen again by a fresh discovery.
#[test]
#[ignore = "requires an owned Samba share at ONESTORE_SMB_LAB"]
fn live_structure() {
    use crate::session::Notebook;
    let client = std::sync::Arc::new(client());
    let root = format!("structure-{}", std::process::id());
    client.create_directory(&root).unwrap();
    let first = onestore::create_section("First.one", "First page", "Author").unwrap();
    client.create(&format!("{root}/First.one"), &first).unwrap();
    let first_id = onestore::Store::parse(&first).unwrap().header.file_id;
    let toc =
        onestore::create_table_of_contents("Open Notebook.onetoc2", &[("First.one", first_id)])
            .unwrap();
    client
        .create(&format!("{root}/Open Notebook.onetoc2"), &toc)
        .unwrap();
    let toc_id = onestore::Store::parse(&toc).unwrap().header.file_id;
    client
        .place(&format!("{root}/First.one"), toc_id, "First.one")
        .unwrap();
    let cache = tempfile::tempdir().unwrap();
    let mut notebook =
        Notebook::open_smb(std::sync::Arc::clone(&client), &root, cache.path()).unwrap();
    assert_eq!(
        notebook.create_section("", "Second", &onestore::PageCreation::new(None, Some(""), "Author").unwrap()).unwrap(),
        "Second.one"
    );
    assert_eq!(notebook.create_group("", "Archive").unwrap(), "Archive");
    assert_eq!(
        notebook
            .create_section("Archive", "Inner", &onestore::PageCreation::new(None, Some(""), "Author").unwrap())
            .unwrap(),
        "Archive/Inner.one"
    );
    assert_eq!(
        notebook.rename("Second.one", "Renamed").unwrap(),
        "Renamed.one"
    );
    assert_eq!(notebook.rename("Archive", "Kept").unwrap(), "Kept");
    notebook
        .set_section_color("Renamed.one", Some(0x5ed7ff))
        .unwrap();
    notebook.reorder("", &["Kept", "Renamed.one"]).unwrap();
    notebook.delete("First.one").unwrap();
    let names = |folder: &crate::discover::Folder| {
        (
            folder
                .sections
                .iter()
                .map(|s| s.path.clone())
                .collect::<Vec<_>>(),
            folder
                .groups
                .iter()
                .map(|g| g.path.clone())
                .collect::<Vec<_>>(),
        )
    };
    let fresh = Notebook::open_smb(std::sync::Arc::clone(&client), &root, cache.path()).unwrap();
    assert_eq!(
        names(fresh.catalog()),
        (
            vec!["Renamed.one".to_owned()],
            vec!["Kept".to_owned(), "OneNote_RecycleBin".to_owned()]
        )
    );
    assert_eq!(
        names(&fresh.catalog().groups[0]),
        (vec!["Kept/Inner.one".to_owned()], vec![])
    );
    assert_eq!(
        names(&fresh.catalog().groups[1]),
        (vec!["OneNote_RecycleBin/First.one".to_owned()], vec![])
    );
    assert!(fresh.catalog().toc.as_ref().unwrap().unresolved.is_empty());
    let renamed = client
        .read_storage(&format!("{root}/Renamed.one"), 1 << 20)
        .unwrap();
    let header = onestore::Store::parse(&renamed).unwrap().header;
    assert_eq!(header.ancestor, toc_id);
    assert_eq!(header.name_crc, 0x6108912a);
    assert!(fresh.section("Renamed.one", || {}).is_err());
    for path in [
        "Renamed.one",
        "Kept/Inner.one",
        "Kept/Open Notebook.onetoc2",
        "Kept",
        "OneNote_RecycleBin/First.one",
        "OneNote_RecycleBin/Open Notebook.onetoc2",
        "OneNote_RecycleBin",
        "Open Notebook.onetoc2",
    ] {
        client.delete(&format!("{root}/{path}")).unwrap();
    }
    client.delete(&root).unwrap();
}
