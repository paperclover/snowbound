//! Make To-Do List trades bulleted and numbered paragraphs' lists for the To Do tag, and Make
//! Bulleted List trades it back, each published as one revision (`corpus/to-do-list`).

use canvas::{
    document::TextPosition,
    editor::{CanvasEditor, Formatting, NoteTag},
    layout::TextEngine,
};
use onestore::{
    Arena, ExGuid, Section, Store,
    op::{Edit, Op},
    page::{Page, PageObject},
};

const LISTS: &[u8] = include_bytes!("../../../corpus/list-edit/cold/notebook/lists.one");

fn body(page: &Page) -> ExGuid {
    page.objects
        .iter()
        .find_map(|object| match object {
            PageObject::Outline(outline) => Some(outline.id),
            _ => None,
        })
        .unwrap()
}

fn at(paragraph: usize) -> TextPosition {
    TextPosition {
        paragraph,
        offset: 0,
    }
}

/// Each paragraph's list, as `bullet`, `number` or none, and its tags' labels.
fn items(page: &Page, engine: &mut TextEngine) -> Vec<(Option<&'static str>, Vec<String>)> {
    let mut editor = CanvasEditor::from_page(page.clone(), engine).unwrap();
    editor.focus_outline(body(page)).unwrap();
    (0..7)
        .map(|paragraph| {
            editor.select([at(paragraph); 2].into()).unwrap();
            let state = editor.format_state().unwrap();
            let list = match (state.bullets, state.numbering) {
                (true, _) => Some("bullet"),
                (_, true) => Some("number"),
                _ => None,
            };
            (
                list,
                state.tags.into_iter().map(|(tag, _)| tag.label).collect(),
            )
        })
        .collect()
}

/// `SNOWBOUND_TO_DO_LIST_EXPORT` names a new directory receiving the candidate for a cold
/// reopen.
#[test]
fn lists_become_to_do_lists_and_back_one_revision_each() {
    let arena = Arena::default();
    let mut section = Section::open(&arena, LISTS.to_vec()).unwrap();
    let space = section.pages().unwrap()[0].0;
    let page = section.page(space).unwrap();
    let mut engine = TextEngine::default();
    let mut editor = CanvasEditor::from_page(page.clone(), &mut engine).unwrap();
    editor.focus_outline(body(&page)).unwrap();
    let to_do = NoteTag::defaults()[0].clone();
    let mut publish = |editor: &mut CanvasEditor, at| {
        let ops = editor
            .take_ops()
            .unwrap()
            .into_iter()
            .map(|op| Op::Page { space, op })
            .collect();
        section.apply("Author", &Edit { at, ops }).unwrap();
    };
    // Through "Nested numbered", the last paragraph each edit takes.
    let nested = TextPosition {
        paragraph: 3,
        offset: 15,
    };
    editor.select([at(0), nested].into()).unwrap();
    editor
        .format(&mut engine, Formatting::ToDoList(to_do.clone(), 0))
        .unwrap();
    publish(&mut editor, 134_000_000_000_000_000);
    editor.select([at(2), nested].into()).unwrap();
    editor
        .format(&mut engine, Formatting::BulletedList(to_do, 0))
        .unwrap();
    publish(&mut editor, 134_000_000_010_000_000);

    let task = || vec!["To Do".to_owned()];
    let expected = [
        (None, task()),
        (None, task()),
        (Some("bullet"), vec![]),
        (Some("bullet"), vec![]),
        (Some("number"), vec![]),
        (None, vec![]),
        (None, vec![]),
    ];
    assert_eq!(items(&section.page(space).unwrap(), &mut engine), expected);
    section.seal().unwrap();
    let written = section.image();
    let reread = Section::open(&arena, written.clone()).unwrap();
    assert_eq!(items(&reread.page(space).unwrap(), &mut engine), expected);
    if let Some(directory) = std::env::var_os("SNOWBOUND_TO_DO_LIST_EXPORT") {
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(directory.join("lists.one"), &written).unwrap();
        let file_id = Store::parse(&written).unwrap().header.file_id;
        std::fs::write(
            directory.join("Open Notebook.onetoc2"),
            onestore::create_table_of_contents("Open Notebook.onetoc2", &[("lists.one", file_id)])
                .unwrap(),
        )
        .unwrap();
    }
}
