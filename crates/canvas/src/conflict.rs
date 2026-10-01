//! Conflict pages and page versions as OneNote shows them.

use crate::document::descendants;
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
        page.objects
            .iter()
            .flat_map(|object| match object {
                PageObject::Outline(outline) => std::slice::from_ref(outline),
                PageObject::Title(title) => title.outlines.as_slice(),
                _ => &[],
            })
            .flat_map(|outline| descendants(&outline.paragraphs, None))
            .filter_map(|(_, _, node)| match &node.content {
                ParagraphContent::Text(text) => Some((text.id, text.text.text().to_owned())),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let before = older.map(texts).unwrap_or_default();
    let changed: Vec<ExGuid> = texts(&version)
        .into_iter()
        .filter(|text| !before.contains(text))
        .map(|(id, _)| id)
        .collect();
    banded(version, &changed, CHANGED)
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
