use notebook::session::Notebook;
use onestore::page::link::{LinkTarget, internal_link};
use std::io::Write;

fn write_section(root: &std::path::Path, name: &str, text: &str) -> [u8; 16] {
    let bytes = onestore::create_section(name, text, "Author").unwrap();
    std::fs::File::create_new(root.join(name))
        .unwrap()
        .write_all(&bytes)
        .unwrap();
    onestore::Store::parse(&bytes).unwrap().header.file_id
}

/// Links resolve by identity: a page keeps its link when its section is renamed or moved,
/// and a link whose section identity is stale still finds the page in another section.
#[test]
fn internal_links_find_their_page_by_identity() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("notebook");
    std::fs::create_dir(&root).unwrap();
    let first = write_section(&root, "First.one", "Target page");
    let second = write_section(&root, "Second.one", "Other page");
    std::fs::write(
        root.join("Open Notebook.onetoc2"),
        onestore::create_table_of_contents(
            "Open Notebook.onetoc2",
            &[("First.one", first), ("Second.one", second)],
        )
        .unwrap(),
    )
    .unwrap();
    let mut notebook = Notebook::open(&root, temporary.path().join("cache")).unwrap();
    let section = notebook.section("First.one", || {}).unwrap();
    let (space, _) = section.pages().unwrap()[0];
    let page = section.page(space).unwrap();
    section.close().unwrap();
    let target = LinkTarget::Page {
        identity: page.identity.unwrap(),
        title: &page.title,
    };
    let link = internal_link(first, "C:\\notebook\\First.one", target);
    assert_eq!(
        notebook.find_page(&link).unwrap(),
        Some(("First.one".to_owned(), Some(space)))
    );
    assert_eq!(
        notebook
            .find_page(&internal_link(first, "", LinkTarget::Section))
            .unwrap(),
        Some(("First.one".to_owned(), None))
    );
    assert_eq!(
        notebook.find_page("https://example.invalid/").unwrap(),
        None
    );

    notebook.create_group("", "Group").unwrap();
    notebook.rename("First.one", "Renamed").unwrap();
    assert_eq!(
        notebook.find_page(&link).unwrap(),
        Some(("Renamed.one".to_owned(), Some(space)))
    );

    let stale = internal_link(second, "", target);
    assert_eq!(
        notebook.find_page(&stale).unwrap(),
        Some(("Renamed.one".to_owned(), Some(space)))
    );
    let unknown = internal_link(
        second,
        "",
        LinkTarget::Page {
            identity: [7; 16],
            title: "Gone",
        },
    );
    assert_eq!(notebook.find_page(&unknown).unwrap(), None);
}
