//! Tables carrying note tags edit like any other: the tag draws in the tag column centred on
//! the table, a click checks its box, typing in a cell keeps the table's tags, and deleting a
//! table and undoing brings its tags back (`corpus/table-tags`).

use canvas::{
    document::TextPosition,
    editor::{CanvasEditor, Whole},
    layout::TextEngine,
};
use onestore::{
    Arena, ExGuid, Section, Store,
    document::Tag,
    op::{Edit, Op},
    page::{Page, PageObject, PageParagraph, ParagraphContent},
};

const TABLES: &[u8] = include_bytes!("../../../corpus/table-tags/native/notebook/Tables.one");

/// Each table's cell texts and tags, their arena positions cleared, in page order.
fn tables(page: &Page) -> Vec<(Vec<String>, Vec<Tag>)> {
    fn walk(nodes: &[PageParagraph], out: &mut Vec<(Vec<String>, Vec<Tag>)>) {
        for node in nodes {
            if let ParagraphContent::Table(table) = &node.content {
                let cells = table.rows.iter().flat_map(|row| &row.cells);
                let texts = cells
                    .clone()
                    .flat_map(|cell| &cell.paragraphs)
                    .filter_map(|paragraph| Some(paragraph.text()?.text.text().to_owned()))
                    .collect();
                let tags = node
                    .tags
                    .iter()
                    .chain(&table.tags)
                    .map(|tag| Tag {
                        extra_set: 0,
                        ..tag.clone()
                    })
                    .collect();
                out.push((texts, tags));
                for cell in cells {
                    walk(&cell.paragraphs, out);
                }
            }
        }
    }
    let mut out = Vec::new();
    for object in &page.objects {
        if let PageObject::Outline(outline) = object {
            walk(&outline.paragraphs, &mut out);
        }
    }
    out
}

fn body(page: &Page) -> ExGuid {
    page.objects
        .iter()
        .find_map(|object| match object {
            PageObject::Outline(outline) => Some(outline.id),
            _ => None,
        })
        .unwrap()
}

fn publish(section: &mut Section<'_>, space: ExGuid, editor: &mut CanvasEditor, at: u64) {
    let ops = editor
        .take_ops()
        .unwrap()
        .into_iter()
        .map(|op| Op::Page { space, op })
        .collect();
    section.apply("Author", &Edit { at, ops }).unwrap();
}

/// `SNOWBOUND_TABLE_TAGS_EXPORT` names a new directory receiving the candidate for a cold
/// reopen.
#[test]
fn tagged_tables_draw_check_and_survive_edits() {
    let arena = Arena::default();
    let mut section = Section::open(&arena, TABLES.to_vec()).unwrap();
    let pages = section.pages().unwrap();
    let space = |title: &str| pages.iter().find(|page| page.1 == title).unwrap().0;
    let (check, edit, restore) = (
        space("Check a table"),
        space("Edit a tagged table"),
        space("Delete and undo"),
    );
    let mut engine = TextEngine::default();

    let page = section.page(check).unwrap();
    let [(_, tags)] = &tables(&page)[..] else {
        panic!("one table")
    };
    assert_eq!(tags.len(), 1);
    assert_eq!(tags[0].status & 1, 0);
    let mut editor = CanvasEditor::from_page(page.clone(), &mut engine).unwrap();
    assert!(editor.has_page_outline(body(&page)));
    let outline = editor
        .outlines()
        .iter()
        .find(|outline| outline.id == body(&page))
        .unwrap();
    let holder = outline.document().nodes()[1].id;
    let [(tagged, origin, tag)] = &outline.shaped().tags().collect::<Vec<_>>()[..] else {
        panic!("one tag drawn")
    };
    assert_eq!(*tagged, holder);
    // Centred on the table, in the column left of it.
    let table = &outline.shaped().tables[0];
    let [top, bottom] = [table.cells[0].rect[1], table.cells[0].rect[3]];
    assert!((origin[1] + tag.size / 2.0 - (top + bottom) / 2.0).abs() < 2.0);
    assert!(origin[0] + tag.size < table.cells[0].rect[0]);
    editor
        .click_check(&mut engine, body(&page), holder)
        .unwrap();
    publish(&mut section, check, &mut editor, 134_000_000_000_000_000);
    let checked = tables(&section.page(check).unwrap());
    assert_eq!(checked[0].1[0].status & 1, 1);
    assert!(checked[0].1[0].completed.is_some_and(|time| time > 0));

    let page = section.page(edit).unwrap();
    let before = tables(&page);
    let mut editor = CanvasEditor::from_page(page.clone(), &mut engine).unwrap();
    editor.focus_outline(body(&page)).unwrap();
    let at = TextPosition {
        paragraph: 1,
        offset: 0,
    };
    editor.select([at; 2].into()).unwrap();
    editor.insert(&mut engine, "Now ").unwrap();
    editor.enter(&mut engine, false).unwrap();
    publish(&mut section, edit, &mut editor, 134_000_000_010_000_000);
    let after = tables(&section.page(edit).unwrap());
    assert_eq!(after[0].0[..2], ["Now ", "Typed into"]);
    assert_eq!(after[0].1, before[0].1);

    let original = section.page(restore).unwrap();
    let kept = tables(&original);
    assert_eq!(kept.len(), 2);
    let mut editor = CanvasEditor::from_page(original.clone(), &mut engine).unwrap();
    editor.focus_outline(body(&original)).unwrap();
    while editor.whole() != Some(Whole::Page) {
        editor.widen_selection().unwrap();
    }
    editor.delete(&mut engine, false).unwrap();
    publish(&mut section, restore, &mut editor, 134_000_000_020_000_000);
    assert!(tables(&section.page(restore).unwrap()).is_empty());
    assert!(editor.undo(&mut engine).unwrap());
    publish(&mut section, restore, &mut editor, 134_000_000_030_000_000);
    assert_eq!(tables(&section.page(restore).unwrap()), kept);

    section.seal().unwrap();
    let written = section.image();
    let reread = Section::open(&arena, written.clone()).unwrap();
    assert_eq!(tables(&reread.page(check).unwrap()), checked);
    assert_eq!(tables(&reread.page(edit).unwrap()), after);
    assert_eq!(tables(&reread.page(restore).unwrap()), kept);
    if let Some(directory) = std::env::var_os("SNOWBOUND_TABLE_TAGS_EXPORT") {
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(directory.join("Tables.one"), &written).unwrap();
        let file_id = Store::parse(&written).unwrap().header.file_id;
        std::fs::write(
            directory.join("Open Notebook.onetoc2"),
            onestore::create_table_of_contents("Open Notebook.onetoc2", &[("Tables.one", file_id)])
                .unwrap(),
        )
        .unwrap();
    }
}
