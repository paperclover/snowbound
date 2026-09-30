use notebook::discover::{Cache, Entry, Error, Limits, Local, Reason, Source, discover};
use std::{fs, io, path::Path};

fn limits() -> Limits {
    Limits {
        entries: 1000,
        bytes_per_file: 16 * 1024 * 1024,
        depth: 16,
    }
}

fn fixture(root: &Path) -> Vec<u8> {
    let section = onestore::create_section("one.one", "Retained 🦀", "Fixture").unwrap();
    fs::write(root.join("one.one"), &section).unwrap();
    let identity = onestore::Store::parse(&section).unwrap().header.file_id;
    let toc = onestore::create_table_of_contents("Open Notebook.onetoc2", &[("one.one", identity)])
        .unwrap();
    fs::write(root.join("Open Notebook.onetoc2"), toc).unwrap();
    section
}

#[test]
fn rename_preserves_identity_and_does_not_trust_a_stale_cached_filename() {
    let root = tempfile::tempdir().unwrap();
    let section = fixture(root.path());
    let mut source = Local::open(root.path()).unwrap();
    let before = discover(&mut source, limits()).unwrap();
    fs::rename(
        root.path().join("one.one"),
        root.path().join("Renamed 🦀.ONE"),
    )
    .unwrap();
    let after = discover(&mut source, limits()).unwrap();
    assert_eq!(before.sections[0].file_id, after.sections[0].file_id);
    assert_eq!(after.sections[0].path, "Renamed 🦀.ONE");
    assert!(after.toc.as_ref().unwrap().unresolved.is_empty());
    assert_eq!(
        fs::read(root.path().join("Renamed 🦀.ONE")).unwrap(),
        section
    );
    fs::write(
        root.path().join("one.one"),
        onestore::create_section("one.one", "Different", "Fixture").unwrap(),
    )
    .unwrap();
    let with_replacement = discover(&mut source, limits()).unwrap();
    assert_eq!(
        with_replacement.sections[0].file_id,
        before.sections[0].file_id
    );
    assert_eq!(with_replacement.sections[0].path, "Renamed 🦀.ONE");
    assert_ne!(
        with_replacement.sections[1].file_id,
        before.sections[0].file_id
    );
    fs::remove_file(root.path().join("Renamed 🦀.ONE")).unwrap();
    let absent = discover(&mut source, limits()).unwrap();
    assert_eq!(absent.toc.as_ref().unwrap().unresolved.len(), 1);
    assert_eq!(
        absent.toc.as_ref().unwrap().unresolved[0].file,
        before.sections[0].file_id
    );
    assert_ne!(
        absent.sections[0].file_id,
        absent.toc.as_ref().unwrap().unresolved[0].file
    );
}

/// Sets `path`'s modification time to `seconds` after the epoch.
fn touch(path: &Path, seconds: u64) {
    fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds))
        .unwrap();
}

fn copy_of(unavailable: &notebook::discover::Unavailable) -> Option<&str> {
    match &unavailable.reason {
        Reason::Copy { of } => Some(of),
        _ => None,
    }
}

/// Which of the folder's sections are copies of another, by path.
fn copies(folder: &notebook::discover::Folder) -> Vec<(&str, bool)> {
    folder
        .sections
        .iter()
        .map(|section| (section.path.as_str(), section.copy))
        .collect()
}

/// OneNote 2010 opens a copied section file beside its original as a section of its own
/// (`Cross 2.one`, 2026-09-29 lab run); the one the TOC names is the original, however
/// short the copy's path.
#[test]
fn a_copied_section_lists_beside_the_one_the_toc_names() {
    let root = tempfile::tempdir().unwrap();
    fixture(root.path());
    fs::create_dir(root.path().join("group")).unwrap();
    for copy in ["o.one", "group/other.one"] {
        fs::copy(root.path().join("one.one"), root.path().join(copy)).unwrap();
        touch(&root.path().join(copy), 2_000_000_000);
    }
    let catalog = discover(&mut Local::open(root.path()).unwrap(), limits()).unwrap();
    assert!(catalog.unavailable.is_empty());
    let mut listed = copies(&catalog);
    listed.sort();
    assert_eq!(listed, [("o.one", true), ("one.one", false)]);
    assert_eq!(copies(&catalog.groups[0]), [("group/other.one", true)]);
}

/// Without a TOC, the original is the copy with the shortest path, however new the others:
/// a choice that stands while the files keep their names.
#[test]
fn without_a_toc_the_shortest_path_is_the_original() {
    let root = tempfile::tempdir().unwrap();
    fixture(root.path());
    fs::remove_file(root.path().join("Open Notebook.onetoc2")).unwrap();
    fs::copy(root.path().join("one.one"), root.path().join("one 2.one")).unwrap();
    touch(&root.path().join("one.one"), 1_000_000_000);
    touch(&root.path().join("one 2.one"), 2_000_000_000);
    let catalog = discover(&mut Local::open(root.path()).unwrap(), limits()).unwrap();
    let mut listed = copies(&catalog);
    listed.sort();
    assert_eq!(listed, [("one 2.one", true), ("one.one", false)]);
}

#[test]
fn a_copied_group_lists_once_with_its_sections() {
    let root = tempfile::tempdir().unwrap();
    fixture(root.path());
    let group = root.path().join("Group");
    fs::create_dir(&group).unwrap();
    let inner = onestore::create_section("Inner.one", "Inner", "Fixture").unwrap();
    fs::write(group.join("Inner.one"), &inner).unwrap();
    let identity = onestore::Store::parse(&inner).unwrap().header.file_id;
    fs::write(
        group.join("Open Notebook.onetoc2"),
        onestore::create_table_of_contents("Open Notebook.onetoc2", &[("Inner.one", identity)])
            .unwrap(),
    )
    .unwrap();
    let copy = root.path().join("Group 2");
    fs::create_dir(&copy).unwrap();
    for name in ["Inner.one", "Open Notebook.onetoc2"] {
        fs::copy(group.join(name), copy.join(name)).unwrap();
        touch(&group.join(name), 1_000_000_000);
        touch(&copy.join(name), 2_000_000_000);
    }
    let catalog = discover(&mut Local::open(root.path()).unwrap(), limits()).unwrap();
    assert_eq!(catalog.groups.len(), 1);
    assert_eq!(catalog.groups[0].path, "Group 2");
    assert_eq!(catalog.groups[0].sections[0].path, "Group 2/Inner.one");
    assert_eq!(catalog.unavailable.len(), 1);
    assert_eq!(catalog.unavailable[0].path, "Group");
    assert!(catalog.unavailable[0].group);
    assert_eq!(copy_of(&catalog.unavailable[0]), Some("Group 2"));
}

/// iOS lists a file iCloud Drive evicted as `.Name.icloud`: the section lists as not
/// downloaded under its own name, and nothing writes a TOC over an evicted one.
#[test]
fn evicted_files_list_under_their_names_until_downloaded() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("notebook");
    fs::create_dir(&root).unwrap();
    let section = fixture(&root);
    fs::rename(root.join("one.one"), root.join(".one.one.icloud")).unwrap();
    let mut source = Local::open(&root).unwrap();
    let catalog = discover(&mut source, limits()).unwrap();
    assert!(catalog.sections.is_empty());
    assert_eq!(catalog.unavailable.len(), 1);
    assert_eq!(catalog.unavailable[0].path, "one.one");
    assert_eq!(catalog.unavailable[0].reason, Reason::Evicted);
    // Downloading can leave the placeholder beside the file for a moment.
    fs::write(root.join("one.one"), &section).unwrap();
    let catalog = discover(&mut source, limits()).unwrap();
    assert_eq!(catalog.sections[0].path, "one.one");
    assert!(catalog.unavailable.is_empty());

    fs::remove_file(root.join(".one.one.icloud")).unwrap();
    fs::rename(
        root.join("Open Notebook.onetoc2"),
        root.join(".Open Notebook.onetoc2.icloud"),
    )
    .unwrap();
    let catalog = discover(&mut source, limits()).unwrap();
    assert!(catalog.toc.is_none());
    assert_eq!(catalog.sections[0].path, "one.one");
    let mut notebook =
        notebook::session::Notebook::open(&root, temporary.path().join("cache")).unwrap();
    let page = onestore::PageCreation::new(None, Some(""), "Author").unwrap();
    assert!(notebook.create_section("", "Second", &page).is_err());
    assert!(!root.join("Open Notebook.onetoc2").exists());
}

/// A source whose files list as macOS lists those iCloud Drive evicted: dataless, under their
/// names and listings, and never read.
struct Dataless(Local);

impl Source for Dataless {
    fn entries(&mut self, path: &str, limit: usize) -> io::Result<Vec<Entry>> {
        let mut entries = self.0.entries(path, limit)?;
        for entry in &mut entries {
            entry.kind = notebook::discover::EntryKind::Evicted;
        }
        Ok(entries)
    }

    fn read(&mut self, path: &str, _: usize) -> io::Result<Vec<u8>> {
        panic!("read {path}, which is not on this device");
    }
}

/// A dataless file lists as its last read found it, where it lists unchanged since, and as not
/// downloaded otherwise.
#[test]
fn dataless_files_list_as_last_read_or_wait_for_their_download() {
    let root = tempfile::tempdir().unwrap();
    fixture(root.path());
    let mut cache = Cache::default();
    cache
        .discover(&mut Local::open(root.path()).unwrap(), limits())
        .unwrap();
    let catalog = cache
        .discover(&mut Dataless(Local::open(root.path()).unwrap()), limits())
        .unwrap();
    assert_eq!(catalog.sections[0].path, "one.one");
    assert!(catalog.toc.is_some() && catalog.unavailable.is_empty());

    let catalog = discover(&mut Dataless(Local::open(root.path()).unwrap()), limits()).unwrap();
    assert!(catalog.sections.is_empty() && catalog.toc.is_none());
    assert_eq!(catalog.unavailable[0].path, "one.one");
    assert_eq!(catalog.unavailable[0].reason, Reason::Evicted);
}

#[test]
fn locked_and_unreadable_sections_retain_their_storage_identity() {
    let root = tempfile::tempdir().unwrap();
    for (name, fixture) in [
        (
            "locked.one",
            "native-encrypted/encrypted-01/notebook/synthetic.one",
        ),
        (
            "stub.one",
            "native-encrypted/cold-encrypted-02/notebook/Open Notebook.one",
        ),
    ] {
        fs::copy(
            Path::new("../../corpus").join(fixture),
            root.path().join(name),
        )
        .unwrap();
    }
    let catalog = discover(&mut Local::open(root.path()).unwrap(), limits()).unwrap();
    assert!(catalog.toc.is_none());
    assert!(matches!(
        catalog.sections[0].state,
        notebook::discover::SectionState::Locked
    ));
    assert!(matches!(
        catalog.sections[1].state,
        notebook::discover::SectionState::Unreadable(_)
    ));
    for section in catalog.sections {
        let bytes = fs::read(root.path().join(section.path)).unwrap();
        assert_eq!(
            section.file_id,
            onestore::Store::parse(&bytes).unwrap().header.file_id
        );
    }
}

#[test]
fn a_group_rename_retains_toc_identity_and_parent_order() {
    let root = tempfile::tempdir().unwrap();
    let native = Path::new("../../corpus/m6/native-features-01/notebook");
    for relative in [
        "Open Notebook.onetoc2",
        "Group A/Open Notebook.onetoc2",
        "Group A/Duplicate.one",
        "Group B/Open Notebook.onetoc2",
        "Group B/Nested/Open Notebook.onetoc2",
        "Group B/Nested/Duplicate.one",
    ] {
        let target = root.path().join(relative);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::copy(native.join(relative), target).unwrap();
    }
    let before = discover(&mut Local::open(root.path()).unwrap(), limits()).unwrap();
    assert_eq!(before.groups[0].path, "Group A");
    fs::rename(root.path().join("Group A"), root.path().join("Renamed 🦀")).unwrap();
    let catalog = discover(&mut Local::open(root.path()).unwrap(), limits()).unwrap();
    assert_eq!(catalog.groups[0].path, "Renamed 🦀");
    assert_eq!(
        catalog.groups[0].toc.as_ref().unwrap().file_id,
        before.groups[0].toc.as_ref().unwrap().file_id
    );
    assert_eq!(catalog.groups[1].path, "Group B");
    assert_eq!(
        catalog.groups[0].sections[0].path,
        "Renamed 🦀/Duplicate.one"
    );
    assert_eq!(
        serde_json::to_value(&catalog.toc.unwrap().unresolved).unwrap(),
        serde_json::to_value(&before.toc.unwrap().unresolved).unwrap()
    );
}

#[test]
fn limits_and_incomplete_files_do_not_produce_a_catalog() {
    let root = tempfile::tempdir().unwrap();
    fixture(root.path());
    let mut source = Local::open(root.path()).unwrap();
    assert!(
        discover(
            &mut source,
            Limits {
                entries: 1,
                ..limits()
            }
        )
        .is_err()
    );
    assert!(
        discover(
            &mut source,
            Limits {
                bytes_per_file: 8,
                ..limits()
            }
        )
        .is_err()
    );
    fs::create_dir(root.path().join("group")).unwrap();
    assert!(matches!(
        discover(
            &mut source,
            Limits {
                depth: 0,
                ..limits()
            }
        ),
        Err(Error::Limit { .. })
    ));
    fs::write(root.path().join("Open Notebook.onetoc2"), b"unfinished").unwrap();
    assert!(
        matches!(discover(&mut source, limits()), Err(Error::Document { path, .. }) if path == "Open Notebook.onetoc2")
    );
}

#[test]
fn a_section_that_cannot_be_read_lists_as_unavailable_and_the_rest_open() {
    /// Answers as a share does a file mid-commit, as many times as `busy` counts for its
    /// path, and fails the path `failure` names.
    struct Busy {
        source: Local,
        busy: Vec<(&'static str, usize)>,
        failure: Option<(&'static str, io::ErrorKind)>,
    }
    impl Source for Busy {
        fn entries(&mut self, path: &str, limit: usize) -> io::Result<Vec<Entry>> {
            self.source.entries(path, limit)
        }
        fn read(&mut self, path: &str, limit: usize) -> io::Result<Vec<u8>> {
            if let Some((_, kind)) = self.failure.filter(|(failing, _)| *failing == path) {
                return Err(kind.into());
            }
            if let Some((_, left)) = self.busy.iter_mut().find(|(busy, _)| *busy == path)
                && *left > 0
            {
                *left -= 1;
                return Err(io::ErrorKind::WouldBlock.into());
            }
            self.source.read(path, limit)
        }
    }
    let root = tempfile::tempdir().unwrap();
    fixture(root.path());
    for name in ["briefly.one", "stuck.one"] {
        fs::write(
            root.path().join(name),
            onestore::create_section(name, "Text", "Fixture").unwrap(),
        )
        .unwrap();
    }
    fs::write(root.path().join("broken.one"), b"unfinished").unwrap();
    let mut source = Busy {
        source: Local::open(root.path()).unwrap(),
        busy: vec![("briefly.one", 2), ("stuck.one", usize::MAX)],
        failure: Some(("one.one", io::ErrorKind::InvalidData)),
    };
    let catalog = discover(&mut source, limits()).unwrap();
    let sections: Vec<_> = catalog
        .sections
        .iter()
        .map(|section| &section.path)
        .collect();
    assert_eq!(sections, ["briefly.one"]);
    let unavailable: Vec<_> = catalog
        .unavailable
        .iter()
        .map(|entry| (entry.path.as_str(), &entry.reason))
        .collect();
    assert_eq!(
        unavailable,
        [
            ("broken.one", &Reason::Unreadable),
            ("one.one", &Reason::Unreadable),
            ("stuck.one", &Reason::InUse),
        ]
    );

    // A lost connection still fails the whole discovery.
    source.failure = Some(("one.one", io::ErrorKind::ConnectionAborted));
    assert!(matches!(
        discover(&mut source, limits()),
        Err(Error::Io { path, error }) if path == "one.one" && error.kind() == io::ErrorKind::ConnectionAborted
    ));
}

#[test]
fn a_failed_refresh_retains_the_previous_catalog_as_an_independent_value() {
    struct Changing {
        source: Local,
        reads: usize,
        fail: bool,
    }
    impl Source for Changing {
        fn entries(&mut self, path: &str, limit: usize) -> io::Result<Vec<Entry>> {
            self.reads += 1;
            let mut entries = self.source.entries(path, limit)?;
            if self.reads == 2 {
                if self.fail {
                    return Err(io::ErrorKind::ConnectionAborted.into());
                }
                entries[0].name.push_str(".renamed");
            }
            Ok(entries)
        }
        fn read(&mut self, path: &str, limit: usize) -> io::Result<Vec<u8>> {
            self.source.read(path, limit)
        }
    }
    let root = tempfile::tempdir().unwrap();
    fixture(root.path());
    let mut source = Local::open(root.path()).unwrap();
    let catalog = discover(&mut source, limits()).unwrap();
    let before = serde_json::to_value(&catalog).unwrap();
    for fail in [false, true] {
        let mut changing = Changing {
            source: Local::open(root.path()).unwrap(),
            reads: 0,
            fail,
        };
        let error = discover(&mut changing, limits()).unwrap_err();
        if fail {
            assert!(
                matches!(error, Error::Io { error, .. } if error.kind() == io::ErrorKind::ConnectionAborted)
            );
        } else {
            assert!(matches!(error, Error::Changed { .. }));
        }
        assert_eq!(serde_json::to_value(&catalog).unwrap(), before);
    }
}

#[test]
fn local_access_stays_inside_the_selected_root() {
    let root = tempfile::tempdir().unwrap();
    fixture(root.path());
    let mut source = Local::open(root.path()).unwrap();
    for path in [
        "../one.one",
        "/one.one",
        "x/../one.one",
        "one.one\0",
        "x\\one.one",
    ] {
        assert_eq!(
            source.read(path, 100).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
    }
    #[cfg(unix)]
    {
        let other = tempfile::tempdir().unwrap();
        fs::write(other.path().join("other.one"), b"outside").unwrap();
        std::os::unix::fs::symlink(other.path().join("other.one"), root.path().join("link.one"))
            .unwrap();
        assert_eq!(
            source.read("link.one", 100).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        assert!(
            matches!(discover(&mut source, limits()), Err(Error::Entry { path }) if path == "link.one")
        );
    }
}

/// Restores permissions a test took away, so the temporary directory can be removed.
#[cfg(unix)]
struct Restore(Vec<(std::path::PathBuf, u32)>);

#[cfg(unix)]
impl Drop for Restore {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;
        for (path, mode) in &self.0 {
            let _ = fs::set_permissions(path, fs::Permissions::from_mode(*mode));
        }
    }
}

#[cfg(unix)]
#[test]
fn unreadable_children_are_listed_as_unavailable_and_retried() {
    use std::os::unix::fs::PermissionsExt;
    // Permission bits do not bind the superuser.
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("notebook");
    fs::create_dir(&root).unwrap();
    fixture(&root);
    fs::create_dir(root.join("Art")).unwrap();
    fs::create_dir(root.join("Group")).unwrap();
    for name in ["kept.one", "denied.one"] {
        fs::write(
            root.join("Group").join(name),
            onestore::create_section(name, "Text", "Fixture").unwrap(),
        )
        .unwrap();
    }
    fs::write(
        root.join("Group/Open Notebook.onetoc2"),
        onestore::create_table_of_contents("Open Notebook.onetoc2", &[]).unwrap(),
    )
    .unwrap();
    let _restore = Restore(vec![
        (root.join("Art"), 0o755),
        (root.join("Group/denied.one"), 0o644),
    ]);
    for path in [root.join("Art"), root.join("Group/denied.one")] {
        fs::set_permissions(path, fs::Permissions::from_mode(0o000)).unwrap();
    }

    let mut notebook =
        notebook::session::Notebook::open(&root, directory.path().join("cache")).unwrap();
    let catalog = notebook.catalog();
    assert_eq!(catalog.sections.len(), 1);
    assert_eq!(catalog.groups.len(), 1);
    assert_eq!(catalog.unavailable.len(), 1);
    assert_eq!(catalog.unavailable[0].path, "Art");
    assert!(catalog.unavailable[0].group);
    assert!(catalog.unavailable[0].error.contains("Permission denied"));
    let group = &catalog.groups[0];
    assert_eq!(group.sections.len(), 1);
    assert_eq!(group.sections[0].path, "Group/kept.one");
    assert_eq!(group.unavailable[0].path, "Group/denied.one");
    assert!(!group.unavailable[0].group);

    // An unavailable entry is not the catalog's to change, nor a group holding one.
    assert!(notebook.delete("Art").is_err());
    assert!(notebook.delete("Group").is_err());
    assert!(root.join("Group/kept.one").exists());
    assert!(notebook.rename("Art", "Other").is_err());
    assert!(notebook.create_group("", "Art").is_err());

    fs::set_permissions(root.join("Art"), fs::Permissions::from_mode(0o755)).unwrap();
    notebook.refresh().unwrap();
    assert!(notebook.catalog().unavailable.is_empty());
    assert_eq!(notebook.catalog().groups.len(), 2);

    // The notebook's own folder is still the notebook.
    fs::set_permissions(&root, fs::Permissions::from_mode(0o000)).unwrap();
    let denied = notebook.refresh();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(denied.is_err());
}

/// A local folder that notes each file read, with copies of some files by identity.
struct Counting {
    source: Local,
    read: Vec<String>,
    copies: Vec<([u8; 16], Vec<u8>)>,
}

impl Source for Counting {
    fn entries(&mut self, path: &str, limit: usize) -> io::Result<Vec<Entry>> {
        self.source.entries(path, limit)
    }

    fn read(&mut self, path: &str, limit: usize) -> io::Result<Vec<u8>> {
        self.read.push(path.to_owned());
        self.source.read(path, limit)
    }

    fn copy(&mut self, _: &str, known: [u8; 16]) -> Option<Vec<u8>> {
        let (_, copy) = self
            .copies
            .iter()
            .find(|(identity, _)| *identity == known)?;
        Some(copy.clone())
    }
}

#[test]
fn a_cached_discovery_reads_only_the_files_listed_otherwise() {
    let root = tempfile::tempdir().unwrap();
    fixture(root.path());
    let two = |title| onestore::create_section("two.one", title, "Fixture").unwrap();
    fs::write(root.path().join("two.one"), two("Two")).unwrap();
    let mut source = Counting {
        source: Local::open(root.path()).unwrap(),
        read: Vec::new(),
        copies: Vec::new(),
    };
    let mut cache = Cache::default();
    let first = cache.discover(&mut source, limits()).unwrap();
    assert_eq!(std::mem::take(&mut source.read).len(), 3);
    let again = cache.discover(&mut source, limits()).unwrap();
    assert!(source.read.is_empty(), "{:?}", source.read);
    assert_eq!(
        serde_json::to_value(&again).unwrap(),
        serde_json::to_value(&first).unwrap()
    );

    let changed = two("Two, changed");
    fs::write(root.path().join("two.one"), &changed).unwrap();
    let after = cache.discover(&mut source, limits()).unwrap();
    assert_eq!(std::mem::take(&mut source.read), ["two.one"]);
    let fresh = discover(&mut Local::open(root.path()).unwrap(), limits()).unwrap();
    assert_eq!(
        serde_json::to_value(&after).unwrap(),
        serde_json::to_value(&fresh).unwrap()
    );
    assert!(cache.found("two.one").unwrap().1 == onestore::Stamp::of(&changed).unwrap());

    // Kept across launches, it reads nothing unchanged.
    let mut kept: Cache = serde_json::from_slice(&serde_json::to_vec(&cache).unwrap()).unwrap();
    kept.discover(&mut source, limits()).unwrap();
    assert!(source.read.is_empty(), "{:?}", source.read);

    // A file written as a copy of it holds, as a replica its own edits published from, is
    // taken from the copy.
    let copied = two("Two, published");
    fs::write(root.path().join("two.one"), &copied).unwrap();
    let identity = onestore::Store::parse(&changed).unwrap().header.file_id;
    source.copies.push((identity, copied.clone()));
    kept.discover(&mut source, limits()).unwrap();
    assert!(source.read.is_empty(), "{:?}", source.read);
    assert!(kept.found("two.one").unwrap().1 == onestore::Stamp::of(&copied).unwrap());

    // A failed discovery leaves the cache as it was.
    fs::write(root.path().join("Open Notebook.onetoc2"), b"unfinished").unwrap();
    assert!(kept.discover(&mut source, limits()).is_err());
    assert!(kept.found("two.one").is_some());
}

/// A share in memory, as a Mac's SMB client leaves it, that notes each file read. Finder
/// writes another AppleDouble file into the root while discovery is under way.
struct Share {
    files: std::collections::BTreeMap<String, Vec<u8>>,
    read: Vec<String>,
    listings: usize,
}

impl Source for Share {
    fn entries(&mut self, path: &str, _: usize) -> io::Result<Vec<Entry>> {
        if path.is_empty() {
            self.listings += 1;
            if self.listings > 1 {
                self.files
                    .insert("._Later.one".into(), b"\0\x05\x16\x07".to_vec());
            }
        }
        let prefix = if path.is_empty() {
            String::new()
        } else {
            format!("{path}/")
        };
        let mut entries: Vec<Entry> = Vec::new();
        for (file, bytes) in &self.files {
            let Some(rest) = file.strip_prefix(&prefix) else {
                continue;
            };
            let (name, kind) = match rest.split_once('/') {
                Some((folder, _)) => (folder, notebook::discover::EntryKind::Directory),
                None => (rest, notebook::discover::EntryKind::File),
            };
            if entries.last().is_some_and(|last| last.name == name) {
                continue;
            }
            entries.push(Entry {
                name: name.into(),
                kind,
                listed: notebook::discover::Listed {
                    size: bytes.len() as u64,
                    modified: 1,
                },
            });
        }
        Ok(entries)
    }

    fn read(&mut self, path: &str, _: usize) -> io::Result<Vec<u8>> {
        self.read.push(path.to_owned());
        self.files
            .get(path)
            .cloned()
            .ok_or_else(|| io::ErrorKind::NotFound.into())
    }
}

#[test]
fn metadata_other_systems_leave_on_a_share_is_not_the_notebook() {
    let section = onestore::create_section("Section 003.one", "Kept", "Fixture").unwrap();
    let identity = onestore::Store::parse(&section).unwrap().header.file_id;
    let toc = onestore::create_table_of_contents(
        "Open Notebook.onetoc2",
        &[("Section 003.one", identity)],
    )
    .unwrap();
    let inner = onestore::create_section("Inner.one", "Inner", "Fixture").unwrap();
    // AppleDouble companions and `.DS_Store` begin with their own magic, not a OneNote header.
    let apple_double = [&b"\0\x05\x16\x07\0\x02\0\0Mac OS X        "[..], &[0; 4064]].concat();
    let junk = [
        ("._Section 003.one", apple_double.clone()),
        ("._Open Notebook.onetoc2", apple_double.clone()),
        (".DS_Store", b"\0\0\0\x01Bud1".to_vec()),
        ("Thumbs.db", vec![0xd0, 0xcf, 0x11, 0xe0]),
        ("desktop.ini", b"[.ShellClassInfo]".to_vec()),
        ("~$Section 003.one", vec![7; 162]),
        (".snowbound/tags.one", b"{}".to_vec()),
        (".snowbound/Open Notebook.onetoc2", b"{}".to_vec()),
        ("Group/._Inner.one", apple_double),
        ("Group/.DS_Store", b"\0\0\0\x01Bud1".to_vec()),
    ];
    let mut source = Share {
        files: [
            ("Section 003.one", section),
            ("Open Notebook.onetoc2", toc),
            ("Group/Inner.one", inner),
        ]
        .into_iter()
        .chain(junk)
        .map(|(path, bytes)| (path.to_owned(), bytes))
        .collect(),
        read: Vec::new(),
        listings: 0,
    };
    let catalog = discover(&mut source, limits()).unwrap();
    assert_eq!(
        catalog
            .sections
            .iter()
            .map(|section| &section.path[..])
            .collect::<Vec<_>>(),
        ["Section 003.one"]
    );
    assert!(catalog.toc.unwrap().unresolved.is_empty());
    assert_eq!(catalog.groups.len(), 1);
    assert_eq!(catalog.groups[0].sections[0].path, "Group/Inner.one");
    assert_eq!(catalog.groups[0].sections.len(), 1);
    source.read.sort();
    assert_eq!(
        source.read,
        [
            "Group/Inner.one",
            "Open Notebook.onetoc2",
            "Section 003.one"
        ]
    );
}

#[test]
fn a_discovery_holds_each_section_it_read_once() {
    let root = tempfile::tempdir().unwrap();
    let section = fixture(root.path());
    let mut cache = Cache::default();
    cache
        .discover(&mut Local::open(root.path()).unwrap(), limits())
        .unwrap();
    let images = cache.take();
    assert_eq!(images.keys().collect::<Vec<_>>(), ["one.one"]);
    assert_eq!(images["one.one"], section);
    assert!(cache.take().is_empty(), "taken once");
    // Nothing read, nothing held.
    cache
        .discover(&mut Local::open(root.path()).unwrap(), limits())
        .unwrap();
    assert!(cache.take().is_empty());
}
