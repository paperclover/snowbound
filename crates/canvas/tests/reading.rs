//! The reading view over OneNote-written pages: what it offers, the order it reads, and that
//! the reflowed page keeps every paragraph and object while only the shown copy moves.

use canvas::{
    layout::TextEngine,
    reading::{self, Kept, Kind, Refusal, Verdict},
};
use onestore::{
    Arena, ExGuid, Section,
    page::{Page, PageObject},
};
use std::collections::BTreeSet;

const COLUMN: f32 = 230.0;

fn first_page(bytes: &[u8]) -> Page {
    let arena = Arena::default();
    let mut section = Section::open(&arena, bytes.to_vec()).unwrap();
    let space = section.pages().unwrap()[0].0;
    section.page(space).unwrap()
}

fn paragraphs(page: &Page) -> BTreeSet<ExGuid> {
    page.objects
        .iter()
        .filter_map(|object| match object {
            PageObject::Outline(outline) => Some(outline.paragraphs.iter().map(|p| p.id)),
            _ => None,
        })
        .flatten()
        .collect()
}

#[test]
fn a_planning_page_reads_down_one_column() {
    let page = first_page(include_bytes!(
        "../../../corpus/notebook-management/native/page-color/Garden.one"
    ));
    let mut engine = TextEngine::default();
    let read = reading::read(&page, COLUMN, &mut engine).unwrap();
    assert_eq!(
        read.verdict,
        Verdict::Partial(vec![Kept::Asides, Kept::SideBySide, Kept::Wide])
    );
    let reflowed = read.page.unwrap();
    // Splitting around asides drops no paragraph; only spacing blank runs could go.
    assert_eq!(paragraphs(&reflowed), paragraphs(&page));
    // Every block starts at the column's left edge or an aside's indent, top to bottom.
    let mut tops = Vec::new();
    for block in &read.blocks {
        let object = reflowed
            .objects
            .iter()
            .find(|object| object.id() == block.members[0])
            .unwrap();
        let layout = object.layout();
        if block.kind == Kind::Text {
            assert!(layout.max_width.unwrap() <= COLUMN);
        }
        tops.push(layout.y.unwrap());
    }
    assert!(tops.windows(2).all(|pair| pair[0] < pair[1]), "{tops:?}");
    // The page itself is untouched.
    let again = first_page(include_bytes!(
        "../../../corpus/notebook-management/native/page-color/Garden.one"
    ));
    assert_eq!(
        serde_json::to_string(&page).unwrap(),
        serde_json::to_string(&again).unwrap()
    );
}

#[test]
fn a_drawing_is_not_reflowed() {
    let page = first_page(include_bytes!(
        "../../../corpus/ink-edit/drawing/candidate/ink.one"
    ));
    let read = reading::read(&page, COLUMN, &mut TextEngine::default()).unwrap();
    assert_eq!(read.verdict, Verdict::Refused(Refusal::Drawn));
    assert!(read.page.is_none());
}

#[test]
fn a_page_as_narrow_as_the_column_shows_as_laid_out() {
    let page = first_page(include_bytes!(
        "../../../corpus/append/round-01/complex/notebook/synthetic.one"
    ));
    let read = reading::read(&page, 2000.0, &mut TextEngine::default()).unwrap();
    assert_eq!(read.verdict, Verdict::Fits);
}
