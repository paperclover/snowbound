//! Files attached through the editor, as Snowbound's Attach File and drops store them: the
//! ops the editor records seal into a section that reads back with the exact bytes.
//! `CANVAS_ATTACHMENT_EXPORT` names a directory receiving that section as a notebook, for
//! a cold reopen in OneNote 2010 (`corpus/attachment-insert`), and `CANVAS_FLOATING_EXPORT`
//! the section of files attached on blank page (`corpus/attachment-floating`).

use canvas::{editor::CanvasEditor, layout::TextEngine};
use draw::edit::Movement;
use onestore::{
    Arena, Section, Store,
    op::{Edit, Op},
    page::{Attachment, PageObject, ParagraphContent},
};
use std::sync::Arc;

/// The icon OneNote 2010 stored with a text file (`corpus/native/cold-05-06-attachment`).
fn native_icon() -> Arc<[u8]> {
    let native =
        include_bytes!("../../../corpus/native/cold-05-06-attachment/notebook/synthetic.one");
    let arena = Arena::default();
    let mut section = Section::open(&arena, native.to_vec()).unwrap();
    let (space, ..) = section.pages().unwrap()[0].clone();
    let page = section.page(space).unwrap();
    page.objects
        .iter()
        .filter_map(|object| match object {
            PageObject::Outline(outline) => Some(&outline.paragraphs),
            _ => None,
        })
        .flatten()
        .find_map(|paragraph| match &paragraph.content {
            ParagraphContent::Attachment(file) => file.preview.clone(),
            _ => None,
        })
        .unwrap()
}

/// Two mebibytes no compressor shortens, larger than any buffer the writer fills at once.
fn large() -> Vec<u8> {
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    (0..2 << 20)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        })
        .collect()
}

fn file(name: &str, bytes: Vec<u8>, preview: Option<Arc<[u8]>>) -> Attachment {
    Attachment {
        id: onestore::page::text::new_id().unwrap(),
        filename: name.into(),
        source_path: Some(format!("/Users/snowbound/Documents/{name}")),
        size: Some([24.0, 24.0]),
        layout: Default::default(),
        bytes: Some(bytes.into()),
        preview,
        recording: None,
        tags: Vec::new(),
    }
}

#[test]
fn attached_files_store_their_bytes_between_the_text_around_them() {
    let source = onestore::create_section("files.one", "Before the file", "Author").unwrap();
    let arena = Arena::default();
    let mut section = Section::open(&arena, source.clone()).unwrap();
    let (space, ..) = section.pages().unwrap()[0].clone();
    let mut engine = TextEngine::default();
    let mut editor = CanvasEditor::from_page(section.page(space).unwrap(), &mut engine).unwrap();
    let body = editor
        .outlines()
        .iter()
        .find(|outline| !outline.title)
        .unwrap()
        .id;
    editor.focus_outline(body).unwrap();
    editor
        .move_selection(&mut engine, Movement::DocumentEnd, false)
        .unwrap();
    let small = file(
        "notes 🦀.txt",
        b"Snowbound attached this file.\n".to_vec(),
        Some(native_icon()),
    );
    // The icon a host without the system's draws, which `PageView::insert_attachment` gives.
    let large = file(
        "large.bin",
        large(),
        Some(canvas::gpu::page::file_icon().into()),
    );
    let mut at = 133_000_000_000_000_000;
    let mut store = |editor: &mut CanvasEditor| {
        let ops = editor.take_ops().unwrap();
        assert!(!ops.is_empty());
        at += 10_000_000;
        let ops = ops.into_iter().map(|op| Op::Page { space, op }).collect();
        section.apply("Author", &Edit { at, ops }).unwrap();
    };
    editor
        .insert_attachment(&mut engine, small.clone())
        .unwrap();
    store(&mut editor);
    editor.insert(&mut engine, "After the file").unwrap();
    store(&mut editor);
    editor
        .insert_attachment(&mut engine, large.clone())
        .unwrap();
    store(&mut editor);
    let mut image = source;
    section.seal().unwrap().unwrap().apply(&mut image).unwrap();

    let arena = Arena::default();
    let reopened = Section::open(&arena, image.clone()).unwrap();
    let page = reopened.page(space).unwrap();
    let body = page
        .objects
        .iter()
        .find_map(|object| match object {
            PageObject::Outline(outline) if !outline.title => Some(outline),
            _ => None,
        })
        .unwrap();
    let shown: Vec<String> = body
        .paragraphs
        .iter()
        .map(|paragraph| match &paragraph.content {
            ParagraphContent::Text(text) => text.text.text().to_owned(),
            ParagraphContent::Attachment(file) => {
                let expected = [&small, &large]
                    .into_iter()
                    .find(|expected| expected.id == file.id)
                    .unwrap();
                assert_eq!(file, expected);
                assert_eq!(file.bytes, expected.bytes);
                assert_eq!(file.preview, expected.preview);
                format!("[{}]", file.filename)
            }
            _ => panic!(),
        })
        .collect();
    assert_eq!(
        shown,
        [
            "Before the file",
            "[notes 🦀.txt]",
            "After the file",
            "[large.bin]",
            ""
        ]
    );

    if let Some(directory) = std::env::var_os("CANVAS_ATTACHMENT_EXPORT") {
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("files.one"), &image).unwrap();
        let file_id = Store::parse(&image).unwrap().header.file_id;
        std::fs::write(
            directory.join("Open Notebook.onetoc2"),
            onestore::create_table_of_contents("Open Notebook.onetoc2", &[("files.one", file_id)])
                .unwrap(),
        )
        .unwrap();
    }
}

/// Files attached where a click on blank page left the caret lie on the page there, as OneNote
/// 2010 places them, and move as one does.
#[test]
fn files_attached_on_blank_page_store_their_bytes_where_they_were_placed_and_moved() {
    let source = onestore::create_section("files.one", "Floating files", "Author").unwrap();
    let arena = Arena::default();
    let mut section = Section::open(&arena, source.clone()).unwrap();
    let (space, ..) = section.pages().unwrap()[0].clone();
    let mut engine = TextEngine::default();
    let mut editor = CanvasEditor::from_page(section.page(space).unwrap(), &mut engine).unwrap();
    let small = file(
        "float 🦀.txt",
        b"Snowbound put this file on the page.\n".to_vec(),
        Some(native_icon()),
    );
    let large = file(
        "large.bin",
        large(),
        Some(canvas::gpu::page::file_icon().into()),
    );
    let mut at = 133_000_000_000_000_000;
    let mut store = |editor: &mut CanvasEditor| {
        let ops = editor.take_ops().unwrap();
        assert_eq!(ops.len(), 1, "{ops:?}");
        at += 10_000_000;
        let ops = ops.into_iter().map(|op| Op::Page { space, op }).collect();
        section.apply("Author", &Edit { at, ops }).unwrap();
    };
    editor
        .place_caret(&mut engine, [342.0, 284.4], 240.0)
        .unwrap();
    editor
        .insert_attachment(&mut engine, small.clone())
        .unwrap();
    store(&mut editor);
    editor
        .place_caret(&mut engine, [126.0, 410.4], 240.0)
        .unwrap();
    editor
        .insert_attachment(&mut engine, large.clone())
        .unwrap();
    store(&mut editor);
    let (_, size) = editor.image_placement(large.id).unwrap();
    editor
        .place_image(&mut engine, large.id, [288.0, 140.4], size)
        .unwrap();
    store(&mut editor);
    let mut image = source;
    section.seal().unwrap().unwrap().apply(&mut image).unwrap();

    let arena = Arena::default();
    let reopened = Section::open(&arena, image.clone()).unwrap();
    let page = reopened.page(space).unwrap();
    let files: Vec<&Attachment> = page
        .objects
        .iter()
        .filter_map(|object| match object {
            PageObject::Attachment(file) => Some(file),
            _ => None,
        })
        .collect();
    let [first, second] = files.as_slice() else {
        panic!("{page:?}")
    };
    for (stored, expected, at) in [
        (first, &small, [342.0, 284.4]),
        (second, &large, [288.0, 140.4]),
    ] {
        assert_eq!(stored.id, expected.id);
        assert_eq!([stored.layout.x, stored.layout.y], at.map(Some));
        assert_eq!(stored.layout.max_width, Some(54.0));
        assert_eq!(stored.bytes, expected.bytes);
        assert_eq!(stored.preview, expected.preview);
    }
    assert_eq!(editor.page().unwrap(), page);

    if let Some(directory) = std::env::var_os("CANVAS_FLOATING_EXPORT") {
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("files.one"), &image).unwrap();
        let file_id = Store::parse(&image).unwrap().header.file_id;
        std::fs::write(
            directory.join("Open Notebook.onetoc2"),
            onestore::create_table_of_contents("Open Notebook.onetoc2", &[("files.one", file_id)])
                .unwrap(),
        )
        .unwrap();
    }
}
