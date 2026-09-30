#[path = "support/ops.rs"]
mod ops;
use onestore::{
    ExGuid, RevisionIndex, Store,
    document::{Document, Format, Layout},
    page::{Image, Page, PageObject, PageParagraph, ParagraphContent, TextObject, text::new_id},
};
use std::sync::Arc;

const NATIVE: &[u8] =
    include_bytes!("../../../corpus/native/cold-05-05-image/notebook/synthetic.one");
const NATIVE_RESIZE: &[u8] =
    include_bytes!("../../../corpus/picture-edit/native-resize/notebook/pictures.one");
const NATIVE_PAGE_LEVEL: &[u8] =
    include_bytes!("../../../corpus/picture-edit/native-page-level/notebook/pictures.one");
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
        display: None,
        alt: None,
        background: false,
        printout: None,
        tags: Vec::new(),
        link: None,
        text: None,
    };
    holder.content = ParagraphContent::Image(image.clone());
    body_paragraphs(&mut after).push(holder.clone());
    body_paragraphs(&mut after).push(plain_paragraph(&template, "After the picture"));
    let written = ops::saved(&source, space, &after).unwrap();
    let stored = page_in(written.as_slice(), space);
    let mut expected = after.clone();
    expected.title = stored.title.clone();
    assert_eq!(stored, expected);
    let found = pictures(&stored);
    let [picture] = found.as_slice() else {
        panic!()
    };
    assert_eq!(picture.bytes.as_deref(), Some(PNG));
    assert_eq!(
        ops::saved(written.as_slice(), space, &stored)
            .unwrap()
            .as_slice(),
        written.as_slice()
    );
    if let Some(directory) = std::env::var_os("ONESTORE_IMAGE_EXPORT") {
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(directory.join("pictures.one"), written.as_slice()).unwrap();
        let written_store = Store::parse(written.as_slice()).unwrap();
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
    let again = ops::saved(written.as_slice(), space, &removed).unwrap();
    let stored = page_in(again.as_slice(), space);
    assert!(pictures(&stored).is_empty());
    assert_eq!(body_paragraphs(&mut stored.clone()).len(), 2);
}

fn resize(page: &mut Page, layout: Layout, alt: Option<&str>) -> ExGuid {
    let mut found = None;
    for object in &mut page.objects {
        if let PageObject::Outline(outline) = object {
            for paragraph in &mut outline.paragraphs {
                if let ParagraphContent::Image(image) = &mut paragraph.content {
                    image.layout = layout.clone();
                    image.alt = alt.map(str::to_owned);
                    assert!(found.replace(image.id).is_none());
                }
            }
        }
    }
    found.unwrap()
}

/// `ONESTORE_IMAGE_RESIZE_EXPORT` names a new directory receiving the candidate for a cold reopen.
#[test]
fn a_native_picture_is_resized_and_described_then_reset() {
    let (space, native) = first_page(NATIVE);
    let mut resized = native.clone();
    let layout = Layout {
        max_width: Some(144.0),
        width_set_by_user: Some(true),
        max_height: Some(108.0),
        ..Default::default()
    };
    resize(&mut resized, layout, Some("Resized in Rust"));
    let written = ops::saved(NATIVE, space, &resized).unwrap();
    let stored = page_in(written.as_slice(), space);
    assert_eq!(stored, resized);
    assert_eq!(
        ops::saved(written.as_slice(), space, &stored)
            .unwrap()
            .as_slice(),
        written.as_slice()
    );
    if let Some(directory) = std::env::var_os("ONESTORE_IMAGE_RESIZE_EXPORT") {
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(directory.join("synthetic.one"), written.as_slice()).unwrap();
        let written_store = Store::parse(written.as_slice()).unwrap();
        std::fs::write(
            directory.join("Open Notebook.onetoc2"),
            onestore::create_table_of_contents(
                "Open Notebook.onetoc2",
                &[("synthetic.one", written_store.header.file_id)],
            )
            .unwrap(),
        )
        .unwrap();
    }
    let mut reset = stored.clone();
    resize(&mut reset, Layout::default(), None);
    let again = ops::saved(written.as_slice(), space, &reset).unwrap();
    assert_eq!(page_in(again.as_slice(), space), native);
}

#[test]
fn a_rust_picture_resized_natively_reads_back_with_its_size_and_description() {
    let (space, page) = first_page(NATIVE_RESIZE);
    let found = pictures(&page);
    let [picture] = found.as_slice() else {
        panic!("{page:?}");
    };
    assert_eq!(picture.bytes.as_deref(), Some(PNG));
    assert_eq!(picture.size, Some([0.75, 0.75]));
    assert_eq!(picture.layout.max_width, Some(144.0));
    assert_eq!(picture.layout.max_height, Some(108.0));
    assert_eq!(picture.layout.width_set_by_user, Some(true));
    assert_eq!(picture.alt.as_deref(), Some("Resized by OneNote"));
    let mut wider = page.clone();
    let mut layout = picture.layout.clone();
    layout.max_width = Some(200.0);
    resize(&mut wider, layout, Some("Wider in Rust"));
    let written = ops::saved(NATIVE_RESIZE, space, &wider).unwrap();
    assert_eq!(page_in(written.as_slice(), space), wider);
}

fn page_pictures(page: &Page) -> Vec<&Image> {
    page.objects
        .iter()
        .filter_map(|object| match object {
            PageObject::Image(image) => Some(image),
            _ => None,
        })
        .collect()
}

#[test]
fn a_page_level_picture_placed_natively_reads_as_a_page_object() {
    let (space, page) = first_page(NATIVE_PAGE_LEVEL);
    assert_eq!(pictures(&page).len(), 1);
    let found = page_pictures(&page);
    let [picture] = found.as_slice() else {
        panic!("{page:?}");
    };
    assert_eq!(picture.bytes.as_deref(), Some(PNG));
    assert_eq!(
        (picture.layout.x, picture.layout.y),
        (Some(360.0), Some(240.0))
    );
    assert_eq!(picture.layout.width_set_by_user, Some(true));
    assert_eq!(picture.alt.as_deref(), Some("Page-level picture"));
    let mut moved = page.clone();
    for object in &mut moved.objects {
        if let PageObject::Image(image) = object {
            image.layout.x = Some(72.0);
            image.layout.y = Some(400.0);
        }
    }
    let written = ops::saved(NATIVE_PAGE_LEVEL, space, &moved).unwrap();
    assert_eq!(page_in(written.as_slice(), space), moved);
}

/// `ONESTORE_PAGE_IMAGE_EXPORT` names a new directory receiving the candidate for a cold reopen.
#[test]
fn a_page_level_picture_is_inserted_moved_and_removed() {
    let source = onestore::create_section("pictures.one", "Beside the picture", "Author").unwrap();
    let (space, before) = first_page(&source);
    let mut after = before.clone();
    let image = Image {
        id: new_id().unwrap(),
        layout: Layout {
            x: Some(360.0),
            y: Some(240.0),
            max_width: Some(96.0),
            width_set_by_user: Some(true),
            max_height: Some(72.0),
            reserved_width: None,
        },
        size: Some([0.75, 0.75]),
        bytes: Some(Arc::from(PNG)),
        display: None,
        alt: Some("Placed in Rust".into()),
        background: false,
        printout: None,
        tags: Vec::new(),
        link: None,
        text: None,
    };
    after.objects.push(PageObject::Image(image.clone()));
    let written = ops::saved(&source, space, &after).unwrap();
    let stored = page_in(written.as_slice(), space);
    let mut expected = after.clone();
    expected.title = stored.title.clone();
    assert_eq!(stored, expected);
    assert_eq!(page_pictures(&stored)[0].bytes.as_deref(), Some(PNG));
    assert_eq!(
        ops::saved(written.as_slice(), space, &stored)
            .unwrap()
            .as_slice(),
        written.as_slice()
    );
    if let Some(directory) = std::env::var_os("ONESTORE_PAGE_IMAGE_EXPORT") {
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(directory.join("pictures.one"), written.as_slice()).unwrap();
        let written_store = Store::parse(written.as_slice()).unwrap();
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
    let mut moved = stored.clone();
    for object in &mut moved.objects {
        if let PageObject::Image(image) = object {
            image.layout.x = Some(36.0);
            image.alt = None;
        }
    }
    let again = ops::saved(written.as_slice(), space, &moved).unwrap();
    assert_eq!(page_in(again.as_slice(), space), moved);
    let mut removed = moved.clone();
    removed.objects.retain(|object| object.id() != image.id);
    let last = ops::saved(again.as_slice(), space, &removed).unwrap();
    let stored = page_in(last.as_slice(), space);
    assert!(page_pictures(&stored).is_empty());
    assert_eq!(stored, removed);
    let mut unplaced = stored.clone();
    let mut nowhere = image.clone();
    nowhere.layout = Layout::default();
    unplaced.objects.push(PageObject::Image(nowhere));
    assert!(ops::saved(last.as_slice(), space, &unplaced).is_err());
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
        display: None,
        alt: None,
        background: false,
        printout: None,
        tags: Vec::new(),
        link: None,
        text: None,
    });
    body_paragraphs(&mut after).push(holder);
    assert!(ops::saved(&source, space, &after).is_err());
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
    assert!(ops::saved(NATIVE, space, &resized).is_err());
}
