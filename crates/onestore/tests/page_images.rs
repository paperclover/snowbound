use onestore::{
    ExGuid, PreparedEdit, RevisionIndex, Store,
    document::{Document, Format},
    page::{Image, Page, PageObject, PageParagraph, ParagraphContent, TextObject, text::new_id},
};
use std::sync::Arc;

const NATIVE: &[u8] =
    include_bytes!("../../../corpus/native/cold-05-05-image/notebook/synthetic.one");
const AUTHOR: &str = "Picture author";
/// A one-pixel PNG.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53,
    0xde, 0x00, 0x00, 0x00, 0x0c, 0x49, 0x44, 0x41, 0x54, 0x08, 0xd7, 0x63, 0xf8, 0xcf, 0xc0, 0x00,
    0x00, 0x03, 0x01, 0x01, 0x00, 0x18, 0xdd, 0x8d, 0xb0, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e,
    0x44, 0xae, 0x42, 0x60, 0x82,
];

fn first_page(bytes: &[u8]) -> (ExGuid, Page) {
    let store = Store::parse(bytes).unwrap();
    assert!(store.checksum_mismatches.is_empty());
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let (space, _) = document.pages().unwrap()[0];
    (space, Page::from_space(&document, space).unwrap())
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

fn pictures(page: &Page) -> Vec<&Image> {
    page.objects
        .iter()
        .filter_map(|object| match object {
            PageObject::Outline(outline) => Some(outline),
            _ => None,
        })
        .flat_map(|outline| outline.paragraphs.iter())
        .filter_map(|p| match &p.content {
            ParagraphContent::Image(image) => Some(image),
            _ => None,
        })
        .collect()
}

fn plain_paragraph(template: &PageParagraph, text: &str) -> PageParagraph {
    let mut paragraph = template.clone();
    paragraph.id = new_id().unwrap();
    paragraph.parent = None;
    paragraph.level = 1;
    paragraph.lists.clear();
    paragraph.tags.clear();
    paragraph.style = None;
    paragraph.collapsed = false;
    paragraph.content = ParagraphContent::Text(TextObject {
        id: new_id().unwrap(),
        date_field: None,
        text: onestore::page::Paragraph::new(
            text.into(),
            Format {
                font: Some("Calibri".into()),
                font_size: Some(11.0),
                language: Some(1033),
                ..Default::default()
            },
        ),
        tags: Vec::new(),
    });
    paragraph
}

#[test]
fn a_native_picture_reads_as_paragraph_content_with_its_payload() {
    let (_, page) = first_page(NATIVE);
    let found = pictures(&page);
    let [picture] = found.as_slice() else {
        panic!("{page:?}");
    };
    assert_eq!(picture.bytes.as_ref().map(|b| b.len()), Some(68));
    assert!(picture.bytes.as_ref().unwrap().starts_with(&PNG[..8]));
    assert_eq!(picture.size, Some([0.75, 0.75]));
}

/// `ONESTORE_IMAGE_EXPORT` names a new directory receiving the candidate for a cold reopen.
#[test]
fn a_picture_inserted_on_a_fresh_page_reads_back_and_can_be_removed() {
    let source = onestore::create_section("pictures.one", "Before the picture", "Author").unwrap();
    let (space, before) = first_page(&source);
    let mut after = before.clone();
    let template = body_paragraphs(&mut after)[0].clone();
    let mut holder = plain_paragraph(&template, "");
    holder.format = Format::default();
    let image = Image {
        id: new_id().unwrap(),
        layout: Default::default(),
        size: Some([0.75, 0.75]),
        bytes: Some(Arc::from(PNG)),
        alt: None,
        background: false,
    };
    holder.content = ParagraphContent::Image(image.clone());
    body_paragraphs(&mut after).push(holder.clone());
    body_paragraphs(&mut after).push(plain_paragraph(&template, "After the picture"));
    let written = PreparedEdit::page(&source, space, &after, AUTHOR).unwrap();
    let stored = page_in(written.as_bytes(), space);
    let mut expected = after.clone();
    expected.title = stored.title.clone();
    assert_eq!(stored, expected);
    let found = pictures(&stored);
    let [picture] = found.as_slice() else {
        panic!()
    };
    assert_eq!(picture.bytes.as_deref(), Some(PNG));
    assert_eq!(
        PreparedEdit::page(written.as_bytes(), space, &stored, AUTHOR)
            .unwrap()
            .as_bytes(),
        written.as_bytes()
    );
    if let Some(directory) = std::env::var_os("ONESTORE_IMAGE_EXPORT") {
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(directory.join("pictures.one"), written.as_bytes()).unwrap();
        let written_store = Store::parse(written.as_bytes()).unwrap();
        std::fs::write(
            directory.join("Open Notebook.onetoc2"),
            onestore::create_table_of_contents(
                "Open Notebook.onetoc2",
                &[("pictures.one", written_store.header.file_id)],
            )
            .unwrap(),
        )
        .unwrap();
    }
    let mut removed = stored.clone();
    body_paragraphs(&mut removed).retain(|p| p.id != holder.id);
    let again = PreparedEdit::page(written.as_bytes(), space, &removed, AUTHOR).unwrap();
    let stored = page_in(again.as_bytes(), space);
    assert!(pictures(&stored).is_empty());
    assert_eq!(body_paragraphs(&mut stored.clone()).len(), 2);
}

#[test]
fn pictures_need_a_recognised_payload_and_stored_ones_stay_fixed() {
    let source = onestore::create_section("pictures.one", "Text", "Author").unwrap();
    let (space, before) = first_page(&source);
    let mut after = before.clone();
    let template = body_paragraphs(&mut after)[0].clone();
    let mut holder = plain_paragraph(&template, "");
    holder.content = ParagraphContent::Image(Image {
        id: new_id().unwrap(),
        layout: Default::default(),
        size: None,
        bytes: Some(Arc::from(b"not a picture".as_slice())),
        alt: None,
        background: false,
    });
    body_paragraphs(&mut after).push(holder);
    assert!(PreparedEdit::page(&source, space, &after, AUTHOR).is_err());
    let (space, native) = first_page(NATIVE);
    let mut resized = native.clone();
    let mut found = 0;
    for object in &mut resized.objects {
        if let PageObject::Outline(outline) = object {
            for paragraph in &mut outline.paragraphs {
                if let ParagraphContent::Image(image) = &mut paragraph.content {
                    image.size = Some([10.0, 10.0]);
                    found += 1;
                }
            }
        }
    }
    assert_eq!(found, 1);
    assert!(PreparedEdit::page(NATIVE, space, &resized, AUTHOR).is_err());
}
