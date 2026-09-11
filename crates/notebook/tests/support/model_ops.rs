//! Expresses the editor's actions as page-model edits saved through the replica, so tests
//! can state intents in terms of paragraphs and ranges.
#![allow(dead_code)]

use notebook::Replica;
use onestore::{
    ExGuid, RevisionIndex, Store,
    document::{Document, Format},
    page::{
        Outline, Page, PageObject, PageParagraph, Paragraph, ParagraphContent, TextObject,
        text::{Edit, new_id},
    },
};
use std::ops::Range;

pub const AUTHOR: &str = "Offline author";

pub fn page_of(bytes: &[u8], space: ExGuid) -> Page {
    let store = Store::parse(bytes).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    Page::from_space(&document, space).unwrap()
}

/// The page space and page model containing the text object.
pub fn locate(bytes: &[u8], text: ExGuid) -> (ExGuid, Page) {
    let store = Store::parse(bytes).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    document
        .pages()
        .unwrap()
        .into_iter()
        .find_map(|(space, _)| {
            let page = Page::from_space(&document, space).ok()?;
            paragraph_with(&page, text)
                .is_some()
                .then_some((space, page))
        })
        .expect("text object belongs to an active page")
}

pub fn outlines_mut(page: &mut Page) -> Vec<&mut Outline> {
    let mut out = Vec::new();
    for object in &mut page.objects {
        match object {
            PageObject::Outline(outline) => out.push(outline),
            PageObject::Title(title) => out.extend(title.outlines.iter_mut()),
            _ => {}
        }
    }
    out
}

pub fn paragraph_with(page: &Page, text: ExGuid) -> Option<&PageParagraph> {
    page.objects.iter().find_map(|object| {
        let outlines: Vec<&Outline> = match object {
            PageObject::Outline(outline) => vec![outline],
            PageObject::Title(title) => title.outlines.iter().collect(),
            _ => Vec::new(),
        };
        outlines
            .into_iter()
            .flat_map(|o| o.paragraphs.iter())
            .find(|p| p.text().is_some_and(|t| t.id == text))
    })
}

fn text_mut(page: &mut Page, text: ExGuid) -> &mut TextObject {
    outlines_mut(page)
        .into_iter()
        .flat_map(|o| o.paragraphs.iter_mut())
        .find(|p| p.text().is_some_and(|t| t.id == text))
        .and_then(|p| p.text_mut())
        .expect("text object is on the page")
}

/// Replaces a UTF-16 range of a text object in the model, inheriting the format at its start.
pub fn replace_text(page: &mut Page, text: ExGuid, range: Range<u32>, replacement: &str) {
    let target = text_mut(page, text);
    let format = target.text.format_at(range.start).unwrap().clone();
    target
        .text
        .apply(Edit {
            range,
            replacement: Paragraph::new(replacement.into(), format),
        })
        .unwrap();
}

/// Rewrites the formats over a UTF-16 range of a text object.
pub fn restyle(page: &mut Page, text: ExGuid, range: Range<u32>, change: impl Fn(&mut Format)) {
    let target = text_mut(page, text);
    let slice = target.text.slice(range.clone()).unwrap();
    let mut runs = Vec::new();
    let mut from = 0;
    for span in slice.spans() {
        let mut format = span.format.clone();
        change(&mut format);
        runs.push((slice.text()[from..span.end].to_owned(), format));
        from = span.end;
    }
    target
        .text
        .apply(Edit {
            range,
            replacement: Paragraph::from_runs(runs),
        })
        .unwrap();
}

/// A plain new paragraph carrying fresh identities and the template's resolved format.
pub fn fresh_paragraph(template: &PageParagraph, text: &str) -> PageParagraph {
    let mut paragraph = template.clone();
    paragraph.id = new_id().unwrap();
    paragraph.parent = None;
    paragraph.level = 1;
    paragraph.lists.clear();
    paragraph.tags.clear();
    paragraph.style = None;
    paragraph.collapsed = false;
    let format = template.text().unwrap().text.format_at(0).unwrap().clone();
    paragraph.content = ParagraphContent::Text(TextObject {
        id: new_id().unwrap(),
        date_field: None,
        text: Paragraph::new(text.into(), format),
        tags: Vec::new(),
    });
    paragraph
}

/// Inserts a plain paragraph after `after` (or first) in the outline containing it, at the
/// same level and parent, returning the new paragraph and text identities.
pub fn insert_after(page: &mut Page, after: ExGuid, text: &str) -> (ExGuid, ExGuid) {
    for outline in outlines_mut(page) {
        if let Some(at) = outline
            .paragraphs
            .iter()
            .position(|p| p.text().is_some_and(|t| t.id == after))
        {
            let mut fresh = fresh_paragraph(&outline.paragraphs[at], text);
            fresh.parent = outline.paragraphs[at].parent;
            fresh.level = outline.paragraphs[at].level;
            let ids = (fresh.id, fresh.text().unwrap().id);
            // Skip the anchor's descendants so the new paragraph follows its whole subtree.
            let mut end = at + 1;
            while end < outline.paragraphs.len()
                && outline.paragraphs[end].level > outline.paragraphs[at].level
            {
                end += 1;
            }
            outline.paragraphs.insert(end, fresh);
            return ids;
        }
    }
    panic!("anchor text is on the page");
}

/// Adds a body outline with one plain paragraph at the given point coordinates.
pub fn insert_outline(page: &mut Page, x: f32, y: f32, text: &str) -> (ExGuid, ExGuid, ExGuid) {
    let template = page
        .objects
        .iter()
        .find_map(|object| match object {
            PageObject::Outline(outline) => outline
                .paragraphs
                .iter()
                .find(|p| p.text().is_some())
                .cloned(),
            _ => None,
        })
        .expect("a text paragraph to copy formatting from");
    let paragraph = fresh_paragraph(&template, text);
    let ids = (
        new_id().unwrap(),
        paragraph.id,
        paragraph.text().unwrap().id,
    );
    let outline = Outline {
        id: ids.0,
        title: false,
        min_width: None,
        layout: onestore::document::Layout {
            x: Some(x),
            y: Some(y),
            ..Default::default()
        },
        indents: Vec::new(),
        paragraphs: vec![paragraph],
        unsupported: Vec::new(),
    };
    let at = page
        .objects
        .iter()
        .position(|o| matches!(o, PageObject::Title(_)))
        .unwrap_or(page.objects.len());
    page.objects.insert(at, PageObject::Outline(outline));
    ids
}

/// Removes a paragraph and its descendants.
pub fn delete_paragraph(page: &mut Page, text: ExGuid) {
    for outline in outlines_mut(page) {
        if let Some(at) = outline
            .paragraphs
            .iter()
            .position(|p| p.text().is_some_and(|t| t.id == text))
        {
            let level = outline.paragraphs[at].level;
            let mut end = at + 1;
            while end < outline.paragraphs.len() && outline.paragraphs[end].level > level {
                end += 1;
            }
            outline.paragraphs.drain(at..end);
            return;
        }
    }
    panic!("text is on the page");
}

/// Saves `edit` applied to the page containing `text`, using the replica's current image.
pub fn save(
    cache: &Replica,
    text: ExGuid,
    edit: impl FnOnce(&mut Page),
) -> Result<Option<u64>, notebook::Error> {
    let source = cache.snapshot()?;
    let (space, mut page) = locate(&source, text);
    edit(&mut page);
    cache.save(&source, space, &page, AUTHOR)
}

/// Reviews a conflicting save by applying `edit` to the current remote page.
pub fn review(
    cache: &Replica,
    id: u64,
    text: ExGuid,
    edit: impl FnOnce(&mut Page),
) -> Result<(), notebook::Error> {
    let local = cache.snapshot()?;
    let remote = cache.remote_snapshot()?;
    let (_, mut page) = locate(&remote, text);
    edit(&mut page);
    cache.review_page(id, &local, &remote, &page)
}
