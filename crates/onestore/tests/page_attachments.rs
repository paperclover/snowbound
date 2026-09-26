#[path = "support/ops.rs"]
mod ops;
use onestore::{
    ExGuid, RevisionIndex, Store,
    document::{Document, Format},
    page::{
        Attachment, Page, PageObject, PageParagraph, ParagraphContent, TextObject, text::new_id,
    },
};
use std::sync::Arc;

const NATIVE: &[u8] =
    include_bytes!("../../../corpus/native/cold-05-06-attachment/notebook/synthetic.one");
const PAYLOAD: &[u8] = b"Rust wrote this attachment.\n";

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

fn attachments(page: &Page) -> Vec<&Attachment> {
    page.objects
        .iter()
        .filter_map(|object| match object {
            PageObject::Outline(outline) => Some(outline),
            _ => None,
        })
        .flat_map(|outline| outline.paragraphs.iter())
        .filter_map(|p| match &p.content {
            ParagraphContent::Attachment(attachment) => Some(attachment),
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
fn a_native_attachment_reads_as_paragraph_content_with_payload_and_preview() {
    let (_, page) = first_page(NATIVE);
    let found = attachments(&page);
    let [attachment] = found.as_slice() else {
        panic!("{page:?}");
    };
    assert_eq!(attachment.filename, "fictitious-attachment.txt");
    assert!(
        attachment
            .source_path
            .as_deref()
            .unwrap()
            .ends_with("fictitious-attachment.txt")
    );
    assert_eq!(attachment.bytes.as_ref().map(|b| b.len()), Some(43));
    assert_eq!(attachment.preview.as_ref().map(|b| b.len()), Some(724));
    assert_eq!(attachment.size, Some([24.0, 24.0]));
}

fn insert_attachment(section: &str, preview: Option<Arc<[u8]>>) -> Vec<u8> {
    let source = onestore::create_section(section, "Before the file", "Author").unwrap();
    let (space, before) = first_page(&source);
    let mut after = before.clone();
    let template = body_paragraphs(&mut after)[0].clone();
    let mut holder = plain_paragraph(&template, "");
    holder.format = Format::default();
    holder.content = ParagraphContent::Attachment(Attachment {
        id: new_id().unwrap(),
        filename: "notes 🦀.txt".into(),
        source_path: None,
        size: Some([24.0, 24.0]),
        bytes: Some(Arc::from(PAYLOAD)),
        preview: preview.clone(),
        recording: None,
    });
    body_paragraphs(&mut after).push(holder.clone());
    body_paragraphs(&mut after).push(plain_paragraph(&template, "After the file"));
    let written = ops::saved(&source, space, &after).unwrap();
    let stored = page_in(written.as_slice(), space);
    let mut expected = after.clone();
    expected.title = stored.title.clone();
    assert_eq!(stored, expected);
    let found = attachments(&stored);
    let [stored_attachment] = found.as_slice() else {
        panic!()
    };
    assert_eq!(stored_attachment.bytes.as_deref(), Some(PAYLOAD));
    assert_eq!(stored_attachment.preview, preview);
    assert_eq!(
        ops::saved(written.as_slice(), space, &stored)
            .unwrap()
            .as_slice(),
        written.as_slice()
    );
    let mut removed = stored.clone();
    body_paragraphs(&mut removed).retain(|p| p.id != holder.id);
    let again = ops::saved(written.as_slice(), space, &removed).unwrap();
    assert!(attachments(&page_in(again.as_slice(), space)).is_empty());
    written.as_slice().to_vec()
}

/// `ONESTORE_ATTACHMENT_EXPORT` names a directory receiving both candidates for a cold reopen:
/// `plain/` without an icon preview and `icon/` with the native fixture's icon.
#[test]
fn an_attachment_inserted_on_a_fresh_page_reads_back_and_can_be_removed() {
    let (_, native) = first_page(NATIVE);
    let icon = attachments(&native)[0].preview.clone().unwrap();
    let plain = insert_attachment("files.one", None);
    let with_icon = insert_attachment("icon.one", Some(icon));
    if let Some(directory) = std::env::var_os("ONESTORE_ATTACHMENT_EXPORT") {
        let directory = std::path::PathBuf::from(directory);
        for (name, written) in [("plain", &plain), ("icon", &with_icon)] {
            let directory = directory.join(name);
            std::fs::create_dir_all(&directory).unwrap();
            std::fs::write(directory.join("files.one"), written).unwrap();
            let file_id = Store::parse(written).unwrap().header.file_id;
            std::fs::write(
                directory.join("Open Notebook.onetoc2"),
                onestore::create_table_of_contents(
                    "Open Notebook.onetoc2",
                    &[("files.one", file_id)],
                )
                .unwrap(),
            )
            .unwrap();
        }
    }
}

#[test]
fn attachments_need_a_file_name_and_stored_ones_are_renamed_in_place() {
    let source = onestore::create_section("files.one", "Text", "Author").unwrap();
    let (space, before) = first_page(&source);
    let mut after = before.clone();
    let template = body_paragraphs(&mut after)[0].clone();
    let mut holder = plain_paragraph(&template, "");
    holder.content = ParagraphContent::Attachment(Attachment {
        id: new_id().unwrap(),
        filename: "sub/dir.txt".into(),
        source_path: None,
        size: None,
        bytes: Some(Arc::from(PAYLOAD)),
        preview: None,
        recording: None,
    });
    body_paragraphs(&mut after).push(holder);
    assert!(ops::saved(&source, space, &after).is_err());
    let (space, native) = first_page(NATIVE);
    let mut renamed = native.clone();
    let mut found = 0;
    for object in &mut renamed.objects {
        if let PageObject::Outline(outline) = object {
            for paragraph in &mut outline.paragraphs {
                if let ParagraphContent::Attachment(attachment) = &mut paragraph.content {
                    attachment.filename = "renamed \u{1f980}.txt".into();
                    attachment.source_path = Some("C:\\inputs\\renamed \u{1f980}.txt".into());
                    found += 1;
                }
            }
        }
    }
    assert_eq!(found, 1);
    let written = ops::saved(NATIVE, space, &renamed).unwrap();
    let stored = page_in(written.as_slice(), space);
    let (before, after) = (attachments(&native)[0], attachments(&stored)[0]);
    assert_eq!(after.filename, "renamed \u{1f980}.txt");
    assert_eq!(
        after.source_path.as_deref(),
        Some("C:\\inputs\\renamed \u{1f980}.txt")
    );
    assert_eq!((after.id, after.size), (before.id, before.size));
    assert_eq!(after.bytes, before.bytes);
    assert_eq!(after.preview, before.preview);
    if let Some(directory) = std::env::var_os("ONESTORE_ATTACHMENT_RENAME_EXPORT") {
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("synthetic.one"), written.as_slice()).unwrap();
        let file_id = Store::parse(written.as_slice()).unwrap().header.file_id;
        std::fs::write(
            directory.join("Open Notebook.onetoc2"),
            onestore::create_table_of_contents(
                "Open Notebook.onetoc2",
                &[("synthetic.one", file_id)],
            )
            .unwrap(),
        )
        .unwrap();
    }
}
