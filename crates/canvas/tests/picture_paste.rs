//! Pictures pasted or inserted through the editor, placed as OneNote 2010 places them (lab,
//! 2026-09-30): at a caret in text the paragraph splits around the picture, and at a caret
//! on blank page the picture lies on the page there with the caret on the grid row below.
//! Each is one edit and one undo step. `CANVAS_PICTURE_EXPORT` names a directory receiving
//! the section as a notebook, for a cold reopen in OneNote 2010.

use canvas::{
    document::TextPosition,
    editor::{CanvasEditor, Piece, Selection},
    layout::TextEngine,
};
use onestore::{
    Arena, PageCreation, Section, Store,
    document::Layout,
    op::{Edit, Op, SectionOp},
    page::{Image, PageObject, ParagraphContent},
};
use parley::Affinity;

fn picture(size: [f32; 2]) -> Image {
    let mut png = Vec::new();
    image::RgbaImage::from_pixel(200, 100, image::Rgba([30, 144, 255, 255]))
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    Image {
        id: onestore::page::text::new_id().unwrap(),
        layout: Layout {
            max_width: Some(size[0]),
            max_height: Some(size[1]),
            ..Default::default()
        },
        size: Some(size),
        bytes: Some(png.into()),
        display: None,
        alt: None,
        background: false,
        printout: None,
        tags: Vec::new(),
        link: None,
        text: None,
    }
}

fn caret(paragraph: usize, offset: u32) -> Selection {
    let position = TextPosition { paragraph, offset };
    Selection {
        positions: [position; 2],
        affinities: [Affinity::Downstream; 2],
    }
}

/// The body outline's paragraphs: text as itself, a picture as its size.
fn body(editor: &CanvasEditor) -> Vec<String> {
    let page = editor.page().unwrap();
    let outline = page
        .objects
        .iter()
        .find_map(|object| match object {
            PageObject::Outline(outline) if !outline.title => Some(outline),
            _ => None,
        })
        .unwrap();
    outline
        .paragraphs
        .iter()
        .map(|paragraph| match &paragraph.content {
            ParagraphContent::Text(text) => text.text.text().to_owned(),
            ParagraphContent::Image(image) => format!("{:?}", image.size.unwrap()),
            _ => panic!(),
        })
        .collect()
}

fn page_pictures(editor: &CanvasEditor) -> Vec<Image> {
    editor
        .page()
        .unwrap()
        .objects
        .into_iter()
        .filter_map(|object| match object {
            PageObject::Image(image) if !image.background => Some(image),
            _ => None,
        })
        .collect()
}

#[test]
fn pasted_pictures_split_the_text_or_lie_on_the_page_as_onenote_places_them() {
    let source = onestore::create_section("pictures.one", "Pasted pictures", "Author").unwrap();
    let arena = Arena::default();
    let mut section = Section::open(&arena, source.clone()).unwrap();
    let (space, ..) = section.pages().unwrap()[0].clone();
    let mut engine = TextEngine::default();
    let mut editor = CanvasEditor::from_page(section.page(space).unwrap(), &mut engine).unwrap();
    let mut at = 133_000_000_000_000_000;
    let mut store = |editor: &mut CanvasEditor| {
        let ops = editor.take_ops().unwrap();
        assert!(!ops.is_empty());
        at += 10_000_000;
        let ops = ops.into_iter().map(|op| Op::Page { space, op }).collect();
        section.apply("Author", &Edit { at, ops }).unwrap();
    };
    let text = editor
        .outlines()
        .iter()
        .find(|outline| !outline.title)
        .unwrap()
        .id;
    editor.focus_outline(text).unwrap();

    // Within text, the caret starting the second half.
    editor.select(caret(0, 6)).unwrap();
    editor
        .insert_picture(&mut engine, picture([150.0, 75.0]))
        .unwrap();
    store(&mut editor);
    assert_eq!(body(&editor), ["Pasted", "[150.0, 75.0]", " pictures"]);
    assert_eq!(
        editor.selection().positions,
        [TextPosition {
            paragraph: 1,
            offset: 0
        }; 2]
    );
    editor.undo(&mut engine).unwrap();
    store(&mut editor);
    assert_eq!(body(&editor), ["Pasted pictures"]);

    // A web page's text and pictures in order, undone in one step.
    editor.select(caret(0, 7)).unwrap();
    let png = picture([1.0; 2]).bytes.unwrap().to_vec();
    editor
        .paste_pieces(
            &mut engine,
            vec![
                Piece::Text("web ".into()),
                Piece::Picture(png, [150.0, 75.0]),
                Piece::Text("page\ntext ".into()),
            ],
            0x0409,
        )
        .unwrap();
    store(&mut editor);
    assert_eq!(
        body(&editor),
        [
            "Pasted web ",
            "[150.0, 75.0]",
            "",
            "page",
            "text ",
            "pictures"
        ]
    );
    editor.undo(&mut engine).unwrap();
    store(&mut editor);
    assert_eq!(body(&editor), ["Pasted pictures"]);

    // A web picture still on its way ends the paragraph and goes in when it arrives, an
    // undo step of its own, while the caret stays.
    editor.select(caret(0, 7)).unwrap();
    let awaited = editor
        .paste_pieces(
            &mut engine,
            vec![
                Piece::Text("web".into()),
                Piece::Awaited,
                Piece::Text("page ".into()),
            ],
            0x0409,
        )
        .unwrap();
    store(&mut editor);
    assert_eq!(body(&editor), ["Pasted web", "page pictures"]);
    let typing = editor.selection();
    let arrived = picture([150.0, 75.0]);
    assert!(
        editor
            .insert_awaited(&mut engine, awaited[0], arrived)
            .unwrap()
    );
    store(&mut editor);
    assert_eq!(
        body(&editor),
        ["Pasted web", "[150.0, 75.0]", "page pictures"]
    );
    assert_eq!(editor.selection(), typing);
    editor.undo(&mut engine).unwrap();
    store(&mut editor);
    assert_eq!(body(&editor), ["Pasted web", "page pictures"]);
    editor.undo(&mut engine).unwrap();
    store(&mut editor);
    assert_eq!(body(&editor), ["Pasted pictures"]);

    // At the end, an empty paragraph follows for the caret; at an empty paragraph's start
    // the picture takes its place.
    editor.select(caret(0, 15)).unwrap();
    editor
        .insert_picture(&mut engine, picture([150.0, 75.0]))
        .unwrap();
    store(&mut editor);
    editor
        .insert_picture(&mut engine, picture([200.0, 100.0]))
        .unwrap();
    store(&mut editor);
    assert_eq!(
        body(&editor),
        ["Pasted pictures", "[150.0, 75.0]", "[200.0, 100.0]", ""]
    );

    // On blank page: at the caret, the caret moving to the grid row below (OneNote put a
    // 75 pt picture at y 266.4 and the caret at 356.4, a 72 pt one at 248.4 and 338.4).
    for (y, height, below) in [(266.4, 75.0, 356.4), (248.4 + 180.0, 72.0, 338.4 + 180.0)] {
        editor.place_caret(&mut engine, [270.0, y], 468.0).unwrap();
        editor
            .insert_picture(&mut engine, picture([150.0, height]))
            .unwrap();
        store(&mut editor);
        let placed = page_pictures(&editor).pop().unwrap();
        assert_eq!([placed.layout.x, placed.layout.y], [Some(270.0), Some(y)]);
        assert_eq!(editor.active_outline().origin(), [270.0, below]);
    }
    editor.insert(&mut engine, "below").unwrap();
    store(&mut editor);
    editor.undo(&mut engine).unwrap();
    editor.undo(&mut engine).unwrap();
    store(&mut editor);
    assert_eq!(page_pictures(&editor).len(), 1);

    let mut image = source;
    section.seal().unwrap().unwrap().apply(&mut image).unwrap();
    let arena = Arena::default();
    let reopened = Section::open(&arena, image.clone()).unwrap();
    let page = reopened.page(space).unwrap();
    let stored: Vec<_> = page
        .objects
        .iter()
        .filter_map(|object| match object {
            PageObject::Image(image) => Some(image),
            _ => None,
        })
        .collect();
    assert_eq!(stored.len(), 1);
    assert_eq!(
        [stored[0].layout.x, stored[0].layout.y],
        [Some(270.0), Some(266.4)]
    );
    assert_eq!(*stored[0], page_pictures(&editor)[0]);

    if let Some(directory) = std::env::var_os("CANVAS_PICTURE_EXPORT") {
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("pictures.one"), &image).unwrap();
        let file_id = Store::parse(&image).unwrap().header.file_id;
        std::fs::write(
            directory.join("Open Notebook.onetoc2"),
            onestore::create_table_of_contents(
                "Open Notebook.onetoc2",
                &[("pictures.one", file_id)],
            )
            .unwrap(),
        )
        .unwrap();
    }
}

/// From the title, OneNote 2010 (lab, 2026-09-30) put a pasted picture at the end of the
/// outline where the body starts; on a page with no body, in a new outline there; and on a
/// page whose content lay elsewhere, on the page two grid rows below it.
#[test]
fn a_picture_pasted_from_the_title_goes_into_the_body() {
    let source = onestore::create_section("pictures.one", "First page", "Author").unwrap();
    let arena = Arena::default();
    let mut section = Section::open(&arena, source).unwrap();
    let creation = PageCreation::new(None, Some("Title paste"), "Author").unwrap();
    section
        .apply(
            "Author",
            &Edit {
                at: 133_000_000_000_000_000,
                ops: vec![Op::Section(SectionOp::Create(creation))],
            },
        )
        .unwrap();
    let (space, ..) = section.pages().unwrap()[1].clone();
    let page = section.page(space).unwrap();
    let mut engine = TextEngine::default();
    let opened = |engine: &mut TextEngine| {
        let mut editor = CanvasEditor::from_page(page.clone(), engine).unwrap();
        let title = editor.outlines().iter().find(|o| o.title).unwrap().id;
        editor.focus_outline(title).unwrap();
        editor
    };

    // No body: a new outline where it starts.
    let mut editor = opened(&mut engine);
    let start = editor.body_start().unwrap();
    editor
        .insert_picture(&mut engine, picture([150.0, 75.0]))
        .unwrap();
    assert_eq!(body(&editor), ["[150.0, 75.0]", ""]);
    assert_eq!(editor.active_outline().origin(), start);

    // A body where it starts: at its end.
    let mut editor = opened(&mut engine);
    editor.leave_title(&mut engine).unwrap();
    editor.insert(&mut engine, "body").unwrap();
    assert_eq!(editor.active_outline().origin(), start);
    let title = editor.outlines().iter().find(|o| o.title).unwrap().id;
    editor.focus_outline(title).unwrap();
    editor
        .insert_picture(&mut engine, picture([150.0, 75.0]))
        .unwrap();
    assert_eq!(body(&editor), ["body", "[150.0, 75.0]", ""]);

    // Content elsewhere: on the page two grid rows below it.
    let mut editor = opened(&mut engine);
    let margin = editor.margin_origin();
    editor
        .place_caret(&mut engine, [start[0] + 270.0, start[1] + 72.0], 468.0)
        .unwrap();
    editor.insert(&mut engine, "far").unwrap();
    let bottom = editor.active_outline().bounds().y1 as f32;
    let title = editor.outlines().iter().find(|o| o.title).unwrap().id;
    editor.focus_outline(title).unwrap();
    editor
        .insert_picture(&mut engine, picture([150.0, 75.0]))
        .unwrap();
    let placed = page_pictures(&editor).pop().unwrap();
    let row = ((bottom - margin[1]) / 18.0).floor() + 2.0;
    assert_eq!(
        [placed.layout.x, placed.layout.y],
        [Some(start[0]), Some(margin[1] + row * 18.0)]
    );
}
