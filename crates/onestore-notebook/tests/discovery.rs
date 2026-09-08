use onestore_notebook::{Entry, Error, Limits, Local, Source, discover};
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

#[test]
fn copied_identities_are_ambiguous_even_across_different_groups() {
    let root = tempfile::tempdir().unwrap();
    fixture(root.path());
    fs::create_dir(root.path().join("group")).unwrap();
    fs::copy(
        root.path().join("one.one"),
        root.path().join("group/other.one"),
    )
    .unwrap();
    assert!(
        matches!(discover(&mut Local::open(root.path()).unwrap(), limits()),
        Err(Error::DuplicateIdentity { first, second, .. })
            if [first.as_str(), second.as_str()].contains(&"one.one")
                && [first.as_str(), second.as_str()].contains(&"group/other.one"))
    );
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
        onestore_notebook::SectionState::Locked
    ));
    assert!(matches!(
        catalog.sections[1].state,
        onestore_notebook::SectionState::Unreadable(_)
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
    fs::write(root.path().join("one.one"), b"unfinished").unwrap();
    assert!(
        matches!(discover(&mut source, limits()), Err(Error::Document { path, .. }) if path == "one.one")
    );
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
