//! Conflict pages as OneNote shows them.

use onestore::{
    ExGuid,
    page::{Page, PageObject, PageParagraph, ParagraphContent},
};

/// OneNote's highlight for conflicting changes on a conflict page, COLORREF.
pub const CONFLICTING: u32 = 0xd6d6ff;

/// `page` with its conflict `objects` marked as OneNote shows a conflict page's: a band
/// across each conflicting paragraph.
pub fn highlighted(mut page: Page, objects: &[ExGuid]) -> Page {
    fn mark(paragraphs: &mut [PageParagraph], objects: &[ExGuid]) {
        for paragraph in paragraphs {
            match &mut paragraph.content {
                ParagraphContent::Table(table) => {
                    for cell in table.rows.iter_mut().flat_map(|row| &mut row.cells) {
                        mark(&mut cell.paragraphs, objects);
                    }
                }
                ParagraphContent::Text(text)
                    if objects.contains(&paragraph.id) || objects.contains(&text.id) =>
                {
                    paragraph.format.highlight = Some(CONFLICTING);
                }
                _ => {}
            }
        }
    }
    for object in &mut page.objects {
        match object {
            PageObject::Outline(outline) => mark(&mut outline.paragraphs, objects),
            PageObject::Title(title) => {
                for outline in &mut title.outlines {
                    mark(&mut outline.paragraphs, objects);
                }
            }
            _ => {}
        }
    }
    page
}
