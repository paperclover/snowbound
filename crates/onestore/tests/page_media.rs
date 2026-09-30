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
    titled(bytes, TITLE)
}

fn titled(bytes: &[u8], title: &str) -> (ExGuid, Page) {
    let store = Store::parse(bytes).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    document
        .pages()
        .unwrap()
        .into_iter()
        .map(|(space, _)| (space, Page::from_space(&document, space).unwrap()))
        .find(|(_, page)| page.title == title)
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

/// The recordings page `space` lists (AudioRecordingGuids).
fn playlist(bytes: &[u8], space: ExGuid) -> Vec<[u8; 16]> {
    let store = Store::parse(bytes).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let revision = document.spaces[&space].active().unwrap();
    revision
        .nodes
        .values()
        .find_map(|node| match &node.kind {
            onestore::document::Kind::Page { recordings, .. } => Some(recordings.clone()),
            _ => None,
        })
        .unwrap()
}

/// OneNote 2010's own recording (`corpus/recording/native`): a WMA started after "Before
/// recording", its line saying when and notes linked while it ran.
const RECORDED: &[u8] = include_bytes!("../../../corpus/recording/native/notebook/files.one");
/// The same page after OneNote deleted the recording: the page lists no recording, and the
/// notes keep their links.
const DELETED: &[u8] = include_bytes!("../../../corpus/recording/native/deleted/files.one");

/// `ONESTORE_RECORDING_EDIT_EXPORT` names a new directory receiving the edited section for
/// a cold reopen (`corpus/recording/edit`).
#[test]
fn onenote_s_recording_round_trips_through_deleting_restoring_and_linking() {
    let (space, native) = titled(RECORDED, "Recorded");
    let (native_recording, _) = recording(&native);
    assert_eq!(native_recording.kind, 1);
    assert!(native_recording.duration_ms.is_some_and(|ms| ms > 30_000));
    assert_eq!(playlist(RECORDED, space), vec![native_recording.id]);
    let (_, deleted) = titled(DELETED, "Recorded");
    assert!(playlist(DELETED, space).is_empty());
    assert!(recording_paragraph(&deleted).is_none());

    // Deleting the recording unlists it, as OneNote does; the notes keep their links.
    let mut removed = native.clone();
    let outline = body(&mut removed);
    let at = outline
        .paragraphs
        .iter()
        .position(|paragraph| matches!(paragraph.content, ParagraphContent::Attachment(_)))
        .unwrap();
    let file = outline.paragraphs.remove(at);
    let written = ops::saved(RECORDED, space, &removed).unwrap();
    assert!(playlist(&written, space).is_empty());
    assert_eq!(titled(&written, "Recorded").1, removed);

    // Putting it back, as undo does, stores the same recording, listed again.
    let written = ops::saved(&written, space, &native).unwrap();
    let (_, restored) = titled(&written, "Recorded");
    assert_eq!(restored, native);
    let bytes = |page: &Page| match &recording_paragraph(page).unwrap().content {
        ParagraphContent::Attachment(file) => file.bytes.clone(),
        _ => unreachable!(),
    };
    assert_eq!(bytes(&restored), bytes(&native));
    assert!(bytes(&restored).unwrap().starts_with(b"0&\xb2\x75"));
    assert_eq!(playlist(&written, space), vec![native_recording.id]);
    assert_eq!(recording_paragraph(&restored).unwrap(), &file);

    // A note written while playing it back links to a moment, as one taken while recording.
    let mut linked = restored.clone();
    let mut added = ops::paragraph("Linked by Snowbound");
    added.media = MediaIndex {
        recordings: vec![native_recording.id],
        time_ms: Some(25_000),
    };
    body(&mut linked).paragraphs.push(added);
    let written = ops::saved(&written, space, &linked).unwrap();
    assert_eq!(titled(&written, "Recorded").1, linked);

    if let Some(directory) = std::env::var_os("ONESTORE_RECORDING_EDIT_EXPORT") {
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("files.one"), &written).unwrap();
        std::fs::write(
            directory.join("Open Notebook.onetoc2"),
            include_bytes!("../../../corpus/recording/native/notebook/Open Notebook.onetoc2"),
        )
        .unwrap();
    }
}

fn body(page: &mut Page) -> &mut onestore::page::Outline {
    page.objects
        .iter_mut()
        .find_map(|object| match object {
            PageObject::Outline(outline) if !outline.title => Some(outline),
            _ => None,
        })
        .unwrap()
}

fn recording_paragraph(page: &Page) -> Option<&onestore::page::PageParagraph> {
    page.objects
        .iter()
        .filter_map(|object| match object {
            PageObject::Outline(outline) => Some(&outline.paragraphs),
            _ => None,
        })
        .flatten()
        .find(|paragraph| {
            matches!(&paragraph.content, ParagraphContent::Attachment(file) if file.recording.is_some())
        })
}
