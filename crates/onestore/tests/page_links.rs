use onestore::{
    ExGuid, PreparedEdit, RevisionIndex, Store,
    document::Document,
    page::{Page, PageObject, PageParagraph, Paragraph, text::Edit},
};

const OUTLINES: &[u8] =
    include_bytes!("../../../corpus/outline-edit/before/notebook/synthetic.one");
const AUTHOR: &str = "Link author";
const CODE: &str = "\u{fddf}HYPERLINK \"https://example.invalid/rust\"";

fn page_by_title(bytes: &[u8], title: &str) -> (ExGuid, Page) {
    let store = Store::parse(bytes).unwrap();
    assert!(store.checksum_mismatches.is_empty());
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    document
        .pages()
        .unwrap()
        .into_iter()
        .find_map(|(space, _)| {
            let page = Page::from_space(&document, space).unwrap();
            (page.title == title).then_some((space, page))
        })
        .unwrap()
}

fn page_in(bytes: &[u8], space: ExGuid) -> Page {
    let store = Store::parse(bytes).unwrap();
    assert!(store.checksum_mismatches.is_empty());
    let index = RevisionIndex::parse(&store).unwrap();
    Page::from_space(&Document::parse(&index).unwrap(), space).unwrap()
}

fn body_paragraphs(page: &mut Page) -> &mut Vec<PageParagraph> {
    page.objects
        .iter_mut()
        .find_map(|object| match object {
            PageObject::Outline(outline) if !outline.title => Some(&mut outline.paragraphs),
            _ => None,
        })
        .unwrap()
}

/// Language tags stay as stored, so the comparison leaves them out.
fn without_language(mut page: Page) -> Page {
    for object in &mut page.objects {
        if let PageObject::Outline(outline) = object {
            for paragraph in &mut outline.paragraphs {
                if let Some(text) = paragraph.text_mut() {
                    let mut start = 0;
                    text.text = Paragraph::from_runs(text.text.spans().iter().map(|span| {
                        let value = text.text.text()[start..span.end].to_owned();
                        start = span.end;
                        let mut format = span.format.clone();
                        format.language = None;
                        (value, format)
                    }));
                }
            }
        }
    }
    page
}

fn assert_same(written: &[u8], space: ExGuid, expected: &Page) -> Page {
    let stored = page_in(written, space);
    let mut expected = expected.clone();
    expected.title = stored.title.clone();
    assert_eq!(without_language(stored.clone()), without_language(expected));
    stored
}

fn linked(page: &mut Page) -> usize {
    body_paragraphs(page)
        .iter()
        .position(|p| {
            p.text()
                .is_some_and(|t| t.text.text().contains("HYPERLINK"))
        })
        .unwrap()
}

/// Appends a hyperlink: a hidden field code run followed by the visible label, both
/// flagged as link runs like OneNote writes them.
fn append_link(text: &mut Paragraph, label: &str) {
    let end = text.utf16_offset(text.text().len()).unwrap();
    let base = text.format_at(end).unwrap().clone();
    let mut code = base.clone();
    code.hyperlink = Some(true);
    code.hyperlink_label = Some(true);
    code.hidden = Some(true);
    let mut visible = base.clone();
    visible.hyperlink = Some(true);
    visible.hyperlink_label = Some(true);
    text.apply(Edit {
        range: end..end,
        replacement: Paragraph::from_runs([
            (" ".to_owned(), base),
            (CODE.to_owned(), code),
            (label.to_owned(), visible),
        ]),
    })
    .unwrap();
}

#[test]
fn a_native_link_is_removed_leaving_plain_label_text() {
    let (space, before) = page_by_title(OUTLINES, "Move leaf down");
    let mut after = before.clone();
    let at = linked(&mut after);
    let text = &mut body_paragraphs(&mut after)[at].text_mut().unwrap().text;
    let content = text.text().to_owned();
    let start = content.find('\u{fddf}').unwrap();
    let end = content.find("Link label").unwrap();
    let mut plain = text.format_at(0).unwrap().clone();
    plain.hyperlink = Some(false);
    plain.hyperlink_label = Some(false);
    plain.hidden = Some(false);
    let (from, to) = (
        text.utf16_offset(start).unwrap(),
        text.utf16_offset(content.len()).unwrap(),
    );
    text.apply(Edit {
        range: from..to,
        replacement: Paragraph::new(content[end..].to_owned(), plain),
    })
    .unwrap();
    let written = PreparedEdit::page(OUTLINES, space, &after, AUTHOR).unwrap();
    let stored = assert_same(written.as_bytes(), space, &after);
    let mut stored = stored;
    let text = &body_paragraphs(&mut stored)[at].text().unwrap().text;
    assert!(text.text().ends_with(" Link label"));
    assert!(
        text.spans()
            .iter()
            .all(|span| span.format.hyperlink != Some(true) && span.format.hidden != Some(true))
    );
}

/// `ONESTORE_LINK_EXPORT` names a new directory receiving the candidate for a cold reopen.
#[test]
fn a_link_is_added_to_a_fresh_page_and_reads_back() {
    let source = onestore::create_section("links.one", "Read about Rust", "Author").unwrap();
    let store = Store::parse(&source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let (space, _) = document.pages().unwrap()[0];
    let before = Page::from_space(&document, space).unwrap();
    let mut after = before.clone();
    append_link(
        &mut body_paragraphs(&mut after)[0].text_mut().unwrap().text,
        "the Rust site",
    );
    let written = PreparedEdit::page(&source, space, &after, AUTHOR).unwrap();
    let stored = assert_same(written.as_bytes(), space, &after);
    let mut stored = stored;
    let text = &body_paragraphs(&mut stored)[0].text().unwrap().text;
    let flags: Vec<(Option<bool>, Option<bool>, Option<bool>)> = text
        .spans()
        .iter()
        .map(|s| {
            (
                s.format.hyperlink,
                s.format.hyperlink_label,
                s.format.hidden,
            )
        })
        .collect();
    assert_eq!(
        flags,
        [
            (None, None, None),
            (Some(true), Some(true), Some(true)),
            (Some(true), Some(true), None),
        ]
    );
    if let Some(directory) = std::env::var_os("ONESTORE_LINK_EXPORT") {
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(directory.join("links.one"), written.as_bytes()).unwrap();
        let written_store = Store::parse(written.as_bytes()).unwrap();
        std::fs::write(
            directory.join("Open Notebook.onetoc2"),
            onestore::create_table_of_contents(
                "Open Notebook.onetoc2",
                &[("links.one", written_store.header.file_id)],
            )
            .unwrap(),
        )
        .unwrap();
    }
}
