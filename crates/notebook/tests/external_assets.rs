use notebook::discover::{Error, Local, read_external_asset};
use std::{fs, io};

#[test]
fn nested_external_payloads_are_exact_and_empty_is_distinct_from_missing() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("Group 🦀/Section é_onefiles");
    fs::create_dir_all(&directory).unwrap();
    let filename = "2a83ae62-6754-4383-8e2b-4033ff3cfba1.onebin";
    let mut source = Local::open(root.path()).unwrap();
    let section = "Group 🦀/Section é.ONE";
    assert!(
        matches!(read_external_asset(&mut source, section, filename, 0),
        Err(Error::Io { error, .. }) if error.kind() == io::ErrorKind::NotFound)
    );
    fs::write(directory.join(filename), []).unwrap();
    assert!(
        read_external_asset(&mut source, section, filename, 0)
            .unwrap()
            .is_empty()
    );
    let bytes: Vec<_> = (0..65537).map(|index| (index % 251) as u8).collect();
    fs::write(directory.join(filename), &bytes).unwrap();
    assert!(
        matches!(read_external_asset(&mut source, section, filename, bytes.len()-1),
        Err(Error::Io { error, .. }) if error.kind() == io::ErrorKind::FileTooLarge)
    );
    assert_eq!(
        read_external_asset(&mut source, section, filename, bytes.len()).unwrap(),
        bytes
    );
    assert_eq!(fs::read(directory.join(filename)).unwrap(), bytes);
}

#[test]
fn invalid_asset_locations_fail_before_accessing_the_source() {
    struct Unused;
    impl notebook::discover::Source for Unused {
        fn entries(&mut self, _: &str, _: usize) -> io::Result<Vec<notebook::discover::Entry>> {
            panic!("Unexpected enumeration")
        }
        fn read(&mut self, _: &str, _: usize) -> io::Result<Vec<u8>> {
            panic!("Unexpected read")
        }
    }
    let filename = "2a83ae62-6754-4383-8e2b-4033ff3cfba1.onebin";
    for section in [
        "",
        ".one",
        "Group/.one",
        "/Section.one",
        "../Section.one",
        "Group/../Section.one",
        "Group\\Section.one",
        "Section.onetoc2",
    ] {
        assert!(matches!(
            read_external_asset(&mut Unused, section, filename, 100),
            Err(Error::Entry { .. })
        ));
    }
    for name in [
        "",
        "attachment.png",
        "../2a83ae62-6754-4383-8e2b-4033ff3cfba1.onebin",
        "2a83ae62-6754-4383-8e2b-4033ff3cfba1.onebin:other",
        "<invfdo>",
    ] {
        assert!(matches!(
            read_external_asset(&mut Unused, "Section.one", name, 100),
            Err(Error::Entry { .. })
        ));
    }
}

#[test]
fn reserved_payload_directories_are_not_notebook_groups() {
    let root = tempfile::tempdir().unwrap();
    fs::write(
        root.path().join("Section.ONE"),
        onestore::create_section("Section.one", "Fixture", "Author").unwrap(),
    )
    .unwrap();
    for name in ["section_onefiles", "orphan_onefiles", "ordinary"] {
        fs::create_dir(root.path().join(name)).unwrap();
    }
    fs::write(
        root.path().join("section_onefiles/unfinished.onebin"),
        b"payload",
    )
    .unwrap();
    let catalog = notebook::discover::discover(
        &mut Local::open(root.path()).unwrap(),
        notebook::discover::Limits {
            entries: 4,
            bytes_per_file: 1 << 20,
            depth: 1,
        },
    )
    .unwrap();
    assert_eq!(catalog.sections.len(), 1);
    assert_eq!(
        catalog
            .groups
            .iter()
            .map(|group| group.path.as_str())
            .collect::<Vec<_>>(),
        ["ordinary"]
    );
}

#[test]
#[cfg(feature = "smb")]
#[ignore = "requires a disposable Samba mirror of corpus/native-external-assets/notebook at ONESTORE_SMB_NOTEBOOK"]
fn live_external_payloads() {
    use onestore::{
        FileDataReference, RevisionIndex, Store,
        document::{Document, Kind},
    };
    let root = std::path::Path::new("../../corpus/native-external-assets/notebook");
    let bytes = fs::read(root.join("synthetic.one")).unwrap();
    let store = Store::parse(&bytes).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let client = notebook::smb::Client::connect(
        &std::env::var("ONESTORE_SMB_LAB").unwrap(),
        "agent",
        notebook::smb::Credentials::default(),
        std::time::Duration::from_secs(5),
    )
    .unwrap();
    let mut remote =
        notebook::discover::Smb::new(&client, &std::env::var("ONESTORE_SMB_NOTEBOOK").unwrap())
            .unwrap();
    let mut local = Local::open(root).unwrap();
    let mut seen = std::collections::BTreeSet::new();
    for node in document
        .spaces
        .values()
        .flat_map(|space| space.revisions.values())
        .flat_map(|revision| revision.nodes.values())
    {
        if let Kind::File {
            reference: FileDataReference::External(filename),
            ..
        } = &node.kind
            && seen.insert(filename)
        {
            let expected = fs::read(root.join("synthetic_onefiles").join(filename)).unwrap();
            assert_eq!(
                read_external_asset(&mut local, "synthetic.one", filename, expected.len()).unwrap(),
                expected
            );
            assert_eq!(
                read_external_asset(&mut remote, "synthetic.one", filename, expected.len())
                    .unwrap(),
                expected
            );
        }
    }
    assert_eq!(seen.len(), 3);
}
