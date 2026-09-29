//! The search engine against OneNote 2010's own results on the sample notebook
//! (`corpus/search`), and its folding, word starts, ranking and snippets.

use canvas::{
    document::TextDocument,
    editor::CanvasEditor,
    layout::TextEngine,
    search::{Entry, Index, Query, fold, page_matches, paragraph_match},
};
use onestore::{
    ExGuid, RevisionIndex, Store,
    document::{Document, Format},
    page::{Page, text::Paragraph},
};
use std::path::Path;

/// Each page of the corpus notebook's section `name`.
fn section(name: &str) -> Vec<(ExGuid, Page)> {
    let file = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus/search/notebook")
        .join(format!("{name}.one"));
    let bytes = std::fs::read(file).unwrap();
    let store = Store::parse(&bytes).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    document
        .pages()
        .unwrap()
        .into_iter()
        .map(|(space, id)| {
            let page = Page::from_revision(document.active(space).unwrap(), id).unwrap();
            (space, page)
        })
        .collect()
}

fn notebook() -> Index {
    let mut index = Index::default();
    for name in ["Garden", "Home", "Journal", "Kitchen", "Reading", "Trips"] {
        for (space, page) in section(name) {
            index.set(Entry::new(name, space, &page, 0));
        }
    }
    index
}

fn titles(found: &[canvas::search::Found], in_title: bool) -> Vec<&str> {
    let mut titles: Vec<&str> = found
        .iter()
        .filter(|found| found.in_title == in_title)
        .map(|found| found.title.as_str())
        .collect();
    titles.sort();
    titles
}

#[test]
fn a_notebook_search_finds_what_onenote_finds() {
    let index = notebook();
    let found = index.search(&Query::new("tom"), |_| true);
    assert_eq!(titles(&found, true), ["Tomatoes"]);
    assert_eq!(
        titles(&found, false),
        [
            "Harvest log 2025",
            "Seed inventory",
            "Spring planting plan",
            "Weeknight dal"
        ]
    );
    assert!(found[0].in_title, "title hits come first");
    let found = index.search(&Query::new("sungold tom"), |_| true);
    assert_eq!(titles(&found, true), Vec::<&str>::new());
    assert_eq!(
        titles(&found, false),
        [
            "Harvest log 2025",
            "Seed inventory",
            "Spring planting plan",
            "Tomatoes"
        ]
    );
    assert!(index.search(&Query::new("zzq"), |_| true).is_empty());
    let garden = index.search(&Query::new("tom"), |section| section == "Kitchen");
    assert_eq!(titles(&garden, false), ["Weeknight dal"]);
}

#[test]
fn snippets_show_the_first_match_marked() {
    let index = notebook();
    let found = index.search(&Query::new("lentils"), |_| true);
    let dal = found
        .iter()
        .find(|found| found.title == "Weeknight dal")
        .unwrap();
    assert!(dal.snippet.contains("lentils"), "{:?}", dal.snippet);
    let [hit] = dal.snippet_hits.as_slice() else {
        panic!("{:?}", dal.snippet_hits)
    };
    assert_eq!(dal.snippet[hit.clone()].to_lowercase(), "lentils");
    assert!(!dal.in_title && dal.title_hits.is_empty());
}

#[test]
fn find_on_page_counts_what_onenote_counts() {
    let (_, page) = section("Garden")
        .into_iter()
        .find(|(_, page)| page.title == "Spring planting plan")
        .unwrap();
    let mut engine = TextEngine::default();
    let editor = CanvasEditor::from_page(page, &mut engine).unwrap();
    let matches = page_matches(&editor, &Query::new("bed"));
    assert_eq!(matches.len(), 6, "OneNote shows Match 1 of 6");
    for (outline, selection) in &matches {
        let outline = editor
            .outlines()
            .iter()
            .find(|candidate| candidate.id == *outline)
            .unwrap();
        let [start, end] = selection.positions;
        let paragraph = outline
            .document()
            .paragraphs()
            .nth(start.paragraph)
            .unwrap();
        let text: String = paragraph
            .text()
            .encode_utf16()
            .skip(start.offset as usize)
            .take((end.offset - start.offset) as usize)
            .map(|unit| char::from_u32(u32::from(unit)).unwrap())
            .collect();
        assert_eq!(text.to_lowercase(), "bed");
    }
    assert!(page_matches(&editor, &Query::new("zzq")).is_empty());
}

#[test]
fn folding_ignores_case_and_diacritics() {
    assert_eq!(fold("Crème BRÛLÉE"), "creme brulee");
    assert_eq!(fold("Straße Ærø don’t"), "strasse aero don't");
    assert_eq!(fold("a\u{a0}b\u{b}c\nd"), "a b c\nd");
}

/// A page titled `title` holding one paragraph of `text`.
fn page(engine: &mut TextEngine, title: &str, text: &str) -> Page {
    let document =
        TextDocument::new(vec![Paragraph::new(text.to_owned(), Format::default())]).unwrap();
    let mut page = CanvasEditor::new(engine, document, 480.0)
        .unwrap()
        .page()
        .unwrap();
    page.title = title.to_owned();
    page
}

#[test]
fn words_match_word_starts_and_pages_rank_by_title_then_recency() {
    let mut engine = TextEngine::default();
    let mut index = Index::default();
    let space = |n: u32| ExGuid {
        guid: [n as u8; 16],
        n,
    };
    index.set(Entry::new(
        "a",
        space(1),
        &page(&mut engine, "Notes", "café au lait at the bottom"),
        30,
    ));
    index.set(Entry::new(
        "a",
        space(2),
        &page(&mut engine, "Café list", "nothing else"),
        10,
    ));
    index.set(Entry::new(
        "b",
        space(3),
        &page(&mut engine, "Old", "CAFE menu"),
        20,
    ));
    index.set(Entry::new(
        "b",
        space(4),
        &page(&mut engine, "東京", "今日は東京に行く"),
        5,
    ));
    let order = |index: &Index, query: &str| -> Vec<String> {
        index
            .search(&Query::new(query), |_| true)
            .into_iter()
            .map(|found| found.title)
            .collect()
    };
    assert_eq!(order(&index, "cafe"), ["Café list", "Notes", "Old"]);
    assert_eq!(
        order(&index, "tom"),
        Vec::<String>::new(),
        "only word starts match"
    );
    assert_eq!(order(&index, "\"au lait\""), ["Notes"]);
    assert_eq!(order(&index, "\"lait au\""), Vec::<String>::new());
    assert_eq!(order(&index, "京に"), ["東京"]);
    // Replacing a page, then dropping a section.
    index.set(Entry::new(
        "a",
        space(1),
        &page(&mut engine, "Notes", "tea"),
        40,
    ));
    assert_eq!(order(&index, "cafe"), ["Café list", "Old"]);
    index.retain(|entry| entry.section != "b");
    assert_eq!(order(&index, "cafe"), ["Café list"]);
    assert_eq!(index.len(), 2);
}

/// OneNote's tag gallery (`corpus/structural-probe`): a paragraph under each default tag,
/// listed once for each tag with its stored name, and selected whole from the summary.
#[test]
fn tags_summary_lists_each_tagged_paragraph() {
    let file =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/structural-probe/tag-gallery.one");
    let bytes = std::fs::read(file).unwrap();
    let store = Store::parse(&bytes).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let (space, _) = document.pages().unwrap()[0];
    let page = Page::from_space(&document, space).unwrap();
    let mut index = Index::default();
    index.set(Entry::new("gallery", space, &page, 7));
    let tagged = index.tagged(|_| true);
    let mut names: Vec<&str> = tagged.iter().map(|tagged| tagged.name.as_str()).collect();
    names.sort_unstable();
    // The probe's T24 and T25 also carry Call back, one entry under each tag.
    assert_eq!(tagged.len(), names.len());
    names.dedup();
    let defaults = canvas::editor::NoteTag::defaults();
    let mut labels: Vec<&str> = defaults.iter().map(|tag| tag.label.as_str()).collect();
    labels.sort_unstable();
    assert_eq!(names, labels);
    assert!(tagged.iter().all(|tagged| tagged.space == space));
    assert!(index.tagged(|entry| entry.section != "gallery").is_empty());
    let mut engine = TextEngine::default();
    let editor = CanvasEditor::from_page(page, &mut engine).unwrap();
    let (outline, selection) = paragraph_match(&editor, tagged[0].paragraph).unwrap();
    let outline = editor
        .outlines()
        .iter()
        .find(|candidate| candidate.id == outline)
        .unwrap();
    let [start, end] = selection.positions;
    let text = outline
        .document()
        .paragraphs()
        .nth(start.paragraph)
        .unwrap();
    assert_eq!(start.offset, 0);
    assert_eq!(end.offset as usize, text.text().encode_utf16().count());
    assert_eq!(canvas::search::shown(text), tagged[0].text);
}
