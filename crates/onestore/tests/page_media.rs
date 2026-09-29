#[path = "support/ops.rs"]
mod ops;
use onestore::{
    ExGuid, RevisionIndex, Store,
    document::Document,
    page::{MediaIndex, Page, PageObject, ParagraphContent, Recording},
};

/// A page OneNote 2010 recorded audio on (`corpus/m6/native-features-01`): a recording
/// attachment and a paragraph annotated with a moment in it.
const NATIVE: &[u8] = include_bytes!("../../../corpus/m6/native-features-01/notebook/Features.one");
const NATIVE_READ: &str = include_str!("../../../corpus/m6/native-features-01/read/page-017.xml");
const TITLE: &str = "Files and recording";

fn page(bytes: &[u8]) -> (ExGuid, Page) {
    let store = Store::parse(bytes).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    document
        .pages()
        .unwrap()
        .into_iter()
        .map(|(space, _)| (space, Page::from_space(&document, space).unwrap()))
        .find(|(_, page)| page.title == TITLE)
        .unwrap()
}

fn media_id(read: &str) -> [u8; 16] {
    let start = read.find("mediaID=\"").unwrap() + 9;
    let guid = &read[start..read[start..].find('"').unwrap() + start];
    format!("{guid},0").parse::<ExGuid>().unwrap().guid
}

fn recording(page: &Page) -> (Recording, MediaIndex) {
    let mut recording = None;
    let mut annotation = None;
    for object in &page.objects {
        let PageObject::Outline(outline) = object else {
            continue;
        };
        for paragraph in &outline.paragraphs {
            if let ParagraphContent::Attachment(attachment) = &paragraph.content
                && let Some(found) = attachment.recording
            {
                recording = Some(found);
            }
            if !paragraph.media.recordings.is_empty() {
                annotation = Some(paragraph.media.clone());
            }
        }
    }
    (recording.unwrap(), annotation.unwrap())
}

#[test]
fn a_native_recording_and_its_annotation_read_as_the_export_names_them() {
    let (_, native) = page(NATIVE);
    let (recording, annotation) = recording(&native);
    let id = media_id(NATIVE_READ);
    assert_eq!(recording.id, id);
    assert_eq!(
        annotation,
        MediaIndex {
            recordings: vec![id],
            time_ms: Some(500)
        }
    );
    assert!(NATIVE_READ.contains("timeIndex=\"500\""));
}

/// `ONESTORE_MEDIA_EXPORT` names a new directory receiving the edited section for a cold
/// reopen: the recording and its annotation survive an ordinary edit of the page.
#[test]
fn editing_a_recorded_page_keeps_the_recording_and_its_annotation() {
    use onestore::page::{TextObject, text::new_id};
    let (space, before) = page(NATIVE);
    let mut after = before.clone();
    let outline = after
        .objects
        .iter_mut()
        .find_map(|object| match object {
            PageObject::Outline(outline) if !outline.title => Some(outline),
            _ => None,
        })
        .unwrap();
    let mut added = outline.paragraphs[0].clone();
    added.id = new_id().unwrap();
    added.parent = None;
    added.level = 1;
    added.style = None;
    added.lists.clear();
    added.tags.clear();
    added.media = MediaIndex::default();
    added.content = ParagraphContent::Text(TextObject {
        id: new_id().unwrap(),
        date_field: None,
        text: onestore::page::Paragraph::new("Added after recording".into(), Default::default()),
        tags: Vec::new(),
    });
    outline.paragraphs.push(added.clone());
    let written = ops::saved(NATIVE, space, &after).unwrap();
    let (_, stored) = page(written.as_slice());
    assert_eq!(recording(&stored), recording(&before));
    assert_eq!(stored.objects.len(), before.objects.len());

    // A note linked to the recording, then unlinked, as Snowbound links notes taken while
    // it records.
    let annotate = |page: &Page, media: MediaIndex| {
        let mut page = page.clone();
        for object in &mut page.objects {
            if let PageObject::Outline(outline) = object
                && !outline.title
            {
                outline.paragraphs.last_mut().unwrap().media = media.clone();
            }
        }
        page
    };
    let linked = ops::saved(
        written.as_slice(),
        space,
        &annotate(&stored, recording(&before).1),
    )
    .unwrap();
    let (_, relinked) = page(linked.as_slice());
    assert_eq!(relinked, annotate(&stored, recording(&before).1));
    let unlinked = ops::saved(linked.as_slice(), space, &stored).unwrap();
    assert_eq!(page(unlinked.as_slice()).1, stored);
    let mut recaptured = after.clone();
    for object in &mut recaptured.objects {
        if let PageObject::Outline(outline) = object {
            for paragraph in &mut outline.paragraphs {
                if let ParagraphContent::Attachment(attachment) = &mut paragraph.content {
                    attachment.recording = None;
                }
            }
        }
    }
    assert!(ops::saved(written.as_slice(), space, &recaptured).is_err());

    if let Some(directory) = std::env::var_os("ONESTORE_MEDIA_EXPORT") {
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("Features.one"), written.as_slice()).unwrap();
        let file_id = Store::parse(written.as_slice()).unwrap().header.file_id;
        std::fs::write(
            directory.join("Open Notebook.onetoc2"),
            onestore::create_table_of_contents(
                "Open Notebook.onetoc2",
                &[("Features.one", file_id)],
            )
            .unwrap(),
        )
        .unwrap();
        let store = Store::parse(written.as_slice()).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let count = Document::parse(&index).unwrap().pages().unwrap().len();
        std::fs::write(directory.join("expected-count.txt"), count.to_string()).unwrap();
    }
}
