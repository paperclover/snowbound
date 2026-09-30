//! Replicas are kept per notebook location, so copies of a notebook never share one.

use notebook::session::{Notebook, Section};
use onestore::{
    ExGuid,
    op::{Edit, Op, PageOp},
    page::{Page, PageObject},
};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

/// A notebook folder `name` in `directory` holding `First.one` and `Second.one`.
fn notebook(directory: &Path, name: &str) -> PathBuf {
    let root = directory.join(name);
    std::fs::create_dir(&root).unwrap();
    for section in ["First", "Second"] {
        let file = format!("{section}.one");
        std::fs::write(
            root.join(&file),
            onestore::create_section(&file, section, "Author").unwrap(),
        )
        .unwrap();
    }
    root
}

/// Copies the folder `from` to `to`, as Finder or Files duplicates a notebook.
fn duplicate(from: &Path, to: &Path) {
    std::fs::create_dir(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), to.join(entry.file_name())).unwrap();
    }
}

/// The first text of the section's first page, with its id and the page's space.
fn first(page: &Page) -> (ExGuid, String) {
    page.objects
        .iter()
        .find_map(|object| match object {
            PageObject::Outline(outline) => outline.paragraphs.iter().find_map(|paragraph| {
                let text = paragraph.text()?;
                Some((text.id, text.text.text().to_string()))
            }),
            _ => None,
        })
        .unwrap()
}

/// Types `text` at the start of the section's first page; the page's space.
fn typed(section: &Section, text: &str) -> ExGuid {
    let space = section.pages().unwrap()[0].0;
    let (id, _) = first(&section.page(space).unwrap());
    let edit = Edit {
        at: (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 11_644_473_600)
            * 10_000_000,
        ops: vec![Op::Page {
            space,
            op: PageOp::Text {
                text: id,
                range: 0..0,
                with: text.into(),
            },
        }],
    };
    section.replica().apply("Editor", edit).unwrap();
    space
}

fn stored(file: &Path, space: ExGuid) -> String {
    let arena = onestore::Arena::default();
    let section = onestore::Section::open(&arena, onestore::read_file(file).unwrap()).unwrap();
    first(&section.page(space).unwrap()).1
}

fn until(what: &str, mut accept: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !accept() {
        assert!(Instant::now() < deadline, "{what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Types `text` into `First.one` of `root` while working offline and closes it, leaving the
/// edit queued in its replica.
fn queued(root: &Path, cache: &Path, text: &str) -> ExGuid {
    let notebook = Notebook::open(root, cache).unwrap();
    let section = notebook.section("First.one", || {}).unwrap();
    section.set_offline(true);
    let space = typed(&section, text);
    assert_eq!(section.pending().unwrap().len(), 1);
    section.close().unwrap();
    space
}

/// Opens `First.one` of `root` and waits for it to publish what it holds.
fn published(root: &Path, cache: &Path) -> usize {
    let notebook = Notebook::open(root, cache).unwrap();
    let section = notebook.section("First.one", || {}).unwrap();
    let pending = section.pending().unwrap().len();
    until("the queue published", || {
        section.sync_status().unwrap().queued == 0
    });
    section.close().unwrap();
    pending
}

#[test]
fn copies_of_a_notebook_keep_their_own_replicas_and_publish_only_to_themselves() {
    let directory = tempfile::tempdir().unwrap();
    let cache = directory.path().join("cache");
    let original = notebook(directory.path(), "Notebook");
    let copy = directory.path().join("Notebook 2");
    duplicate(&original, &copy);
    let (a, b) = (
        Notebook::open(&original, &cache).unwrap(),
        Notebook::open(&copy, &cache).unwrap(),
    );
    assert_ne!(
        a.replica_path("First.one").unwrap(),
        b.replica_path("First.one").unwrap()
    );
    // Both open at once: neither waits on the other's replica.
    let (mine, theirs) = (
        a.section("First.one", || {}).unwrap(),
        b.section("First.one", || {}).unwrap(),
    );
    mine.set_offline(true);
    let space = typed(&mine, "Original ");
    theirs.wake();
    until("the copy synced", || {
        theirs.sync_status().unwrap().synced.is_some()
    });
    assert!(theirs.pending().unwrap().is_empty());
    mine.set_offline(false);
    until("the original published", || {
        stored(&original.join("First.one"), space).starts_with("Original ")
    });
    assert_eq!(stored(&copy.join("First.one"), space), "First");
    assert_eq!(first(&theirs.page(space).unwrap()).1, "First");
    mine.close().unwrap();
    theirs.close().unwrap();
}

#[test]
fn a_notebook_the_app_moves_keeps_its_queued_edits() {
    let directory = tempfile::tempdir().unwrap();
    let cache = directory.path().join("cache");
    let root = notebook(directory.path(), "Notebook");
    let space = queued(&root, &cache, "Queued ");
    let from = notebook::location::local(&root).unwrap();
    let renamed = directory.path().join("Renamed");
    std::fs::rename(&root, &renamed).unwrap();
    // A new notebook where the old one was: its location still exists, so only moving the
    // replicas explicitly carries the queue.
    notebook(directory.path(), "Notebook");
    let to = notebook::location::local(&renamed).unwrap();
    notebook::location::moved(&cache, &from, &to).unwrap();
    assert_eq!(published(&renamed, &cache), 1);
    assert!(stored(&renamed.join("First.one"), space).starts_with("Queued "));
    assert_eq!(published(&root, &cache), 0);
}

#[test]
fn a_notebook_moved_outside_the_app_takes_its_replicas_along() {
    let directory = tempfile::tempdir().unwrap();
    let cache = directory.path().join("cache");
    let root = notebook(directory.path(), "Notebook");
    let space = queued(&root, &cache, "Queued ");
    let moved = directory.path().join("Moved");
    std::fs::rename(&root, &moved).unwrap();
    assert_eq!(published(&moved, &cache), 1);
    assert!(stored(&moved.join("First.one"), space).starts_with("Queued "));
}

#[test]
fn a_replica_newer_than_a_surviving_copy_stays_behind() {
    let directory = tempfile::tempdir().unwrap();
    let cache = directory.path().join("cache");
    let root = notebook(directory.path(), "Notebook");
    let copy = directory.path().join("Copy");
    duplicate(&root, &copy);
    // The original moves on past the copy, then queues an edit and goes away.
    {
        let notebook = Notebook::open(&root, &cache).unwrap();
        let section = notebook.section("First.one", || {}).unwrap();
        typed(&section, "Published ");
        until("the original published", || {
            section.sync_status().unwrap().queued == 0
        });
        section.close().unwrap();
    }
    queued(&root, &cache, "Queued ");
    std::fs::remove_dir_all(&root).unwrap();
    assert_eq!(published(&copy, &cache), 0);
    let space = Notebook::open(&copy, &cache)
        .unwrap()
        .section("First.one", || {})
        .unwrap()
        .pages()
        .unwrap()[0]
        .0;
    assert_eq!(stored(&copy.join("First.one"), space), "First");
}

#[test]
fn a_replica_named_by_identity_alone_moves_to_the_first_location_that_opens_it() {
    let directory = tempfile::tempdir().unwrap();
    let cache = directory.path().join("cache");
    let root = notebook(directory.path(), "Notebook");
    queued(&root, &cache, "Queued ");
    let replica = Notebook::open(&root, &cache)
        .unwrap()
        .replica_path("First.one")
        .unwrap();
    let legacy = cache.join(replica.file_name().unwrap());
    std::fs::rename(&replica, &legacy).unwrap();
    assert_eq!(published(&root, &cache), 1);
    assert!(!legacy.exists());
}

#[test]
#[ignore = "expects copies set aside; duplicates now open as sections"]
fn a_section_copy_beside_its_original_opens_only_as_a_lone_file_with_its_own_replica() {
    let directory = tempfile::tempdir().unwrap();
    let cache = directory.path().join("cache");
    let root = notebook(directory.path(), "Notebook");
    std::fs::copy(root.join("First.one"), root.join("First 2.one")).unwrap();
    let mut notebook = Notebook::open(&root, &cache).unwrap();
    // Discovery lists the copy as unavailable, so one replica serves the section.
    assert!(notebook.section("First 2.one", || {}).is_err());
    assert_eq!(
        notebook
            .replicas()
            .iter()
            .filter(|known| known.path.starts_with("First"))
            .count(),
        1
    );
    let original = Section::open(root.join("First.one"), &cache, || {}).unwrap();
    let copy = Section::open(root.join("First 2.one"), &cache, || {}).unwrap();
    original.set_offline(true);
    typed(&original, "Mine ");
    assert!(copy.pending().unwrap().is_empty());
    original.close().unwrap();
    copy.close().unwrap();
}
