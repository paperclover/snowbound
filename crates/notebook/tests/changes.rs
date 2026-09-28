use notebook::session::{Change, Notebook};
use std::io::Write;

/// One client watches a notebook while another changes it on disk; changes follow file
/// identity, and an unreachable notebook keeps the last catalog.
#[test]
fn external_structure_changes_are_reported_by_identity() {
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
    let mut watcher = Notebook::open(&root, temporary.path().join("watcher")).unwrap();
    let mut other = Notebook::open(&root, temporary.path().join("other")).unwrap();
    assert_eq!(watcher.refresh().unwrap(), []);

    other
        .create_section(
            "",
            "Second",
            &onestore::PageCreation::new(None, Some(""), "Author").unwrap(),
        )
        .unwrap();
    other.create_group("", "Group").unwrap();
    other
        .create_section(
            "Group",
            "Inner",
            &onestore::PageCreation::new(None, Some(""), "Author").unwrap(),
        )
        .unwrap();
    assert_eq!(
        watcher.refresh().unwrap(),
        [
            Change::Added("Second.one".into()),
            Change::Added("Group".into()),
            Change::Added("Group/Inner.one".into()),
        ]
    );

    other.rename("Second.one", "Renamed").unwrap();
    assert_eq!(
        watcher.refresh().unwrap(),
        [Change::Moved {
            from: "Second.one".into(),
            to: "Renamed.one".into()
        }]
    );

    other.reorder("", &["Renamed.one", "First.one"]).unwrap();
    assert_eq!(watcher.refresh().unwrap(), [Change::Reordered("".into())]);

    other.delete("First.one").unwrap();
    assert_eq!(
        watcher.refresh().unwrap(),
        [
            Change::Moved {
                from: "First.one".into(),
                to: "OneNote_RecycleBin/First.one".into()
            },
            Change::Added("OneNote_RecycleBin".into()),
        ]
    );
    std::fs::remove_dir_all(root.join("OneNote_RecycleBin")).unwrap();
    assert_eq!(
        watcher.refresh().unwrap(),
        [
            Change::Removed("OneNote_RecycleBin".into()),
            Change::Removed("OneNote_RecycleBin/First.one".into()),
        ]
    );

    let away = temporary.path().join("away");
    std::fs::rename(&root, &away).unwrap();
    assert!(watcher.refresh().is_err());
    assert_eq!(watcher.catalog().sections.len(), 1);
    std::fs::rename(&away, &root).unwrap();
    assert_eq!(watcher.refresh().unwrap(), []);
    assert_eq!(watcher.refresh().unwrap(), []);
}
