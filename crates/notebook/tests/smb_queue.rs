//! The op queue against a disposable Samba lab (`ONESTORE_SMB_LAB=127.0.0.1:PORT`, share
//! `agent`): publications read only the file's header, a native change is read once and
//! merged, and a session edits through the embedded client.
#![cfg(feature = "smb")]

use notebook::{
    EditStatus, Remote, Replica, SmbRemote,
    session::{Event, Section},
    smb::{Client, Credentials},
};
use onestore::{
    CommitError, ExGuid, Stamp, Transaction,
    op::{Edit, Op, PageOp},
};
use std::{
    io,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

fn client() -> Client {
    Client::connect(
        &std::env::var("ONESTORE_SMB_LAB").unwrap(),
        "agent",
        Credentials::default(),
        Duration::from_secs(5),
    )
    .unwrap()
}

fn unique(name: &str) -> String {
    format!(
        "{name}-{}.one",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

/// The remote, counting whole-file reads.
struct Counted {
    remote: SmbRemote,
    reads: usize,
    stamps: usize,
}

impl Remote for Counted {
    fn read(&mut self) -> io::Result<Vec<u8>> {
        self.reads += 1;
        self.remote.read()
    }
    fn stamp(&mut self) -> io::Result<Option<Stamp>> {
        self.stamps += 1;
        self.remote.stamp()
    }
    fn publish(&mut self, transaction: &Transaction) -> Result<(), CommitError> {
        self.remote.publish(transaction)
    }
    fn confirm(&mut self, snapshot: &[u8]) -> Result<(), CommitError> {
        self.remote.confirm(snapshot)
    }
}

/// The first page's space and first body text.
fn target(replica: &Replica) -> (ExGuid, ExGuid) {
    let space = replica.pages().unwrap()[0].0;
    let text = replica
        .page(space)
        .unwrap()
        .objects
        .iter()
        .find_map(|object| match object {
            onestore::page::PageObject::Outline(outline) => outline
                .paragraphs
                .iter()
                .find_map(|p| p.text().map(|t| t.id)),
            _ => None,
        })
        .unwrap();
    (space, text)
}

fn typed(space: ExGuid, text: ExGuid, at: u32, with: &str) -> Edit {
    Edit {
        at: 133_000_000_000_000_000,
        ops: vec![Op::Page {
            space,
            op: PageOp::Text {
                text,
                range: at..at,
                with: with.into(),
            },
        }],
    }
}

fn text_of(replica: &Replica, space: ExGuid, text: ExGuid) -> String {
    let page = replica.page(space).unwrap();
    page.objects
        .iter()
        .find_map(|object| match object {
            onestore::page::PageObject::Outline(outline) => outline
                .paragraphs
                .iter()
                .find_map(|p| p.text().filter(|t| t.id == text)),
            _ => None,
        })
        .unwrap()
        .text
        .text()
        .to_owned()
}

#[test]
#[ignore = "requires ONESTORE_SMB_LAB pointing to disposable Samba"]
fn live_keystrokes_publish_reading_only_the_header_and_a_native_change_merges() {
    let lab = client();
    let path = unique("queue");
    let source = onestore::create_section(&path, "Body", "Author").unwrap();
    lab.create(&path, &source).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let replica = Replica::create(directory.path().join("cache.sqlite"), &source).unwrap();
    let (space, text) = target(&replica);
    let mut remote = Counted {
        remote: SmbRemote::new(client(), path.clone(), 1 << 24),
        reads: 0,
        stamps: 0,
    };
    for n in 0..20 {
        let at = 4 + n;
        let id = replica
            .apply("Author", typed(space, text, at, "x"))
            .unwrap();
        assert!(matches!(
            replica.sync_once(&mut remote).unwrap().edit,
            Some((published, EditStatus::Published { .. })) if published == id
        ));
    }
    assert_eq!(remote.reads, 0, "publications read only the header");
    assert_eq!(remote.stamps, 20);
    assert_eq!(
        lab.read(&path, 1 << 24).unwrap(),
        replica.snapshot().unwrap()
    );
    for _ in 0..5 {
        assert_eq!(replica.sync_once(&mut remote).unwrap().edit, None);
    }
    assert_eq!(remote.reads, 0, "idle polls read only the header");

    // Another writer types at the start of the same text; a local edit at its end merges.
    let native = {
        let arena = onestore::Arena::default();
        let mut section =
            onestore::Section::open(&arena, lab.read(&path, 1 << 24).unwrap()).unwrap();
        section
            .apply("Native", &typed(space, text, 0, "Native "))
            .unwrap();
        section.seal().unwrap().unwrap()
    };
    lab.commit_transaction(&path, &native).unwrap();
    let end = text_of(&replica, space, text).encode_utf16().count() as u32;
    let id = replica
        .apply("Author", typed(space, text, end, " local"))
        .unwrap();
    let synced = replica.sync_once(&mut remote).unwrap();
    assert_eq!(synced.changed, [space]);
    assert!(
        matches!(synced.edit, Some((published, EditStatus::Published { .. })) if published == id)
    );
    assert_eq!(remote.reads, 1, "a changed remote is read once");
    let merged = text_of(&replica, space, text);
    assert!(merged.starts_with("Native Body"), "{merged}");
    assert!(merged.ends_with(" local"), "{merged}");
    assert_eq!(
        lab.read(&path, 1 << 24).unwrap(),
        replica.snapshot().unwrap()
    );
    lab.delete(&path).unwrap();
}

#[test]
#[ignore = "requires ONESTORE_SMB_LAB pointing to disposable Samba"]
fn live_session_applies_edits_through_the_embedded_client_and_reopens() {
    let lab = client();
    let path = unique("session");
    let source = onestore::create_section(&path, "Session", "Author").unwrap();
    lab.create(&path, &source).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let cache = directory.path().join("cache.sqlite");
    let section = Section::resume_smb(
        path.clone(),
        Replica::create(&cache, &source).unwrap(),
        1 << 24,
        || {
            Client::connect(
                &std::env::var("ONESTORE_SMB_LAB").unwrap(),
                "agent",
                Credentials::default(),
                Duration::from_secs(5),
            )
        },
        || {},
    )
    .unwrap();
    let (space, text) = target(section.replica());
    for n in 0..10 {
        section
            .apply("Author", typed(space, text, n, &n.to_string()))
            .unwrap();
    }
    // Applying does not wait: the edits are done when the file holds them.
    let remote_text = |bytes: Vec<u8>| {
        let arena = onestore::Arena::default();
        let section = onestore::Section::open(&arena, bytes).unwrap();
        let page = section.page(space).unwrap();
        page.objects
            .iter()
            .find_map(|object| match object {
                onestore::page::PageObject::Outline(outline) => {
                    outline.paragraphs.iter().find_map(|p| {
                        p.text()
                            .filter(|t| t.id == text)
                            .map(|t| t.text.text().to_owned())
                    })
                }
                _ => None,
            })
            .unwrap()
    };
    let deadline = Instant::now() + Duration::from_secs(30);
    while remote_text(lab.read(&path, 1 << 24).unwrap()) != "0123456789Session"
        || !section.pending().unwrap().is_empty()
    {
        assert!(Instant::now() < deadline, "the edits were not published");
        for event in section.events() {
            assert!(
                !matches!(event, Event::Rejected { .. } | Event::Failed(_)),
                "{event:?}"
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let published = lab.read(&path, 1 << 24).unwrap();
    section.close().unwrap();
    let replica = Replica::open(&cache).unwrap();
    assert_eq!(text_of(&replica, space, text), "0123456789Session");
    assert_eq!(replica.snapshot().unwrap(), published);
    lab.delete(&path).unwrap();
}
