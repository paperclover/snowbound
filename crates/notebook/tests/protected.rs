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
    let pages = notebook.unlock(&section.path, password).unwrap().pages;
    assert_eq!(pages.len(), 11);
    assert!(pages.iter().all(|(_, page)| !page.title.is_empty()));
    assert!(matches!(
        notebook.unlock(&section.path, "wrong"),
        Err(notebook::Error::Protected(
            onestore::protected::Error::PasswordMismatch
        ))
    ));
}

/// An unlocked page is edited as ops stored under the section's key, then reads back with
/// the same password. `ONESTORE_PROTECTED_EXPORT` names a directory receiving the notebook for
/// a cold reopen in OneNote.
#[test]
fn an_unlocked_page_is_saved_under_the_section_key() {
    use onestore::page::{PageObject, Paragraph, text::new_id};
    let root = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../corpus/native-encrypted"
    ));
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("manifest.json")).unwrap()).unwrap();
    let password = manifest["password"].as_str().unwrap();
    let temporary = tempfile::tempdir().unwrap();
    let copy = temporary.path().join("notebook");
    std::fs::create_dir(&copy).unwrap();
    for entry in std::fs::read_dir(root.join("encrypted-01/notebook")).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), copy.join(entry.file_name())).unwrap();
    }
    let notebook = Notebook::open(&copy, temporary.path().join("cache")).unwrap();
    let section = notebook
        .catalog()
        .sections
        .iter()
        .find(|section| matches!(section.state, SectionState::Locked))
        .unwrap()
        .path
        .clone();
    let mut unlocked = notebook.unlock(&section, password).unwrap();
    let (space, mut page) = unlocked.pages[0].clone();
    let before = page.clone();
    let paragraphs = page
        .objects
        .iter_mut()
        .find_map(|object| match object {
            PageObject::Outline(outline) if !outline.title => Some(&mut outline.paragraphs),
            _ => None,
        })
        .unwrap();
    let at = paragraphs.iter().position(|p| p.text().is_some()).unwrap();
    let mut added = paragraphs[at].clone();
    added.id = new_id().unwrap();
    added.style = None;
    added.lists.clear();
    added.tags.clear();
    let text = added.text_mut().unwrap();
    text.id = new_id().unwrap();
    let format = text.text.format_at(0).unwrap().clone();
    text.text = Paragraph::new(
        "Written while protected 🦀".to_owned(),
        onestore::document::Format {
            font: Some("Calibri".into()),
            font_size: Some(11.0),
            language: Some(1033),
            ..Default::default()
        },
    );
    paragraphs.insert(at + 1, added);
    paragraphs[at]
        .text_mut()
        .unwrap()
        .text
        .append(Paragraph::new(
            " and edited under its key".to_owned(),
            format,
        ))
        .unwrap();
    let edit = onestore::op::Edit {
        at: 134_000_000_000_000_000,
        ops: onestore::op::lower_page(&before, &page)
            .unwrap()
            .into_iter()
            .map(|op| onestore::op::Op::Page { space, op })
            .collect(),
    };
    assert!(matches!(
        notebook.apply_unlocked(&section, "wrong", &mut unlocked, "Rust", &edit),
        Err(notebook::Error::Protected(
            onestore::protected::Error::PasswordMismatch
        ))
    ));
    let mut stale = notebook.unlock(&section, password).unwrap();
    notebook
        .apply_unlocked(&section, password, &mut unlocked, "Rust", &edit)
        .unwrap();
    // A view read before that edit no longer matches the file.
    assert!(
        notebook
            .apply_unlocked(&section, password, &mut stale, "Rust", &edit)
            .is_err()
    );
    let (_, stored) = notebook.unlock(&section, password).unwrap().pages.remove(0);
    assert!(stored.objects == page.objects);
    if let Some(export) = std::env::var_os("ONESTORE_PROTECTED_EXPORT") {
        let export = Path::new(&export);
        std::fs::create_dir_all(export).unwrap();
        for entry in std::fs::read_dir(&copy).unwrap() {
            let entry = entry.unwrap();
            std::fs::copy(entry.path(), export.join(entry.file_name())).unwrap();
        }
    }
}
