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
        notebook.create_section("", "Second", "Author").unwrap(),
        "Second.one"
    );
    assert_eq!(notebook.create_group("", "Archive").unwrap(), "Archive");
    assert_eq!(
        notebook
            .create_section("Archive", "Inner", "Author")
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
