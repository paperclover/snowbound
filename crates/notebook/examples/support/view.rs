#![allow(dead_code)]

use onestore::{
    ExGuid, RevisionIndex, Store,
    document::{Document, Kind},
};

pub struct View {
    pub space: ExGuid,
    pub object: ExGuid,
    pub revision: ExGuid,
    pub text: String,
}

pub fn view(bytes: &[u8]) -> Result<View, Box<dyn std::error::Error>> {
    let store = Store::parse(bytes)?;
    if !store.checksum_mismatches.is_empty() {
        return Err("Transaction checksum damage".into());
    }
    let index = RevisionIndex::parse(&store)?;
    index.validate_current()?;
    let document = Document::parse(&index)?;
    let mut found = Vec::new();
    for (sid, page) in document.pages()? {
        let space = &document.spaces[&sid];
        let rid = space.contexts[&ExGuid::default()];
        let revision = &space.revisions[&rid];
        let mut pending = vec![page];
        let mut seen = std::collections::BTreeSet::new();
        while let Some(oid) = pending.pop() {
            if !seen.insert(oid) {
                continue;
            }
            let node = &revision.nodes[&oid];
            pending.extend(
                node.children
                    .iter()
                    .chain(&node.content)
                    .chain(&node.structure)
                    .copied(),
            );
            if let Kind::RichText { text, .. } = &node.kind
                && text.starts_with("Concurrent edits:")
            {
                revision.text_runs(oid)?;
                found.push(View {
                    space: sid,
                    object: oid,
                    revision: rid,
                    text: text.clone(),
                });
            }
        }
    }
    if found.len() != 1 {
        return Err("Expected one concurrent-edit paragraph".into());
    }
    Ok(found.pop().unwrap())
}

/// The concurrent-edit paragraph of `page`: its text object and text.
fn in_page(page: &onestore::page::Page) -> Option<(ExGuid, String)> {
    let mut pending: Vec<&onestore::page::PageParagraph> = page
        .objects
        .iter()
        .flat_map(|object| match object {
            onestore::page::PageObject::Outline(outline) => outline.paragraphs.iter().collect(),
            _ => Vec::new(),
        })
        .collect();
    pending.reverse();
    while let Some(paragraph) = pending.pop() {
        if let Some(text) = paragraph.text()
            && text.text.text().starts_with("Concurrent edits:")
        {
            return Some((text.id, text.text.text().to_owned()));
        }
        if let onestore::page::ParagraphContent::Table(table) = &paragraph.content {
            pending.extend(
                table
                    .rows
                    .iter()
                    .flat_map(|row| &row.cells)
                    .flat_map(|cell| &cell.paragraphs)
                    .rev(),
            );
        }
    }
    None
}

/// The concurrent-edit paragraph as the queued edits leave it: its page, text object and
/// text.
pub fn cached(
    cache: &notebook::Replica,
) -> Result<(ExGuid, ExGuid, String), Box<dyn std::error::Error>> {
    let mut found = Vec::new();
    for (space, ..) in cache.pages()? {
        if let Some((object, text)) = in_page(&cache.page(space)?) {
            found.push((space, object, text));
        }
    }
    match <[_; 1]>::try_from(found) {
        Ok([found]) => Ok(found),
        Err(_) => Err("Expected one concurrent-edit paragraph".into()),
    }
}
