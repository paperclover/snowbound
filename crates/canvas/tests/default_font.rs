//! OneNote's Default font as the editor stores it: a new page's title in its face and colour,
//! and new text in the page's `p` quick style in its face, size and colour. `CANVAS_DEFAULT_FONT_EXPORT`
//! names a directory receiving the section as a notebook, for a cold reopen in OneNote 2010
//! (`corpus/default-font`).

use canvas::{
    editor::{CanvasEditor, DefaultFont},
    layout::TextEngine,
};
use onestore::{
    Arena, PageCreation, Section, Store,
    document::Kind,
    op::{Edit, Op, SectionOp},
    page::{PageObject, ParagraphContent},
};

/// Office's standard Blue, COLORREF, which OneNote 2010 stores as `DefaultFontColor`.
const BLUE: u32 = 0xc07000;

#[test]
fn new_pages_and_text_take_the_default_font() {
    let source = onestore::create_section("fonts.one", "Calibri page", "Author").unwrap();
    let arena = Arena::default();
    let mut section = Section::open(&arena, source.clone()).unwrap();
    let creation = PageCreation::new(None, Some("Georgia title"), "Author")
        .unwrap()
        .titled_in("Georgia", Some(BLUE))
        .unwrap();
    let mut at = 133_000_000_000_000_000;
    section
        .apply(
            "Author",
            &Edit {
                at,
                ops: vec![Op::Section(SectionOp::Create(creation))],
            },
        )
        .unwrap();
    let (space, ..) = section.pages().unwrap()[1].clone();
    let mut engine = TextEngine::default();
    let mut editor = CanvasEditor::from_page(section.page(space).unwrap(), &mut engine).unwrap();
    editor.default_font = DefaultFont {
        face: "Georgia".into(),
        size: 14.0,
        color: Some(BLUE),
    };
    for (text, y) in [("Georgia body", 120.0), ("Georgia again", 200.0)] {
        editor.place_caret(&mut engine, [72.0, y], 240.0).unwrap();
        editor.insert(&mut engine, text).unwrap();
        let ops = editor.take_ops().unwrap();
        at += 10_000_000;
        let ops = ops.into_iter().map(|op| Op::Page { space, op }).collect();
        section.apply("Author", &Edit { at, ops }).unwrap();
    }
    let mut image = source;
    section.seal().unwrap().unwrap().apply(&mut image).unwrap();

    let arena = Arena::default();
    let reopened = Section::open(&arena, image.clone()).unwrap();
    let page = reopened.page(space).unwrap();
    let style = |id| {
        let definition = &page.definitions[&id];
        let Kind::Style { name, .. } = &definition.kind else {
            panic!("{definition:?}")
        };
        (
            name.as_deref().unwrap_or_default().to_owned(),
            definition.format.font.clone().unwrap_or_default(),
            definition.format.font_size,
            definition.format.color,
        )
    };
    let title = page
        .objects
        .iter()
        .find_map(|object| match object {
            PageObject::Title(title) => Some(title),
            _ => None,
        })
        .unwrap();
    let titled = title
        .outlines
        .iter()
        .flat_map(|outline| &outline.paragraphs)
        .filter_map(|paragraph| paragraph.style)
        .map(style)
        .find(|(name, ..)| name == "PageTitle");
    assert_eq!(
        titled,
        Some(("PageTitle".into(), "Georgia".into(), Some(17.0), Some(BLUE)))
    );
    let bodies: Vec<_> = page
        .objects
        .iter()
        .filter_map(|object| match object {
            PageObject::Outline(outline) if !outline.title => Some(&outline.paragraphs[0]),
            _ => None,
        })
        .collect();
    assert_eq!(bodies.len(), 2);
    for body in &bodies {
        assert!(matches!(body.content, ParagraphContent::Text(_)));
        assert_eq!(
            style(body.style.unwrap()),
            ("p".into(), "Georgia".into(), Some(14.0), Some(BLUE))
        );
    }
    assert_eq!(bodies[0].style, bodies[1].style, "one p per font");

    if let Some(directory) = std::env::var_os("CANVAS_DEFAULT_FONT_EXPORT") {
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("fonts.one"), &image).unwrap();
        let file_id = Store::parse(&image).unwrap().header.file_id;
        std::fs::write(
            directory.join("Open Notebook.onetoc2"),
            onestore::create_table_of_contents("Open Notebook.onetoc2", &[("fonts.one", file_id)])
                .unwrap(),
        )
        .unwrap();
    }
}
