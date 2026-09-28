#[path = "support/ops.rs"]
mod ops;
use onestore::{
    ExGuid, RevisionIndex, Store,
    document::Document,
    page::{
        Page, PageObject, PageParagraph, Paragraph,
        link::{InternalLink, LinkTarget, internal_link, parse_internal_link},
        text::Edit,
    },
};

const OUTLINES: &[u8] =
    include_bytes!("../../../corpus/outline-edit/before/notebook/synthetic.one");
const NATIVE_LINKS: &[u8] =
    include_bytes!("../../../corpus/link-edit/native-links/notebook/links.one");
const NATIVE_BASE_PATH: &str = r"C:\one-tests\runs\capture\notebook\links.one";
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
    let written = ops::saved(OUTLINES, space, &after).unwrap();
    let stored = assert_same(written.as_slice(), space, &after);
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
    let written = ops::saved(&source, space, &after).unwrap();
    let stored = assert_same(written.as_slice(), space, &after);
    // As OneNote titles `NATIVE_LINKS`'s page holding this paragraph: without the field code.
    assert_eq!(stored.title, "Read about Rust the Rust site");
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
        std::fs::write(directory.join("links.one"), written.as_slice()).unwrap();
        let written_store = Store::parse(written.as_slice()).unwrap();
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

fn field_codes(page: &mut Page) -> Vec<String> {
    body_paragraphs(page)
        .iter()
        .filter_map(|p| p.text())
        .filter_map(|t| {
            let text = t.text.text();
            let start = text.find("\u{fddf}HYPERLINK \"")? + "\u{fddf}HYPERLINK \"".len();
            let end = start + text[start..].find('"')?;
            Some(text[start..end].to_owned())
        })
        .collect()
}

/// The URLs OneNote 2010 wrote for links to a page, a paragraph and the section
/// (`corpus/link-edit/native-links`) are what `internal_link` produces from the stored
/// identities: the section file identity, the target page's notebook-management identity
/// and the paragraph's stored identity.
#[test]
fn internal_links_match_what_onenote_stores() {
    let section = Store::parse(NATIVE_LINKS).unwrap().header.file_id;
    let (_, target) = page_by_title(NATIVE_LINKS, "Link target");
    let mut target_paragraphs = target.clone();
    // OneNote rewrote the target outline after linking; the link keeps the paragraph
    // identity it saw then (n 28), whose page half is the current paragraph's.
    let paragraph = ExGuid {
        guid: body_paragraphs(&mut target_paragraphs)[0].id.guid,
        n: 28,
    };
    let (_, mut source) = page_by_title(NATIVE_LINKS, "Read about Rust the Rust site");
    let stored = field_codes(&mut source);
    let page = LinkTarget::Page {
        identity: target.identity.unwrap(),
        title: &target.title,
    };
    let object = LinkTarget::Object {
        identity: target.identity.unwrap(),
        title: &target.title,
        object: paragraph,
    };
    assert_eq!(
        stored,
        [
            "https://example.invalid/rust".to_owned(),
            internal_link(section, NATIVE_BASE_PATH, page),
            internal_link(section, NATIVE_BASE_PATH, object),
            internal_link(section, NATIVE_BASE_PATH, LinkTarget::Section),
        ]
    );
    assert_eq!(
        internal_link(
            [0; 16],
            "p",
            LinkTarget::Page {
                identity: [0; 16],
                title: "A b/c"
            }
        ),
        "onenote:#A%20b%2Fc&section-id={00000000-0000-0000-0000-000000000000}&page-id={00000000-0000-0000-0000-000000000000}&end&base-path=p"
    );
    let identity = target.identity.unwrap();
    assert_eq!(
        stored[1..]
            .iter()
            .map(|url| parse_internal_link(url))
            .collect::<Vec<_>>(),
        [
            Some(InternalLink {
                section,
                page: Some(identity),
                object: None
            }),
            Some(InternalLink {
                section,
                page: Some(identity),
                object: Some(paragraph)
            }),
            Some(InternalLink {
                section,
                page: None,
                object: None
            }),
        ]
    );
    assert_eq!(parse_internal_link(&stored[0]), None);
    assert_eq!(parse_internal_link("onenote:#x&page-id={0}"), None);
    // The Link dialog's page and Copy Link to Page name the section file before the `#`.
    let ids = "section-id={0F1EFC25-DF7C-45CB-87A4-BDD03F42057E}&page-id={A9CCA318-A43E-4912-A1C7-F19F1EAF30CA}";
    for url in [
        format!("onenote:links.one#Page&{ids}&base-path=//C:/notebook"),
        format!("onenote:///C:\\notebook\\links.one#Page&{ids}&end"),
    ] {
        let link = parse_internal_link(&url).unwrap();
        assert_eq!(
            (
                link.section[..4].to_vec(),
                link.page.map(|page| page[..4].to_vec())
            ),
            (
                vec![0x25, 0xfc, 0x1e, 0x0f],
                Some(vec![0x18, 0xa3, 0xcc, 0xa9])
            )
        );
    }
}

/// `ONESTORE_INTERNAL_LINK_EXPORT` names a new directory receiving the candidate for a cold
/// reopen: a section whose first page links to its second page and to a paragraph on it.
#[test]
fn a_page_links_to_another_page_and_its_paragraph() {
    let source = onestore::create_section("links.one", "Linking page", "Author").unwrap();
    let creation = onestore::PageCreation::new(None, Some("Link target"), "Author").unwrap();
    let with_target = ops::edited(
        &source,
        vec![onestore::op::Op::Section(onestore::op::SectionOp::Create(
            creation,
        ))],
    )
    .unwrap();
    let section = Store::parse(&with_target).unwrap().header.file_id;
    let (_, target) = page_by_title(&with_target, "Link target");
    let (space, before) = page_by_title(&with_target, "Linking page");
    let paragraph = target
        .objects
        .iter()
        .find_map(|object| match object {
            PageObject::Outline(outline) if !outline.title => outline.paragraphs.first(),
            _ => None,
        })
        .map(|p| p.id);
    let base_path = r"C:\one-tests\runs\capture\notebook\links.one";
    let page_url = internal_link(
        section,
        base_path,
        LinkTarget::Page {
            identity: target.identity.unwrap(),
            title: &target.title,
        },
    );
    let mut after = before.clone();
    let text = &mut body_paragraphs(&mut after)[0].text_mut().unwrap().text;
    let end = text.utf16_offset(text.text().len()).unwrap();
    let base = text.format_at(end).unwrap().clone();
    let mut code = base.clone();
    code.hyperlink = Some(true);
    code.hyperlink_label = Some(true);
    code.hidden = Some(true);
    let mut visible = base.clone();
    visible.hyperlink = Some(true);
    visible.hyperlink_label = Some(true);
    let mut runs = vec![
        (" ".to_owned(), base.clone()),
        (format!("\u{fddf}HYPERLINK \"{page_url}\""), code.clone()),
        ("Link target".to_owned(), visible.clone()),
    ];
    if let Some(paragraph) = paragraph {
        let object_url = internal_link(
            section,
            base_path,
            LinkTarget::Object {
                identity: target.identity.unwrap(),
                title: &target.title,
                object: paragraph,
            },
        );
        runs.push((" ".to_owned(), base.clone()));
        runs.push((format!("\u{fddf}HYPERLINK \"{object_url}\""), code));
        runs.push(("its paragraph".to_owned(), visible));
    }
    text.apply(Edit {
        range: end..end,
        replacement: Paragraph::from_runs(runs),
    })
    .unwrap();
    let written = ops::saved(&with_target, space, &after).unwrap();
    let stored = assert_same(written.as_slice(), space, &after);
    // Titles show the link's label, not its field code, as OneNote stores them.
    assert!(
        stored.title.starts_with("Linking page Link target"),
        "{}",
        stored.title
    );
    if let Some(directory) = std::env::var_os("ONESTORE_INTERNAL_LINK_EXPORT") {
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(directory.join("links.one"), written.as_slice()).unwrap();
        std::fs::write(
            directory.join("Open Notebook.onetoc2"),
            onestore::create_table_of_contents("Open Notebook.onetoc2", &[("links.one", section)])
                .unwrap(),
        )
        .unwrap();
    }
}

/// A title stored with a link's field code, as an earlier writer left it
/// (`corpus/link-edit/internal/candidate`), reads as the label the page shows.
#[test]
fn titles_read_without_link_field_codes() {
    let bytes = include_bytes!("../../../corpus/link-edit/internal/candidate/links.one").to_vec();
    let arena = onestore::Arena::default();
    let mut section = onestore::Section::open(&arena, bytes.clone()).unwrap();
    let titles: Vec<String> = section
        .pages()
        .unwrap()
        .into_iter()
        .map(|(_, title, _)| title)
        .collect();
    assert_eq!(titles, ["Linking page Link target", "Link target"]);
    page_by_title(&bytes, "Linking page Link target");
}
