//! A rewritten object stores the global id table entries it names, not the table of the
//! revision it was read from.

use onestore::{
    PreparedEdit, RevisionIndex, Store,
    document::Document,
    page::{Page, PageObject, Paragraph},
};

const SOURCE: &[u8] = include_bytes!("../../../corpus/native/20260905-05/notebook/synthetic.one");

fn pages(bytes: &[u8]) -> Vec<(onestore::ExGuid, Page)> {
    let store = Store::parse(bytes).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    document
        .pages()
        .unwrap()
        .into_iter()
        .map(|(space, _)| (space, Page::from_space(&document, space).unwrap()))
        .collect()
}

/// Table entries per object group written past `from`, as `(entries, highest index + 1)`.
fn tables(bytes: &[u8], from: usize) -> Vec<(usize, u32)> {
    let store = Store::parse(bytes).unwrap();
    store
        .lists
        .values()
        .filter(|list| list.nodes.first().is_some_and(|node| node.offset >= from))
        .map(|list| {
            let entries: Vec<u32> = list
                .nodes
                .iter()
                .filter(|node| node.id == 0x24)
                .map(|node| u32::from_le_bytes(node.payload[..4].try_into().unwrap()))
                .collect();
            (entries.len(), entries.iter().max().map_or(0, |max| max + 1))
        })
        .filter(|(entries, _)| *entries > 0)
        .collect()
}

/// `ONESTORE_TABLE_EXPORT` names a directory receiving the edited notebook for a cold reopen.
#[test]
fn a_text_edit_stores_the_table_entries_it_names() {
    let mut bytes = SOURCE.to_vec();
    let mut sparse = false;
    let mut edited = Vec::new();
    for (space, mut page) in pages(SOURCE) {
        let Some(text) = page
            .objects
            .iter_mut()
            .filter_map(|object| match object {
                PageObject::Outline(outline) if !outline.title => Some(&mut outline.paragraphs),
                _ => None,
            })
            .flatten()
            .find_map(|paragraph| paragraph.text_mut())
        else {
            continue;
        };
        let format = text.text.format_at(0).unwrap().clone();
        text.text
            .append(Paragraph::new(" and a few more words".to_owned(), format))
            .unwrap();
        let written = PreparedEdit::page(&bytes, space, &page, "Rust")
            .unwrap()
            .as_bytes()
            .to_vec();
        for (entries, span) in tables(&written, bytes.len()) {
            sparse |= (entries as u32) < span;
        }
        assert_eq!(
            PreparedEdit::page(&written, space, &page, "Rust")
                .unwrap()
                .as_bytes(),
            written
        );
        bytes = written;
        edited.push((space, page));
    }
    // The fixture's pages were written over several sessions, so a text edit names a
    // subset of its revision's table and the stored indices have gaps.
    assert!(sparse);
    let stored = pages(&bytes);
    for (space, page) in &edited {
        let (_, stored) = stored.iter().find(|(id, _)| id == space).unwrap();
        assert!(stored.objects == page.objects);
    }
    if let Some(export) = std::env::var_os("ONESTORE_TABLE_EXPORT") {
        let export = std::path::Path::new(&export);
        std::fs::create_dir_all(export).unwrap();
        std::fs::write(export.join("synthetic.one"), &bytes).unwrap();
        std::fs::copy(
            "../../corpus/native/20260905-05/notebook/Open Notebook.onetoc2",
            export.join("Open Notebook.onetoc2"),
        )
        .unwrap();
    }
}
