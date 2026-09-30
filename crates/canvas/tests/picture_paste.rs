//! Pictures pasted or inserted through the editor, placed as OneNote 2010 places them (lab,
//! 2026-09-30): at a caret in text the paragraph splits around the picture, and at a caret
//! on blank page the picture lies on the page there with the caret on the grid row below.
//! Each is one edit and one undo step. `CANVAS_PICTURE_EXPORT` names a directory receiving
//! the section as a notebook, for a cold reopen in OneNote 2010.

use canvas::{
    document::TextPosition,
    editor::{CanvasEditor, Selection},
    layout::TextEngine,
};
use onestore::{
    Arena, Section, Store,
    document::Layout,
    op::{Edit, Op},
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
