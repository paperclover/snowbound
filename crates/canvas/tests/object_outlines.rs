//! An outline holding only a picture or a file edits like any other: a click beside the object
//! puts the caret in a paragraph after it, stored only once typed in, as OneNote 2010 adds one
//! (`corpus/object-outline`).

use canvas::{document::TextPosition, editor::CanvasEditor, layout::TextEngine};
use onestore::{
    Arena, Section, Store,
    op::{Edit, Op},
    page::{Page, PageObject, ParagraphContent},
};

const ALONE: &[u8] = include_bytes!("../../../corpus/object-outline/native/notebook/Alone.one");

/// The body outline's paragraphs: an object's kind, or text.
fn body(page: &Page) -> Vec<String> {
    page.objects
        .iter()
        .find_map(|object| match object {
            PageObject::Outline(outline) => Some(&outline.paragraphs),
            _ => None,
        })
        .unwrap()
        .iter()
        .map(|node| match &node.content {
            ParagraphContent::Text(text) => text.text.text().to_owned(),
            ParagraphContent::Image(_) => "picture".into(),
            ParagraphContent::Attachment(_) => "file".into(),
            _ => "other".into(),
        })
        .collect()
}

/// `SNOWBOUND_OBJECT_OUTLINE_EXPORT` names a new directory receiving the candidate for a cold
/// reopen.
#[test]
fn typing_beside_an_outlines_only_object_adds_a_paragraph() {
    let arena = Arena::default();
    let mut section = Section::open(&arena, ALONE.to_vec()).unwrap();
    let pages = section.pages().unwrap();
    let mut engine = TextEngine::default();
    for (index, (title, object)) in [("File alone", "file"), ("Picture alone", "picture")]
        .into_iter()
        .enumerate()
    {
        let space = pages.iter().find(|page| page.1 == title).unwrap().0;
        let page = section.page(space).unwrap();
        assert_eq!(body(&page), [object]);
        let outline = page
            .objects
            .iter()
            .find_map(|object| match object {
                PageObject::Outline(outline) => Some(outline.id),
                _ => None,
            })
            .unwrap();
        let mut editor = CanvasEditor::from_page(page.clone(), &mut engine).unwrap();
        assert!(editor.has_page_outline(outline));
        // The paragraph the caret stands in is not stored until typed in.
        assert_eq!(editor.page().unwrap(), page);
        editor.focus_outline(outline).unwrap();
        let at = TextPosition {
            paragraph: 0,
            offset: 0,
        };
        editor.select([at; 2].into()).unwrap();
        assert!(editor.take_ops().unwrap().is_empty());
        let mut publish = |editor: &mut CanvasEditor, step: u64| {
            let ops = editor
                .take_ops()
                .unwrap()
                .into_iter()
                .map(|op| Op::Page { space, op })
                .collect();
            let at = 134_000_000_000_000_000 + (index as u64 * 4 + step) * 10_000_000;
            section.apply("Author", &Edit { at, ops }).unwrap();
            body(&section.page(space).unwrap())
        };
        editor.insert(&mut engine, "abc").unwrap();
        assert_eq!(publish(&mut editor, 0), [object, "abc"]);
        // Undoing the typing removes the paragraph again, as OneNote 2010's undo does.
        assert!(editor.undo(&mut engine).unwrap());
        assert_eq!(publish(&mut editor, 1), [object]);
        assert!(editor.redo(&mut engine).unwrap());
        assert_eq!(publish(&mut editor, 2), [object, "abc"]);
    }
    section.seal().unwrap();
    let written = section.image();
    if let Some(directory) = std::env::var_os("SNOWBOUND_OBJECT_OUTLINE_EXPORT") {
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(directory.join("Alone.one"), &written).unwrap();
        let file_id = Store::parse(&written).unwrap().header.file_id;
        std::fs::write(
            directory.join("Open Notebook.onetoc2"),
            onestore::create_table_of_contents("Open Notebook.onetoc2", &[("Alone.one", file_id)])
                .unwrap(),
        )
        .unwrap();
    }
}
