//! Conflict pages and page versions as OneNote shows them.

use onestore::{
    ExGuid,
    page::{Page, PageObject, PageParagraph, ParagraphContent},
};

/// OneNote's highlight for conflicting changes on a conflict page, COLORREF.
pub const CONFLICTING: u32 = 0xd6d6ff;

/// OneNote's highlight for what a page version changed since the version before it, COLORREF.
pub const CHANGED: u32 = 0xd6ffd6;

/// `page` with its conflict `objects` marked as OneNote shows a conflict page's: a band
/// across each conflicting paragraph.
pub fn highlighted(page: Page, objects: &[ExGuid]) -> Page {
    banded(page, objects, CONFLICTING)
}

/// `version`, a page version, with the text it changed since `older`, the version before it,
/// banded as OneNote shows it; all of it without one.
pub fn changes(version: Page, older: Option<&Page>) -> Page {
    let texts = |page: &Page| {
        let mut texts = Vec::new();
        each_text(page, &mut |id, text| texts.push((id, text.to_owned())));
        texts
    };
    let before = older.map(texts).unwrap_or_default();
    let changed: Vec<ExGuid> = texts(&version)
        .into_iter()
        .filter(|text| !before.contains(text))
        .map(|(id, _)| id)
        .collect();
    banded(version, &changed, CHANGED)
}

fn each_text(page: &Page, f: &mut impl FnMut(ExGuid, &str)) {
    fn walk(paragraphs: &[PageParagraph], f: &mut impl FnMut(ExGuid, &str)) {
        for paragraph in paragraphs {
            match &paragraph.content {
                ParagraphContent::Table(table) => {
                    for cell in table.rows.iter().flat_map(|row| &row.cells) {
                        walk(&cell.paragraphs, f);
                    }
                }
                ParagraphContent::Text(text) => f(text.id, text.text.text()),
                _ => {}
            }
        }
    }
    for object in &page.objects {
        match object {
            PageObject::Outline(outline) => walk(&outline.paragraphs, f),
            PageObject::Title(title) => {
                for outline in &title.outlines {
                    walk(&outline.paragraphs, f);
                }
            }
            _ => {}
        }
    }
}

fn banded(mut page: Page, objects: &[ExGuid], color: u32) -> Page {
    fn mark(paragraphs: &mut [PageParagraph], objects: &[ExGuid], color: u32) {
        for paragraph in paragraphs {
            match &mut paragraph.content {
                ParagraphContent::Table(table) => {
                    for cell in table.rows.iter_mut().flat_map(|row| &mut row.cells) {
                        mark(&mut cell.paragraphs, objects, color);
                    }
                }
                ParagraphContent::Text(text)
                    if objects.contains(&paragraph.id) || objects.contains(&text.id) =>
                {
                    paragraph.format.highlight = Some(color);
                }
                _ => {}
            }
        }
    }
    for object in &mut page.objects {
        match object {
            PageObject::Outline(outline) => mark(&mut outline.paragraphs, objects, color),
            PageObject::Title(title) => {
                for outline in &mut title.outlines {
                    mark(&mut outline.paragraphs, objects, color);
                }
            }
            _ => {}
        }
    }
    page
}
