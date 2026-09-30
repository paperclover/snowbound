use notebook::session::Notebook;
use std::io::Write;

/// Each file's header names its parent TOC and carries the name CRC OneNote wrote for the
/// same name in `corpus/notebook-edit/{structured,deleted}/cold`.
fn placement(root: &std::path::Path, expected: &[(&str, u32)]) {
    let header = |path: std::path::PathBuf| {
        onestore::Store::parse(&std::fs::read(path).unwrap())
            .unwrap()
            .header
    };
    for (path, crc) in expected {
        let (folder, name) = path.rsplit_once('/').unwrap_or(("", path));
        let parent = header(root.join(folder).join("Open Notebook.onetoc2")).file_id;
        let file = if name.ends_with(".one") {
            root.join(path)
        } else {
            root.join(path).join("Open Notebook.onetoc2")
        };
        let header = header(file);
        assert_eq!((header.ancestor, header.name_crc), (parent, *crc), "{path}");
    }
}

fn names(folder: &notebook::discover::Folder) -> (Vec<String>, Vec<String>) {
    (
        folder.sections.iter().map(|s| s.path.clone()).collect(),
        folder.groups.iter().map(|g| g.path.clone()).collect(),
    )
}

/// `NOTEBOOK_STRUCTURE_EXPORT` names a new directory receiving the final notebook for a
/// cold reopen.
#[test]
fn sections_and_groups_are_created_renamed_coloured_ordered_and_deleted() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("notebook");
    std::fs::create_dir(&root).unwrap();
    let first = onestore::create_section("First.one", "First page", "Author").unwrap();
    std::fs::File::create_new(root.join("First.one"))
        .unwrap()
        .write_all(&first)
        .unwrap();
    let first_id = onestore::Store::parse(&first).unwrap().header.file_id;
    std::fs::write(
        root.join("Open Notebook.onetoc2"),
        onestore::create_table_of_contents("Open Notebook.onetoc2", &[("First.one", first_id)])
            .unwrap(),
    )
    .unwrap();
    let toc = onestore::read_file(root.join("Open Notebook.onetoc2")).unwrap();
    let toc_id = onestore::Store::parse(&toc).unwrap().header.file_id;
    onestore::place_file(root.join("First.one"), toc_id, "First.one").unwrap();
    let mut notebook = Notebook::open(&root, temporary.path().join("cache")).unwrap();
    assert_eq!(
        notebook
            .create_section(
                "",
                "Second",
                &onestore::PageCreation::new(None, Some(""), "Author").unwrap()
            )
            .unwrap(),
        "Second.one"
    );
    assert_eq!(notebook.create_group("", "Archive").unwrap(), "Archive");
    assert_eq!(
        notebook
            .create_section(
                "Archive",
                "Inner",
                &onestore::PageCreation::new(None, Some(""), "Author").unwrap()
            )
            .unwrap(),
        "Archive/Inner.one"
    );
    assert_eq!(
        names(notebook.catalog()),
        (
            vec!["First.one".to_owned(), "Second.one".to_owned()],
            vec!["Archive".to_owned()]
        )
    );
    assert_eq!(
        names(&notebook.catalog().groups[0]),
        (vec!["Archive/Inner.one".to_owned()], vec![])
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
    assert_eq!(
        names(notebook.catalog()),
        (
            vec!["Renamed.one".to_owned(), "First.one".to_owned()],
            vec!["Kept".to_owned()]
        )
    );
    let toc = notebook.catalog().toc.as_ref().unwrap();
    let orders: Vec<(Option<String>, u32)> = toc
        .unresolved
        .iter()
        .map(|entry| (entry.filename.clone(), entry.order))
        .collect();
    assert!(orders.is_empty(), "{orders:?}");
    let colour = onestore::read_file(root.join("Renamed.one")).unwrap();
    let store = onestore::Store::parse(&colour).unwrap();
    let index = onestore::RevisionIndex::parse(&store).unwrap();
    let document = onestore::document::Document::parse(&index).unwrap();
    let revision = document.active(document.root).unwrap();
    let onestore::document::Kind::SectionMetadata { color, .. } =
        &revision.nodes[&revision.roots[&2]].kind
    else {
        panic!()
    };
    assert_eq!(*color, Some(0x5ed7ff));
    placement(
        &root,
        &[
            ("First.one", 0x2e1b24f3),
            ("Renamed.one", 0x6108912a),
            ("Kept", 0xf2a400f4),
            ("Kept/Inner.one", 0x83e261da),
        ],
    );
    if let Some(directory) = std::env::var_os("NOTEBOOK_STRUCTURE_EXPORT") {
        copy_dir(&root, std::path::Path::new(&directory));
    }
    notebook.delete("First.one").unwrap();
    assert_eq!(
        names(notebook.catalog()),
        (
            vec!["Renamed.one".to_owned()],
            vec!["Kept".to_owned(), "OneNote_RecycleBin".to_owned()]
        )
    );
    assert_eq!(
        names(&notebook.catalog().groups[1]),
        (vec!["OneNote_RecycleBin/First.one".to_owned()], vec![])
    );
    assert!(root.join("OneNote_RecycleBin/First.one").exists());
    let bin = onestore::read_file(root.join("OneNote_RecycleBin/Open Notebook.onetoc2")).unwrap();
    assert!(
        String::from_utf16_lossy(
            &bin.chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect::<Vec<_>>()
        )
        .contains("First.one")
    );
    assert!(notebook.rename("Kept/Inner.one", "First").is_ok());
    assert!(notebook.delete("Missing.one").is_err());
    placement(
        &root,
        &[
            ("Renamed.one", 0x6108912a),
            ("Kept", 0xf2a400f4),
            ("Kept/First.one", 0x2e1b24f3),
            ("OneNote_RecycleBin", 0x986b646a),
            ("OneNote_RecycleBin/First.one", 0x2e1b24f3),
        ],
    );
    if let Some(directory) = std::env::var_os("NOTEBOOK_STRUCTURE_EXPORT_DELETED") {
        copy_dir(&root, std::path::Path::new(&directory));
    }
}

fn copy_dir(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

#[test]
fn a_rename_keeps_its_place_in_the_notebook_and_in_a_group() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("notebook");
    let page = onestore::PageCreation::new(None, Some(""), "Author").unwrap();
    let mut notebook =
        Notebook::create(&root, temporary.path().join("cache"), 0x00d7ff, &page).unwrap();
    notebook.create_group("", "Group").unwrap();
    for folder in ["", "Group"] {
        for name in ["Video", "Song", "Album"] {
            notebook.create_section(folder, name, &page).unwrap();
        }
    }
    let path = |folder: &str, name: &str| {
        if folder.is_empty() {
            format!("{name}.one")
        } else {
            format!("{folder}/{name}.one")
        }
    };
    notebook
        .reorder(
            "",
            &["Album.one", "Video.one", "New Section 1.one", "Song.one"],
        )
        .unwrap();
    notebook
        .reorder("Group", &["Group/Album.one", "Group/Video.one"])
        .unwrap();
    notebook.rename("Video.one", "Kitchen").unwrap();
    notebook.rename("Group/Video.one", "Kitchen").unwrap();
    notebook.rename("Group", "Renamed").unwrap();
    assert_eq!(
        names(notebook.catalog()),
        (
            ["Album", "Kitchen", "New Section 1", "Song"]
                .map(|name| path("", name))
                .to_vec(),
            vec!["Renamed".to_owned()]
        )
    );
    assert_eq!(
        names(&notebook.catalog().groups[0]).0,
        ["Album", "Kitchen", "Song"].map(|name| path("Renamed", name))
    );
}

/// Each entry a TOC file lists: its filename and file identity.
fn listed(path: &std::path::Path) -> Vec<(String, [u8; 16])> {
    let image = onestore::read_file(path).unwrap();
    let store = onestore::Store::parse(&image).unwrap();
    let index = onestore::RevisionIndex::parse(&store).unwrap();
    let document = onestore::document::Document::parse(&index).unwrap();
    let revision = document.active(document.root).unwrap();
    let onestore::document::Kind::Toc { entries, .. } = &revision.nodes[&revision.roots[&1]].kind
    else {
        panic!()
    };
    entries
        .iter()
        .map(|id| match &revision.nodes[id].kind {
            onestore::document::Kind::Toc {
                filename, identity, ..
            } => (filename.clone().unwrap_or_default(), identity.unwrap()),
            _ => panic!(),
        })
        .collect()
}

fn file_id(path: &std::path::Path) -> [u8; 16] {
    onestore::Store::parse(&onestore::read_file(path).unwrap())
        .unwrap()
        .header
        .file_id
}

#[test]
fn a_page_delete_lists_the_recycle_bin_it_finds_or_makes_and_leaves_an_unreadable_one() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("notebook");
    let bin = root.join("OneNote_RecycleBin");
    let page = onestore::PageCreation::new(None, Some("Kept"), "Author").unwrap();
    let mut notebook =
        Notebook::create(&root, temporary.path().join("cache"), 0x00d7ff, &page).unwrap();
    let recycle = |notebook: &mut Notebook| {
        let image = notebook.read_section("New Section 1.one").unwrap();
        let pages = notebook::session::stored_pages(&image).unwrap();
        notebook.recycle_pages(&[pages[0].page.clone()], "Author")
    };
    let bins = |root: &std::path::Path| -> Vec<[u8; 16]> {
        listed(&root.join("Open Notebook.onetoc2"))
            .into_iter()
            .filter(|(name, _)| name == "OneNote_RecycleBin")
            .map(|(_, identity)| identity)
            .collect()
    };
    recycle(&mut notebook).unwrap();

    // Gone from the folder, the bin's entry is stale; the next bin takes its name.
    std::fs::remove_dir_all(&bin).unwrap();
    notebook.refresh().unwrap();
    recycle(&mut notebook).unwrap();
    let toc = bin.join("Open Notebook.onetoc2");
    assert_eq!(bins(&root), [file_id(&toc)]);
    let deleted = file_id(&bin.join("OneNote_DeletedPages.one"));
    assert_eq!(listed(&toc), [("OneNote_DeletedPages.one".into(), deleted)]);

    // A bin whose TOC has another name and misses the deleted pages is kept and completed.
    std::fs::remove_file(&toc).unwrap();
    let other = bin.join("OneNote Table Of Contents.onetoc2");
    std::fs::write(
        &other,
        onestore::create_table_of_contents("OneNote Table Of Contents.onetoc2", &[]).unwrap(),
    )
    .unwrap();
    let parent = file_id(&root.join("Open Notebook.onetoc2"));
    onestore::place_file(&other, parent, "OneNote_RecycleBin").unwrap();
    notebook.refresh().unwrap();
    recycle(&mut notebook).unwrap();
    assert!(!toc.exists());
    assert_eq!(bins(&root), [file_id(&other)]);
    assert_eq!(
        listed(&other),
        [("OneNote_DeletedPages.one".into(), deleted)]
    );
    let ancestor =
        onestore::Store::parse(&onestore::read_file(bin.join("OneNote_DeletedPages.one")).unwrap())
            .unwrap()
            .header
            .ancestor;
    assert_eq!(ancestor, file_id(&other));
    // `NOTEBOOK_RECYCLE_EXPORT` names a new directory receiving it for a cold reopen.
    if let Some(directory) = std::env::var_os("NOTEBOOK_RECYCLE_EXPORT") {
        copy_dir(&root, std::path::Path::new(&directory));
    }
    // A bin that cannot be read is neither replaced nor added to.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Permission bits do not bind the superuser.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o000)).unwrap();
        let denied = notebook.refresh().and_then(|_| recycle(&mut notebook));
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(denied.is_err());
        let names: Vec<_> = std::fs::read_dir(&bin)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names.len(), 2, "{names:?}");
    }
}
