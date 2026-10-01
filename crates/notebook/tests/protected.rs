//! Password-protected sections: unlocking, sealed queues, publishing, merging and conflict
//! pages, and setting, changing and removing a password.

use notebook::{EditStatus, Replica, discover::SectionState, session::Notebook};
use onestore::{
    Arena, ExGuid, Section,
    op::{Edit, Op, PageOp},
    page::{Page, PageObject},
    protected::{Error, Key, rekey},
};
use std::path::Path;

#[path = "support/server.rs"]
mod server;
use server::Server;

fn fixture(name: &str) -> (std::path::PathBuf, String) {
    let root = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus")).join(name);
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("manifest.json")).unwrap()).unwrap();
    (root, manifest["password"].as_str().unwrap().to_owned())
}

/// A copy of a notebook folder.
fn copy(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), to.join(entry.file_name())).unwrap();
    }
}

/// A protected one-page section holding `text`, with its key.
fn protected(text: &str) -> (Vec<u8>, Key) {
    let plain = onestore::create_section("sealed.one", text, "Fixture").unwrap();
    let key = Key::new("fixture password").unwrap();
    (rekey(&plain, None, Some(&key)).unwrap(), key)
}

/// The first text object of `image` with its page.
fn first_text(image: &[u8], key: &Key) -> (ExGuid, ExGuid, String) {
    let arena = Arena::default();
    let mut section = Section::unlock(&arena, image.to_vec(), key).unwrap();
    let (space, ..) = section.pages().unwrap()[0];
    let page = section.page(space).unwrap();
    texts(&page)
        .into_iter()
        .next()
        .map(|(id, text)| (space, id, text))
        .unwrap()
}

fn texts(page: &Page) -> Vec<(ExGuid, String)> {
    page.objects
        .iter()
        .filter_map(|object| match object {
            PageObject::Outline(outline) => Some(&outline.paragraphs),
            _ => None,
        })
        .flatten()
        .filter_map(|paragraph| {
            let text = paragraph.text()?;
            Some((text.id, text.text.text().to_owned()))
        })
        .collect()
}

fn replace(space: ExGuid, text: ExGuid, range: std::ops::Range<u32>, with: &str) -> Edit {
    Edit {
        at: 134_000_000_000_000_000,
        ops: vec![Op::Page {
            space,
            op: PageOp::Text {
                text,
                range,
                with: with.into(),
            },
        }],
    }
}

/// Whether any file of a replica holds `clear`, as UTF-8 or UTF-16.
fn leaks(replica: &Path, clear: &str) -> bool {
    let utf16: Vec<u8> = clear.encode_utf16().flat_map(u16::to_le_bytes).collect();
    ["", "-wal", "-shm"].iter().any(|suffix| {
        let mut file = replica.as_os_str().to_owned();
        file.push(suffix);
        std::fs::read(file).is_ok_and(|bytes| {
            [clear.as_bytes(), &utf16[..]]
                .iter()
                .any(|clear| bytes.windows(clear.len()).any(|w| w == *clear))
        })
    })
}

/// A locked section is discovered as such, unlocks with its password and opens as a session.
#[test]
fn a_locked_section_unlocks_into_a_session() {
    let (root, password) = fixture("native-protected-boundaries");
    let temporary = tempfile::tempdir().unwrap();
    let folder = temporary.path().join("notebook");
    copy(&root.join("notebook"), &folder);
    let notebook = Notebook::open(&folder, temporary.path().join("cache")).unwrap();
    let section = &notebook.catalog().sections[0];
    assert!(matches!(section.state, SectionState::Locked));
    assert!(matches!(
        notebook.unlock(&section.path, "wrong"),
        Err(notebook::Error::Protected(Error::PasswordMismatch))
    ));
    assert!(notebook.section(&section.path, || {}).is_err());
    let key = notebook.unlock(&section.path, &password).unwrap();
    let session = notebook
        .section_unlocked(&section.path, &key, || {})
        .unwrap();
    let pages = session.pages().unwrap();
    assert_eq!(pages.len(), 11);
    assert!(pages.iter().all(|(_, title, _)| !title.is_empty()));
    let title = &pages[0].1;
    session.close().unwrap();
    // The replica holds the section as its file does: encrypted.
    assert!(!leaks(
        &notebook.replica_path(&section.path).unwrap(),
        title
    ));
}

/// Queued edits and their payloads are sealed in the replica, which opens again only under
/// the section's key, and publish into the file under it.
#[test]
fn queued_edits_are_sealed_and_publish_under_the_key() {
    const SECRET: &str = "Queued in confidence";
    let (source, key) = protected("Opening line");
    let (space, text, _) = first_text(&source, &key);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("cache.sqlite");
    let cache = Replica::open_or_create(&path, Some(&key), || Ok(source.clone())).unwrap();
    cache
        .apply("Fixture", replace(space, text, 0..0, SECRET))
        .unwrap();
    let mut page = cache.page(space).unwrap();
    let PageObject::Outline(outline) = page
        .objects
        .iter_mut()
        .find(|object| matches!(object, PageObject::Outline(_)))
        .unwrap()
    else {
        unreachable!()
    };
    let mut file = outline.paragraphs[0].clone();
    file.id = onestore::page::text::new_id().unwrap();
    file.content = onestore::page::ParagraphContent::Attachment(onestore::page::Attachment {
        id: onestore::page::text::new_id().unwrap(),
        filename: "sealed.txt".into(),
        source_path: None,
        size: Some([24.0, 24.0]),
        layout: Default::default(),
        bytes: Some(std::sync::Arc::from(SECRET.as_bytes())),
        preview: None,
        recording: None,
        tags: Vec::new(),
    });
    outline.paragraphs.push(file);
    let before = cache.page(space).unwrap();
    let ops = onestore::op::lower_page(&before, &page).unwrap();
    cache
        .apply(
            "Fixture",
            Edit {
                at: 134_000_000_000_000_001,
                ops: ops.into_iter().map(|op| Op::Page { space, op }).collect(),
            },
        )
        .unwrap();
    drop(cache);
    assert!(!leaks(&path, SECRET));
    assert!(Replica::open(&path).is_err());
    assert!(
        Replica::open_or_create(
            &path,
            Some(&Key::new("another").unwrap()),
            || unreachable!()
        )
        .is_err()
    );
    let cache = Replica::open_or_create(&path, Some(&key), || unreachable!()).unwrap();
    assert_eq!(cache.pending().unwrap().len(), 2);
    let mut server = Server::new(&source);
    assert!(matches!(
        cache.sync_once(&mut server).unwrap().edit,
        Some((_, EditStatus::Published { .. }))
    ));
    drop(cache);
    assert!(!leaks(&path, SECRET));
    assert!(
        !server
            .durable
            .windows(SECRET.len())
            .any(|w| w == SECRET.as_bytes())
    );
    let arena = Arena::default();
    let stored = Section::unlock(&arena, server.durable.clone(), &key).unwrap();
    let published = stored.page(space).unwrap();
    assert!(texts(&published)[0].1.starts_with(SECRET));
    assert!(format!("{published:?}").contains("sealed.txt"));
    let archived = directory.path().join("recovery.sqlite");
    Replica::open_or_create(&path, Some(&key), || unreachable!())
        .unwrap()
        .export_recovery(&archived)
        .unwrap();
    assert!(!leaks(&archived, SECRET));
    assert!(notebook::Recovery::open(&archived).is_err());
    assert!(notebook::Recovery::open_unlocked(&archived, &key).is_ok());
}

/// Edits of the same text on two copies of a protected section merge as an ordinary
/// section's do: the remote's stays the page, the local one becomes a conflict page, and
/// every revision stays under the key.
#[test]
fn a_conflict_in_a_protected_section_becomes_a_conflict_page() {
    let (source, key) = protected("alpha beta gamma");
    let (space, text, _) = first_text(&source, &key);
    let directory = tempfile::tempdir().unwrap();
    let cache = Replica::open_or_create(directory.path().join("c.sqlite"), Some(&key), || {
        Ok(source.clone())
    })
    .unwrap();
    cache
        .apply("Local", replace(space, text, 6..10, "LOCAL"))
        .unwrap();
    let remote = {
        let arena = Arena::default();
        let mut section = Section::unlock(&arena, source.clone(), &key).unwrap();
        section
            .apply("Remote", &replace(space, text, 6..10, "REMOTE"))
            .unwrap();
        section.seal().unwrap();
        section.image()
    };
    let mut server = Server::new(&remote);
    assert!(matches!(
        cache.sync_once(&mut server).unwrap().edit,
        Some((_, EditStatus::Published { .. }))
    ));
    let arena = Arena::default();
    let mut merged = Section::unlock(&arena, server.durable.clone(), &key).unwrap();
    assert_eq!(
        texts(&merged.page(space).unwrap())[0].1,
        "alpha REMOTE gamma"
    );
    let conflicts = merged.conflicts().unwrap();
    assert_eq!(conflicts.len(), 1);
    let conflict = merged.page(conflicts[0].1[0].space).unwrap();
    assert_eq!(texts(&conflict)[0].1, "alpha LOCAL gamma");
    let store = onestore::Store::parse(&server.durable).unwrap();
    assert!(
        onestore::RevisionIndex::parse(&store)
            .unwrap()
            .spaces
            .values()
            .flat_map(|space| space.revisions.values())
            .all(|revision| revision.encrypted)
    );
}

/// Setting a password rewrites the section under new identities that its TOC follows and
/// drops the plaintext replica; changing and removing it rewrite it again.
#[test]
fn a_password_is_set_changed_and_removed_in_a_notebook() {
    const TEXT: &str = "Kept through every password";
    let temporary = tempfile::tempdir().unwrap();
    let folder = temporary.path().join("notebook");
    let cache = temporary.path().join("cache");
    let creation = onestore::PageCreation::new(None, Some(TEXT), "Fixture").unwrap();
    let mut notebook = Notebook::create(&folder, &cache, 0x00f0_a0c0, &creation).unwrap();
    let path = notebook.catalog().sections[0].path.clone();
    let plain = notebook.replica_path(&path).unwrap();
    notebook.section(&path, || {}).unwrap().close().unwrap();
    assert!(plain.exists());
    let identity = notebook.catalog().sections[0].file_id;

    let first = notebook
        .set_password(&path, None, Some("first"))
        .unwrap()
        .unwrap();
    assert!(!plain.exists());
    let section = &notebook.catalog().sections[0];
    assert!(matches!(section.state, SectionState::Locked));
    assert_ne!(section.file_id, identity);
    assert!(
        notebook
            .catalog()
            .toc
            .as_ref()
            .unwrap()
            .unresolved
            .is_empty()
    );
    let image = notebook.read_section(&path).unwrap();
    let utf16: Vec<u8> = TEXT.encode_utf16().flat_map(u16::to_le_bytes).collect();
    assert!(!image.windows(utf16.len()).any(|w| w == utf16));
    let key = notebook.unlock(&path, "first").unwrap();
    assert_eq!(key.secret(), first.secret());
    let session = notebook.section_unlocked(&path, &key, || {}).unwrap();
    assert_eq!(session.pages().unwrap()[0].1, TEXT);
    session.close().unwrap();
    assert!(!leaks(&notebook.replica_path(&path).unwrap(), TEXT));

    assert!(notebook.set_password(&path, None, Some("second")).is_err());
    let second = notebook
        .set_password(&path, Some(&key), Some("second"))
        .unwrap()
        .unwrap();
    assert!(notebook.unlock(&path, "first").is_err());
    let session = notebook.section_unlocked(&path, &second, || {}).unwrap();
    assert_eq!(session.pages().unwrap()[0].1, TEXT);
    session.close().unwrap();

    assert!(
        notebook
            .set_password(&path, Some(&second), None)
            .unwrap()
            .is_none()
    );
    let section = &notebook.catalog().sections[0];
    assert!(matches!(section.state, SectionState::Readable { .. }));
    let session = notebook.section(&path, || {}).unwrap();
    assert_eq!(session.pages().unwrap()[0].1, TEXT);
}

/// A section whose edits wait to be published keeps its password until they are.
#[test]
fn a_password_waits_for_queued_edits() {
    let temporary = tempfile::tempdir().unwrap();
    let folder = temporary.path().join("notebook");
    let creation = onestore::PageCreation::new(None, Some("Waiting"), "Fixture").unwrap();
    let mut notebook = Notebook::create(
        &folder,
        temporary.path().join("cache"),
        0x00f0_a0c0,
        &creation,
    )
    .unwrap();
    let path = notebook.catalog().sections[0].path.clone();
    let session = notebook.section(&path, || {}).unwrap();
    session.set_offline(true);
    let (space, ..) = session.pages().unwrap()[0].clone();
    let page = session.page(space).unwrap();
    let mut edited = page.clone();
    edited.title = "Waiting still".into();
    let ops = onestore::op::lower_page(&page, &edited).unwrap();
    session
        .apply(
            "Fixture",
            Edit {
                at: 134_000_000_000_000_000,
                ops: ops.into_iter().map(|op| Op::Page { space, op }).collect(),
            },
        )
        .unwrap();
    session.written().unwrap();
    session.close().unwrap();
    assert!(notebook.set_password(&path, None, Some("early")).is_err());
    assert!(matches!(
        notebook.catalog().sections[0].state,
        SectionState::Readable { .. }
    ));
}

/// A notebook `here` makes, also opened `there` as another device opens it, with its first
/// section's path and the page title it starts with.
fn two_devices(temporary: &Path, title: &str) -> (Notebook, Notebook, String) {
    let folder = temporary.join("notebook");
    let creation = onestore::PageCreation::new(None, Some(title), "Fixture").unwrap();
    let here = Notebook::create(&folder, temporary.join("here"), 0x00f0_a0c0, &creation).unwrap();
    let path = here.catalog().sections[0].path.clone();
    let there = Notebook::open(&folder, temporary.join("there")).unwrap();
    (here, there, path)
}

/// Another device's replica of a section, plaintext, holds none of it once the section is
/// protected here and that device reads the notebook again.
#[test]
fn a_replica_drops_its_plaintext_once_its_section_is_protected_elsewhere() {
    const TITLE: &str = "Read on the other device";
    let temporary = tempfile::tempdir().unwrap();
    let (mut here, mut there, path) = two_devices(temporary.path(), TITLE);
    there.section(&path, || {}).unwrap().close().unwrap();
    let replica = there.replica_path(&path).unwrap();
    assert!(leaks(&replica, TITLE));
    here.set_password(&path, None, Some("secret")).unwrap();
    there.refresh().unwrap();
    assert!(!leaks(&replica, TITLE));
    let again = Notebook::open(
        temporary.path().join("notebook"),
        temporary.path().join("there"),
    );
    assert!(matches!(
        again.unwrap().catalog().sections[0].state,
        SectionState::Locked
    ));
    assert!(!leaks(&replica, TITLE));
}

/// An open section that another device protects stops synchronizing as protected, and its
/// plaintext goes once it closes.
#[test]
fn an_open_section_protected_elsewhere_stops_as_protected() {
    const TITLE: &str = "Open while protected";
    let temporary = tempfile::tempdir().unwrap();
    let (mut here, mut there, path) = two_devices(temporary.path(), TITLE);
    let session = there.section(&path, || {}).unwrap();
    let replica = there.replica_path(&path).unwrap();
    here.set_password(&path, None, Some("secret")).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        session.wake();
        let state = session.sync_status().unwrap().state();
        if state == notebook::session::SyncState::Protected {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "still {state:?}");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    session.close().unwrap();
    there.refresh().unwrap();
    assert!(!leaks(&replica, TITLE));
}

/// Edits another device queued before the section was protected wait in its replica until
/// that device unlocks the section, then publish under the key, the page they changed coming
/// back as a copy, as a page another client removed does; nothing stays in the clear.
#[test]
fn edits_queued_before_a_password_publish_under_it_once_unlocked() {
    const TITLE: &str = "Protected meanwhile";
    const TYPED: &str = "Typed offline meanwhile";
    let temporary = tempfile::tempdir().unwrap();
    let (mut here, mut there, path) = two_devices(temporary.path(), TITLE);
    let session = there.section(&path, || {}).unwrap();
    session.set_offline(true);
    let (space, ..) = session.pages().unwrap()[0].clone();
    let page = session.page(space).unwrap();
    let Some(PageObject::Title(title)) = page.objects.first() else {
        panic!("no title")
    };
    let text = title.outlines[0].paragraphs[0].text().unwrap().id;
    let end = TITLE.encode_utf16().count() as u32;
    session
        .apply("Fixture", replace(space, text, 0..end, TYPED))
        .unwrap();
    session.written().unwrap();
    session.close().unwrap();
    let held = there.replica_path(&path).unwrap();

    here.set_password(&path, None, Some("secret")).unwrap();
    there.refresh().unwrap();
    assert_eq!(Replica::open(&held).unwrap().pending().unwrap().len(), 1);

    let key = there.unlock(&path, "secret").unwrap();
    assert!(!held.exists());
    let session = there.section_unlocked(&path, &key, || {}).unwrap();
    let titles: Vec<String> = session
        .pages()
        .unwrap()
        .into_iter()
        .map(|(_, title, _)| title)
        .collect();
    assert_eq!(titles, [TITLE, TYPED]);
    published(&session);
    session.close().unwrap();
    assert!(!leaks(&there.replica_path(&path).unwrap(), TYPED));
    let image = there.read_section(&path).unwrap();
    let utf16: Vec<u8> = TYPED.encode_utf16().flat_map(u16::to_le_bytes).collect();
    assert!(!image.windows(utf16.len()).any(|w| w == utf16));
    let stored = notebook::session::stored_pages_unlocked(&image, &key).unwrap();
    assert!(stored.iter().any(|page| page.page.title == TYPED));
}

/// Waits until `section` has published every edit.
fn published(section: &notebook::session::Section) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    section.wake();
    while !section.pending().unwrap().is_empty() {
        assert!(
            std::time::Instant::now() < deadline,
            "edits were not published"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// Appends `text` to the first paragraph of every page of `section`.
fn append(section: &notebook::session::Section, text: &str) {
    for (space, ..) in section.pages().unwrap() {
        let page = section.page(space).unwrap();
        let Some((id, current)) = texts(&page).into_iter().next() else {
            continue;
        };
        let end = current.encode_utf16().count() as u32;
        section
            .apply("Snowbound Test", replace(space, id, end..end, text))
            .unwrap();
    }
    section.written().unwrap();
}

/// The native gate's candidate: a notebook whose sections Snowbound protected, edited under
/// their keys, gave another password, unprotected, and merged into a conflict page, each
/// from OneNote 2010's pages. `ONESTORE_PROTECTED_EXPORT` names a new directory receiving it
/// and its passwords for a cold open in OneNote.
#[test]
fn a_notebook_protected_through_its_sessions() {
    let source = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../corpus/protected-sections/source/synthetic.one"
    ))
    .unwrap();
    let pages = notebook::session::stored_pages(&source).unwrap();
    let temporary = tempfile::tempdir().unwrap();
    let folder = temporary.path().join("notebook");
    let cache = temporary.path().join("cache");
    let creation = onestore::PageCreation::new(None, Some("Gate"), "Snowbound Test").unwrap();
    let mut notebook = Notebook::create(&folder, &cache, 0x00f0_a0c0, &creation).unwrap();
    let names = ["Protected", "Edited", "Changed", "Removed", "Conflicted"];
    for name in names {
        let path = notebook.create_section("", name, &creation).unwrap();
        let section = notebook.section(&path, || {}).unwrap();
        for stored in &pages {
            section.import_page(&stored.page, "Snowbound Test").unwrap();
        }
        published(&section);
        section.close().unwrap();
    }
    let path = |name: &str| format!("{name}.one");
    let mut passwords = serde_json::Map::new();
    let mut keys = std::collections::BTreeMap::new();
    for name in names {
        let password = format!("{name} gate password");
        let key = notebook
            .set_password(&path(name), None, Some(&password))
            .unwrap()
            .unwrap();
        passwords.insert(path(name), password.into());
        keys.insert(name, key);
    }
    // Edited under its key: text on every page, published as dependent revisions.
    let section = notebook
        .section_unlocked(&path("Edited"), &keys["Edited"], || {})
        .unwrap();
    for round in ["first", "second", "third"] {
        append(&section, &format!(" Edited {round} under the key."));
        published(&section);
    }
    section.close().unwrap();
    // Changed: another password.
    let changed = notebook
        .set_password(
            &path("Changed"),
            Some(&keys["Changed"]),
            Some("Changed again"),
        )
        .unwrap()
        .unwrap();
    passwords.insert(path("Changed"), "Changed again".into());
    let section = notebook
        .section_unlocked(&path("Changed"), &changed, || {})
        .unwrap();
    append(&section, " Written under the changed password.");
    published(&section);
    section.close().unwrap();
    // Removed: no password.
    assert!(
        notebook
            .set_password(&path("Removed"), Some(&keys["Removed"]), None)
            .unwrap()
            .is_none()
    );
    passwords.insert(path("Removed"), serde_json::Value::Null);
    // Conflicted: two copies edit one paragraph offline; the second to publish keeps a
    // conflict page.
    let second = Notebook::open(&folder, temporary.path().join("second")).unwrap();
    let key = &keys["Conflicted"];
    let ours = notebook
        .section_unlocked(&path("Conflicted"), key, || {})
        .unwrap();
    let theirs = second
        .section_unlocked(&path("Conflicted"), key, || {})
        .unwrap();
    for (section, word) in [(&ours, " Ours offline."), (&theirs, " Theirs offline.")] {
        section.set_offline(true);
        append(section, word);
    }
    ours.set_offline(false);
    published(&ours);
    theirs.set_offline(false);
    published(&theirs);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while theirs.conflicts().unwrap().is_empty() {
        assert!(std::time::Instant::now() < deadline, "no conflict page");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    published(&theirs);
    ours.close().unwrap();
    theirs.close().unwrap();

    let notebook = Notebook::open(&folder, temporary.path().join("check")).unwrap();
    for name in names {
        let image = notebook.read_section(&path(name)).unwrap();
        let clear = |word: &str| {
            let utf16: Vec<u8> = word.encode_utf16().flat_map(u16::to_le_bytes).collect();
            image.windows(utf16.len()).any(|w| w == utf16)
                || image.windows(word.len()).any(|w| w == word.as_bytes())
        };
        for word in [
            "Edited first",
            "Ours offline",
            "Theirs offline",
            "changed password",
        ] {
            assert!(!clear(word), "{name} holds {word} in the clear");
        }
        assert_eq!(clear("positioned outline"), name == "Removed", "{name}");
        let state = &notebook
            .catalog()
            .sections
            .iter()
            .find(|section| section.path == path(name))
            .unwrap()
            .state;
        assert_eq!(
            matches!(state, SectionState::Locked),
            name != "Removed",
            "{name}"
        );
    }
    assert!(
        notebook
            .catalog()
            .toc
            .as_ref()
            .unwrap()
            .unresolved
            .is_empty()
    );
    let key = notebook
        .unlock(&path("Conflicted"), "Conflicted gate password")
        .unwrap();
    let stored = Section::unlock(
        &Arena::default(),
        notebook.read_section(&path("Conflicted")).unwrap(),
        &key,
    )
    .unwrap()
    .conflicts()
    .unwrap();
    // Each page's first paragraph clashed, so each keeps a conflict page.
    assert_eq!(stored.len(), 2);
    if let Some(export) = std::env::var_os("ONESTORE_PROTECTED_EXPORT") {
        let export = Path::new(&export);
        copy(&folder, &export.join("notebook"));
        std::fs::write(
            export.join("passwords.json"),
            serde_json::to_vec_pretty(&passwords).unwrap(),
        )
        .unwrap();
    }
}
