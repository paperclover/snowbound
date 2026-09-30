//! Recording through the editor, as Snowbound's Record Audio stores it: the ops the editor
//! records seal into a section that reads back as the editor's page, with the line saying
//! when, the notes written meanwhile linked to their moments, and the file where recording
//! started.

use canvas::{editor::CanvasEditor, layout::TextEngine};
use draw::edit::Movement;
use onestore::{
    Arena, Section,
    op::{Edit, Op},
    page::{Attachment, MediaIndex, Page, PageObject, ParagraphContent, Recording},
};

fn body(page: &Page) -> Vec<(String, MediaIndex)> {
    page.objects
        .iter()
        .find_map(|object| match object {
            PageObject::Outline(outline) if !outline.title => Some(outline),
            _ => None,
        })
        .unwrap()
        .paragraphs
        .iter()
        .map(|paragraph| {
            let shown = match &paragraph.content {
                ParagraphContent::Text(text) => text.text.text().to_owned(),
                ParagraphContent::Attachment(file) => format!("[{}]", file.filename),
                _ => panic!(),
            };
            (shown, paragraph.media.clone())
        })
        .collect()
}

#[test]
fn a_recording_stores_its_line_its_linked_notes_and_its_file() {
    let source = onestore::create_section("files.one", "Recorded", "Author").unwrap();
    let arena = Arena::default();
    let mut section = Section::open(&arena, source.clone()).unwrap();
    let (space, ..) = section.pages().unwrap()[0].clone();
    let mut engine = TextEngine::default();
    let mut editor = CanvasEditor::from_page(section.page(space).unwrap(), &mut engine).unwrap();
    let outline = editor
        .outlines()
        .iter()
        .find(|outline| !outline.title)
        .unwrap()
        .id;
    editor.focus_outline(outline).unwrap();
    editor
        .move_selection(&mut engine, Movement::DocumentEnd, false)
        .unwrap();
    let mut at = 133_000_000_000_000_000;
    let mut store = |editor: &mut CanvasEditor| {
        let ops = editor.take_ops().unwrap();
        at += 10_000_000;
        let ops = ops.into_iter().map(|op| Op::Page { space, op }).collect();
        section.apply("Author", &Edit { at, ops }).unwrap();
        let stored = section.page(space).unwrap();
        assert_eq!(
            Page {
                title: stored.title.clone(),
                ..editor.page().unwrap()
            },
            stored
        );
        stored
    };
    let id = editor
        .start_recording(&mut engine, "Audio recording started: 5:18 PM")
        .unwrap();
    assert_eq!(editor.recording(), Some(id));
    store(&mut editor);
    std::thread::sleep(std::time::Duration::from_millis(20));
    editor.insert(&mut engine, "A linked note").unwrap();
    store(&mut editor);
    editor.insert(&mut engine, "\n").unwrap();
    store(&mut editor);
    let file = Attachment {
        id: onestore::page::text::new_id().unwrap(),
        filename: "Recorded.wav".into(),
        source_path: None,
        size: Some([24.0, 24.0]),
        layout: Default::default(),
        bytes: Some(b"RIFF".as_slice().into()),
        preview: Some(canvas::gpu::page::file_icon().into()),
        recording: Some(Recording {
            id,
            kind: 1,
            duration_ms: Some(1000),
        }),
    };
    editor.finish_recording(&mut engine, file).unwrap();
    assert_eq!(editor.recording(), None);
    let stored = store(&mut editor);
    let shown = body(&stored);
    let texts: Vec<&str> = shown.iter().map(|(text, _)| text.as_str()).collect();
    assert_eq!(
        texts,
        [
            "Recorded",
            "[Recorded.wav]",
            "",
            "Audio recording started: 5:18 PM",
            "A linked note",
            ""
        ]
    );
    let linked = |time| MediaIndex {
        recordings: vec![id],
        time_ms: Some(time),
    };
    assert_eq!(shown[0].1, MediaIndex::default());
    assert_eq!(shown[1].1, linked(0));
    assert_eq!(shown[3].1, linked(0));
    assert!(shown[4].1.time_ms.is_some_and(|time| time >= 20));
    assert_eq!(shown[5].1, MediaIndex::default());

    // Undo takes the file and the links away as stored, and redo brings them back.
    let before = stored;
    editor.undo(&mut engine).unwrap();
    let undone = store(&mut editor);
    assert!(!body(&undone).iter().any(|(text, _)| text.starts_with('[')));
    editor.redo(&mut engine).unwrap();
    assert_eq!(store(&mut editor), before);
}

/// With the caret in the title, OneNote 2010 records at the start of the body, the file first
/// (`corpus/recording`); an empty first half of the caret's paragraph is the blank line.
#[test]
fn a_recording_from_the_title_goes_to_the_body_s_start() {
    let source = include_bytes!("../../../corpus/recording/native/notebook/files.one");
    let arena = Arena::default();
    let mut section = Section::open(&arena, source.to_vec()).unwrap();
    let (space, ..) = section
        .pages()
        .unwrap()
        .into_iter()
        .find(|(_, title, _)| title == "Recorded")
        .unwrap();
    let mut engine = TextEngine::default();
    let mut editor = CanvasEditor::from_page(section.page(space).unwrap(), &mut engine).unwrap();
    let title = editor
        .outlines()
        .iter()
        .find(|outline| outline.title)
        .unwrap()
        .id;
    editor.focus_outline(title).unwrap();
    let id = editor.start_recording(&mut engine, "Started").unwrap();
    let file = Attachment {
        id: onestore::page::text::new_id().unwrap(),
        filename: "Recorded.wav".into(),
        source_path: None,
        size: Some([24.0, 24.0]),
        layout: Default::default(),
        bytes: Some(b"RIFF".as_slice().into()),
        preview: Some(canvas::gpu::page::file_icon().into()),
        recording: Some(Recording {
            id,
            kind: 1,
            duration_ms: None,
        }),
    };
    editor.finish_recording(&mut engine, file).unwrap();
    let ops = editor.take_ops().unwrap();
    let ops = ops.into_iter().map(|op| Op::Page { space, op }).collect();
    section
        .apply(
            "Author",
            &Edit {
                at: 133_000_000_000_000_000,
                ops,
            },
        )
        .unwrap();
    let stored = section.page(space).unwrap();
    let shown = body(&stored);
    let texts: Vec<&str> = shown.iter().map(|(text, _)| text.as_str()).collect();
    assert_eq!(
        texts[..4],
        ["[Recorded.wav]", "", "Started", "Before recording edited"]
    );
    // The note keeps its link to OneNote's recording.
    assert_eq!(shown[3].1.time_ms, Some(20131));
    assert_eq!(shown[1].1, MediaIndex::default());
}

/// A note written while recording is paused links to nothing, and the pause is left out of
/// later moments, as OneNote 2010 links them (`corpus/recording/native/read/paused-while-recording.xml`).
/// See Playback then highlights the note linked last at or before the moment playing.
#[test]
fn notes_written_while_paused_stay_unlinked_and_see_playback_follows_the_rest() {
    let source = onestore::create_section("files.one", "Recorded", "Author").unwrap();
    let arena = Arena::default();
    let mut section = Section::open(&arena, source).unwrap();
    let (space, ..) = section.pages().unwrap()[0].clone();
    let mut engine = TextEngine::default();
    let mut editor = CanvasEditor::from_page(section.page(space).unwrap(), &mut engine).unwrap();
    let outline = editor
        .outlines()
        .iter()
        .find(|outline| !outline.title)
        .unwrap()
        .id;
    editor.focus_outline(outline).unwrap();
    editor
        .move_selection(&mut engine, Movement::DocumentEnd, false)
        .unwrap();
    let id = editor.start_recording(&mut engine, "Started").unwrap();
    let wait = |ms| std::thread::sleep(std::time::Duration::from_millis(ms));
    wait(30);
    editor.insert(&mut engine, "Before the pause").unwrap();
    editor.pause_recording(true);
    assert!(editor.recording_paused());
    let paused_at = editor.recording_ms().unwrap();
    editor.insert(&mut engine, "\nWhile paused").unwrap();
    wait(300);
    assert_eq!(editor.recording_ms(), Some(paused_at));
    editor.pause_recording(false);
    wait(30);
    editor.insert(&mut engine, "\nAfter the pause").unwrap();
    let page = editor.page().unwrap();
    let shown = body(&page);
    let moment = |text: &str| {
        shown
            .iter()
            .find(|(shown, _)| shown == text)
            .unwrap()
            .1
            .time_ms
    };
    let before = moment("Before the pause").unwrap();
    let after = moment("After the pause").unwrap();
    assert_eq!(moment("While paused"), None);
    assert!(
        before >= 30 && after >= before + 30 && after < before + 300,
        "{before} {after}"
    );

    let note = |at| {
        editor
            .played_note(id, at)
            .and_then(|note| canvas::search::paragraph_match(&editor, note))
            .map(|(_, selection)| selection.positions[0].paragraph)
    };
    // Before the first note only the line saying when, which is never highlighted.
    assert_eq!(note(before - 1), None);
    let first = note(before).unwrap();
    assert_eq!(note(after - 1), Some(first));
    assert!(note(after).unwrap() > first);
}
