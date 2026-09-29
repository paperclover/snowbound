//! Outlook task tags stay read-only through the editor: Remove Tag drops one, removing a
//! normal tag beside one keeps it, and deleting a page and undoing restores them as OneNote
//! 2010 stored them (`corpus/task-tags`).

use canvas::{
    document::TextPosition,
    editor::{CanvasEditor, Formatting, NoteTag, Whole},
    layout::TextEngine,
};
use onestore::{
    Arena, ExGuid, Section, Store,
    document::Tag,
    op::{Edit, Op},
    page::{Page, PageObject},
};

const TASKS: &[u8] = include_bytes!("../../../corpus/task-tags/native/notebook/Tasks.one");

/// Each body paragraph's text and tags, their arena positions cleared.
fn tagged(page: &Page) -> Vec<(String, Vec<Tag>)> {
    page.objects
        .iter()
        .find_map(|object| match object {
            PageObject::Outline(outline) if !outline.title => Some(&outline.paragraphs),
            _ => None,
        })
        .unwrap()
        .iter()
        .map(|paragraph| {
            let text = paragraph.text().unwrap();
            let tags = paragraph
                .tags
                .iter()
                .chain(&text.tags)
                .map(|tag| Tag {
                    extra_set: 0,
                    ..tag.clone()
                })
                .collect();
            (text.text.text().to_owned(), tags)
        })
        .collect()
}

fn body(editor: &mut CanvasEditor) {
    let body = editor
        .outlines()
        .iter()
        .find(|outline| !outline.title)
        .unwrap()
        .id;
    editor.focus_outline(body).unwrap();
}

fn at(paragraph: usize) -> canvas::editor::Selection {
    [TextPosition {
        paragraph,
        offset: 0,
    }; 2]
        .into()
}

/// Applies the editor's pending ops to the page in `space` as one edit.
fn publish(section: &mut Section<'_>, space: ExGuid, editor: &mut CanvasEditor, at: u64) {
    let ops = editor
        .take_ops()
        .unwrap()
        .into_iter()
        .map(|op| Op::Page { space, op })
        .collect();
    section.apply("Author", &Edit { at, ops }).unwrap();
}

/// `SNOWBOUND_TASK_TAGS_EXPORT` names a new directory receiving the candidate for a cold
/// reopen.
#[test]
fn task_tags_survive_tag_removal_and_page_delete_undo() {
    let arena = Arena::default();
    let mut section = Section::open(&arena, TASKS.to_vec()).unwrap();
    let pages = section.pages().unwrap();
    let space = |title: &str| pages.iter().find(|page| page.1 == title).unwrap().0;
    let (remove, restore) = (space("Remove a task"), space("Delete and undo"));
    let before = tagged(&section.page(remove).unwrap());
    let shapes: Vec<_> = before
        .iter()
        .flat_map(|(_, tags)| tags)
        .map(|tag| (tag.definition.is_some(), tag.status & 4 != 0, tag.shape))
        .collect();
    assert_eq!(
        shapes,
        [
            (false, true, Some(89)),
            (false, true, Some(89)),
            (false, true, Some(89)),
            (true, false, None),
        ]
    );
    let mut engine = TextEngine::default();

    let mut editor = CanvasEditor::from_page(section.page(remove).unwrap(), &mut engine).unwrap();
    body(&mut editor);
    editor.select(at(0)).unwrap();
    editor.format(&mut engine, Formatting::RemoveTags).unwrap();
    editor.select(at(2)).unwrap();
    let to_do = NoteTag::defaults()[0].clone();
    editor
        .format(&mut engine, Formatting::Tag(to_do, 0))
        .unwrap();
    publish(&mut section, remove, &mut editor, 134_000_000_000_000_000);
    let mut expected = before.clone();
    expected[0].1.clear();
    expected[2].1.retain(|tag| tag.definition.is_none());
    assert_eq!(tagged(&section.page(remove).unwrap()), expected);

    let original = section.page(restore).unwrap();
    let kept = tagged(&original);
    let mut editor = CanvasEditor::from_page(original, &mut engine).unwrap();
    body(&mut editor);
    while editor.whole() != Some(Whole::Page) {
        editor.widen_selection().unwrap();
    }
    editor.delete(&mut engine, false).unwrap();
    publish(&mut section, restore, &mut editor, 134_000_000_010_000_000);
    assert!(editor.undo(&mut engine).unwrap());
    publish(&mut section, restore, &mut editor, 134_000_000_020_000_000);
    assert_eq!(tagged(&section.page(restore).unwrap()), kept);

    section.seal().unwrap();
    let written = section.image();
    let reread = Section::open(&arena, written.clone()).unwrap();
    assert_eq!(tagged(&reread.page(remove).unwrap()), expected);
    assert_eq!(tagged(&reread.page(restore).unwrap()), kept);
    if let Some(directory) = std::env::var_os("SNOWBOUND_TASK_TAGS_EXPORT") {
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(directory.join("Tasks.one"), &written).unwrap();
        let file_id = Store::parse(&written).unwrap().header.file_id;
        std::fs::write(
            directory.join("Open Notebook.onetoc2"),
            onestore::create_table_of_contents("Open Notebook.onetoc2", &[("Tasks.one", file_id)])
                .unwrap(),
        )
        .unwrap();
    }
}
