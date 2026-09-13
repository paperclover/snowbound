#![cfg(feature = "protected")]

use notebook::{discover::SectionState, session::Notebook};
use std::path::Path;

/// A locked section is discovered as such and reads with its password, page by page.
#[test]
fn a_locked_section_unlocks_for_reading() {
    let root = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../corpus/native-protected-boundaries"
    ));
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("manifest.json")).unwrap()).unwrap();
    let password = manifest["password"].as_str().unwrap();
    let temporary = tempfile::tempdir().unwrap();
    let notebook = Notebook::open(root.join("notebook"), temporary.path()).unwrap();
    let section = &notebook.catalog().sections[0];
    assert!(matches!(section.state, SectionState::Locked));
    let pages = notebook.unlock(&section.path, password).unwrap();
    assert_eq!(pages.len(), 11);
    assert!(pages.iter().all(|(_, page)| !page.title.is_empty()));
    assert!(matches!(
        notebook.unlock(&section.path, "wrong"),
        Err(notebook::Error::Protected(
            onestore::protected::Error::PasswordMismatch
        ))
    ));
}
