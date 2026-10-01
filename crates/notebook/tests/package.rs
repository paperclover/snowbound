//! OneNote packages and the copies Save As writes (`corpus/notebook-package`).

use notebook::{package, session::Notebook};
use std::path::Path;

fn header(image: &[u8]) -> onestore::Header {
    onestore::Store::parse(image).unwrap().header
}

/// Each entry of the table of contents `image` holds: its filename and file identity.
fn listed(image: &[u8]) -> Vec<(String, [u8; 16])> {
    let store = onestore::Store::parse(image).unwrap();
    let index = onestore::RevisionIndex::parse(&store).unwrap();
    let document = onestore::document::Document::parse(&index).unwrap();
    let revision = document.active(document.root).unwrap();
    let onestore::document::Kind::Toc { entries, .. } = &revision.nodes[&revision.roots[&1]].kind
    else {
        panic!()
    };
    (entries.iter())
        .map(|id| match &revision.nodes[id].kind {
            onestore::document::Kind::Toc {
                filename, identity, ..
            } => (filename.clone().unwrap_or_default(), identity.unwrap()),
            _ => panic!(),
        })
        .collect()
}

/// A notebook of a section, a group holding one, and a page deleted into the recycle bin.
fn notebook(root: &Path, cache: &Path) -> Notebook {
    let page = |title: &str| onestore::PageCreation::new(None, Some(title), "Author").unwrap();
    let mut notebook = Notebook::create(root, cache, 0x00d7ff, &page("First")).unwrap();
    notebook.create_group("", "Group").unwrap();
    notebook
        .create_section("Group", "Inner", &page("Inside"))
        .unwrap();
    notebook
        .create_section("", "Doomed", &page("Binned"))
        .unwrap();
    let image = notebook.read_section("Doomed.one").unwrap();
    let pages = notebook::session::stored_pages(&image).unwrap();
    notebook
        .recycle_pages(&[pages[0].page.clone()], "Author")
        .unwrap();
    notebook.delete("Doomed.one").unwrap();
    notebook
}

/// Pack writes the notebook's tables of contents and sections, the recycle bin left out,
/// outside any notebook; Unpack Notebook gives every file a new identity, placed under its
/// folder's table of contents, whose entries follow, and the notebook opens as it was.
/// `NOTEBOOK_PACKAGE_EXPORT` names a new directory receiving the package and the notebook
/// unpacked, for OneNote 2010 to unpack and to open cold.
#[test]
fn a_notebook_packs_and_unpacks_as_onenote_packs_one() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("Packed");
    let cache = temporary.path().join("cache");
    let notebook = notebook(&root, &cache);
    let files = package::notebook_files(&notebook, |_| None).unwrap();
    let paths: Vec<&str> = files.iter().map(|(path, _)| path.as_str()).collect();
    assert_eq!(
        paths,
        [
            "Open Notebook.onetoc2",
            "New Section 1.one",
            "Group/Open Notebook.onetoc2",
            "Group/Inner.one"
        ]
    );
    let packed = package::pack(files.clone()).unwrap();
    let read = package::read(&packed, 1 << 30).unwrap();
    assert_eq!(read.len(), files.len());
    for ((path, bytes), (stored, original)) in read.iter().zip(&files) {
        assert_eq!(path, stored);
        let (unplaced, placed) = (header(bytes), header(original));
        assert_eq!(
            unplaced.file_id, placed.file_id,
            "{path} keeps its identity"
        );
        assert_eq!(
            (unplaced.ancestor, unplaced.name_crc),
            ([0; 16], 0),
            "{path}"
        );
        assert_eq!(bytes[..128], original[..128]);
        assert_eq!(bytes[148..], original[148..]);
    }

    let unpacked = temporary.path().join("Unpacked");
    package::unpack(read, &unpacked).unwrap();
    let opened = Notebook::open(&unpacked, &cache).unwrap();
    let catalog = opened.catalog();
    let sections = |folder: &notebook::discover::Folder| -> Vec<String> {
        (folder.sections.iter())
            .map(|section| section.path.clone())
            .collect()
    };
    assert_eq!(sections(catalog), ["New Section 1.one"]);
    assert_eq!(sections(&catalog.groups[0]), ["Group/Inner.one"]);
    assert!(catalog.unavailable.is_empty() && catalog.groups[0].unavailable.is_empty());
    let read = |path: &str| std::fs::read(unpacked.join(path)).unwrap();
    let root_toc = read("Open Notebook.onetoc2");
    let group_toc = read("Group/Open Notebook.onetoc2");
    let section = read("New Section 1.one");
    let inner = read("Group/Inner.one");
    for (unpacked, (_, original)) in [&root_toc, &section, &group_toc, &inner]
        .into_iter()
        .zip(&files)
    {
        assert_ne!(header(unpacked).file_id, header(original).file_id);
    }
    assert_eq!(header(&root_toc).ancestor, [0; 16]);
    assert_eq!(header(&section).ancestor, header(&root_toc).file_id);
    assert_eq!(header(&group_toc).ancestor, header(&root_toc).file_id);
    assert_eq!(header(&inner).ancestor, header(&group_toc).file_id);
    // The recycle bin's entry stays, as in OneNote's packages, naming a folder not there.
    let bin = listed(&files[0].1)[2].clone();
    assert_eq!(bin.0, "OneNote_RecycleBin");
    assert_eq!(
        listed(&root_toc),
        [
            ("New Section 1.one".to_owned(), header(&section).file_id),
            ("Group".to_owned(), header(&group_toc).file_id),
            bin
        ]
    );
    assert_eq!(
        listed(&group_toc),
        [("Inner.one".to_owned(), header(&inner).file_id)]
    );
    let pages = notebook::session::stored_pages(&inner).unwrap();
    assert_eq!(pages[0].page.title, "Inside");
    // Placed as they are, nothing is re-identified when the notebook is read again.
    drop(opened);
    let reopened = Notebook::open(&unpacked, &cache).unwrap();
    assert_eq!(
        reopened.catalog().sections[0].file_id,
        header(&section).file_id
    );
    assert!(
        package::unpack(Vec::new(), &unpacked).is_err(),
        "the folder is new"
    );

    if let Some(directory) = std::env::var_os("NOTEBOOK_PACKAGE_EXPORT") {
        let directory = Path::new(&directory);
        std::fs::create_dir(directory).unwrap();
        std::fs::write(directory.join("Packed.onepkg"), &packed).unwrap();
        copy(&unpacked, &directory.join("Unpacked"));
    }
}

fn copy(from: &Path, to: &Path) {
    std::fs::create_dir(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            copy(&entry.path(), &to.join(entry.file_name()));
        } else {
            std::fs::copy(entry.path(), to.join(entry.file_name())).unwrap();
        }
    }
}

/// OneNote 2010's own package (LZX) unpacks and opens.
#[test]
fn onenotes_package_unpacks() {
    let package = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus/notebook-package/native/Lab.onepkg");
    let files = package::read(&std::fs::read(package).unwrap(), 1 << 30).unwrap();
    let paths: Vec<&str> = files.iter().map(|(path, _)| path.as_str()).collect();
    assert_eq!(paths, ["Alpha.one", "Beta.one", "Open Notebook.onetoc2"]);
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("Lab");
    package::unpack(files, &root).unwrap();
    let notebook = Notebook::open(&root, temporary.path().join("cache")).unwrap();
    let sections: Vec<&str> = (notebook.catalog().sections.iter())
        .map(|section| section.path.as_str())
        .collect();
    assert_eq!(sections, ["Alpha.one", "Beta.one"]);
    let alpha =
        notebook::session::stored_pages(&notebook.read_section("Alpha.one").unwrap()).unwrap();
    let titles: Vec<&str> = alpha.iter().map(|page| page.page.title.as_str()).collect();
    assert_eq!(titles, ["Alpha one", "Alpha three"]);
}

/// A package naming a file outside its folder, or unpacking past the limit, is refused.
#[test]
fn a_package_stays_in_its_folder() {
    let section = onestore::create_empty_section("A.one", None).unwrap();
    for path in ["../A.one", "Group/../../A.one", "C:/A.one", ""] {
        let packed = package::pack(vec![(path.to_owned(), section.clone())]).unwrap();
        assert!(package::read(&packed, 1 << 30).is_err(), "{path}");
    }
    let packed = package::pack(vec![("A.one".to_owned(), section.clone())]).unwrap();
    assert!(package::read(&packed, 10).is_err());
}

/// Save As writes a section as a copy outside any notebook, and a page as a section of its
/// own holding it, as OneNote 2010 does. `NOTEBOOK_SAVE_AS_EXPORT` names a new directory
/// receiving both, for OneNote 2010 to open cold.
#[test]
fn save_as_writes_copies_outside_the_notebook() {
    let temporary = tempfile::tempdir().unwrap();
    let notebook = notebook(
        &temporary.path().join("Notebook"),
        &temporary.path().join("cache"),
    );
    let image = notebook.read_section("Group/Inner.one").unwrap();
    let copy = package::section_copy(image.clone()).unwrap();
    assert_ne!(header(&copy).file_id, header(&image).file_id);
    assert_eq!(header(&copy).ancestor, [0; 16]);
    assert_eq!(
        notebook::session::stored_pages(&copy).unwrap()[0]
            .page
            .title,
        "Inside"
    );

    let page = notebook::session::stored_pages(&image)
        .unwrap()
        .remove(0)
        .page;
    let saved = package::page_section(&page, "Inside.one", Some(0xe4a88a), "Author").unwrap();
    let pages = notebook::session::stored_pages(&saved).unwrap();
    assert_eq!(pages.len(), 1);
    assert_eq!(
        (pages[0].page.title.as_str(), pages[0].page.identity),
        ("Inside", page.identity)
    );
    assert_eq!(header(&saved).ancestor, [0; 16]);
    if let Some(directory) = std::env::var_os("NOTEBOOK_SAVE_AS_EXPORT") {
        let directory = Path::new(&directory);
        std::fs::create_dir(directory).unwrap();
        std::fs::write(directory.join("Inner copy.one"), &copy).unwrap();
        std::fs::write(directory.join("Inside.one"), &saved).unwrap();
    }
}

/// Empty Recycle Bin deletes Deleted Pages' pages and every binned section's file, leaving the
/// bin's table of contents as it was, as OneNote 2010 does.
#[test]
fn emptying_the_recycle_bin_leaves_its_table_of_contents() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("Notebook");
    let mut notebook = notebook(&root, &temporary.path().join("cache"));
    let bin = root.join("OneNote_RecycleBin");
    let toc = std::fs::read(bin.join("Open Notebook.onetoc2")).unwrap();
    assert!(bin.join("Doomed.one").exists());
    notebook.empty_recycle_bin().unwrap();
    assert!(!bin.join("Doomed.one").exists());
    let deleted = std::fs::read(bin.join("OneNote_DeletedPages.one")).unwrap();
    assert!(
        notebook::session::stored_pages(&deleted)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        std::fs::read(bin.join("Open Notebook.onetoc2")).unwrap(),
        toc
    );
    notebook.empty_recycle_bin().unwrap();
}
