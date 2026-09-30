//! Table columns fit their content as it is typed and lock where a border is dragged, each
//! keystroke's width stored in the revision with its text, and OneNote 2010 reads them back
//! as it sizes them itself (`corpus/table-widths`).

use canvas::{document::TextPosition, editor::CanvasEditor, layout::TextEngine};
use onestore::{
    Arena, ExGuid, Section, Store,
    op::{Edit, Op},
    page::{Page, PageObject, PageParagraph, ParagraphContent, TableColumn},
};

const TABLES: &[u8] = include_bytes!("../../../corpus/table-tags/native/notebook/Tables.one");

/// Each table's first cell text and columns, in page order.
fn tables(page: &Page) -> Vec<(String, Vec<TableColumn>)> {
    fn walk(nodes: &[PageParagraph], out: &mut Vec<(String, Vec<TableColumn>)>) {
        for node in nodes {
            if let ParagraphContent::Table(table) = &node.content {
                let first = table.rows[0].cells[0].paragraphs[0]
                    .text()
                    .map_or(String::new(), |text| text.text.text().to_owned());
                out.push((first, table.columns.clone()));
                for cell in table.rows.iter().flat_map(|row| &row.cells) {
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
            PageObject::Outline(outline) if !outline.paragraphs.is_empty() => Some(outline.id),
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

/// Opens `space` with the caret at the end of its body's last paragraph.
fn at_end(section: &Section<'_>, space: ExGuid, engine: &mut TextEngine) -> CanvasEditor {
    let page = section.page(space).unwrap();
    let mut editor = CanvasEditor::from_page(page.clone(), engine).unwrap();
    editor.focus_outline(body(&page)).unwrap();
    let document = editor.active_outline().document();
    let paragraph = document.paragraphs().count() - 1;
    let offset = document
        .paragraphs()
        .last()
        .unwrap()
        .text()
        .encode_utf16()
        .count() as u32;
    editor
        .select([TextPosition { paragraph, offset }; 2].into())
        .unwrap();
    editor
}

fn type_rows(editor: &mut CanvasEditor, engine: &mut TextEngine, rows: &[&[&str]]) {
    for (index, row) in rows.iter().enumerate() {
        if index > 0 {
            editor.enter(engine, false).unwrap();
        }
        for (column, text) in row.iter().enumerate() {
            if column > 0 {
                editor.tab(engine, false).unwrap();
            }
            editor.insert(engine, text).unwrap();
        }
    }
}

fn close(columns: &[TableColumn], expected: &[(f32, bool)]) -> bool {
    columns.len() == expected.len()
        && columns
            .iter()
            .zip(expected)
            .all(|(column, (width, locked))| {
                (column.width - width).abs() < 0.01 && column.locked == *locked
            })
}

/// `SNOWBOUND_TABLE_WIDTHS_EXPORT` names a new directory receiving the candidate for a cold
/// reopen.
#[test]
fn typed_and_dragged_columns_store_onenote_s_widths() {
    let arena = Arena::default();
    let mut section = Section::open(&arena, TABLES.to_vec()).unwrap();
    let pages = section.pages().unwrap();
    let space = |title: &str| pages.iter().find(|page| page.1 == title).unwrap().0;
    let (fruit, typed, dragged) = (
        space("Check a table"),
        space("Edit a tagged table"),
        space("Delete and undo"),
    );
    let mut engine = TextEngine::default();
    let mut at = 134_000_000_000_000_000;
    let mut step = |section: &mut Section<'_>, space, editor: &mut CanvasEditor| {
        at += 10_000_000;
        publish(section, space, editor, at);
    };

    // A table typed after the page's text, one revision per keystroke.
    let mut editor = at_end(&section, fruit, &mut engine);
    editor.enter(&mut engine, false).unwrap();
    editor.insert_table(&mut engine, 1, 1).unwrap();
    step(&mut section, fruit, &mut editor);
    for (index, row) in [
        ["Fruit", "Colour", "Notes"],
        ["Apple", "Red", "Crisp and sweet, good for pies"],
        ["Watermelon", "Green", "Summer"],
    ]
    .iter()
    .enumerate()
    {
        if index > 0 {
            editor.enter(&mut engine, false).unwrap();
            step(&mut section, fruit, &mut editor);
        }
        for (column, text) in row.iter().enumerate() {
            if column > 0 {
                editor.tab(&mut engine, false).unwrap();
                step(&mut section, fruit, &mut editor);
            }
            for character in text.chars() {
                editor.insert(&mut engine, &character.to_string()).unwrap();
                step(&mut section, fruit, &mut editor);
            }
        }
    }
    let stored = tables(&section.page(fruit).unwrap());
    // OneNote 2010 typing the same table (lab, 2026-09-30).
    assert!(
        close(
            &stored[1].1,
            &[(60.759, false), (37.11, false), (139.086, false)]
        ),
        "{stored:?}"
    );

    // Typing into a native table's cell widens its column; deleting narrows it again.
    let mut editor = CanvasEditor::from_page(section.page(typed).unwrap(), &mut engine).unwrap();
    editor
        .focus_outline(body(&section.page(typed).unwrap()))
        .unwrap();
    editor
        .select(
            [TextPosition {
                paragraph: 1,
                offset: 10,
            }; 2]
                .into(),
        )
        .unwrap();
    editor
        .insert(&mut engine, " and typed on in Snowbound")
        .unwrap();
    step(&mut section, typed, &mut editor);
    let widened = tables(&section.page(typed).unwrap());
    assert!(widened[0].1[0].width > 100.0, "{widened:?}");

    // A dragged column locks and keeps its width while text wraps in it, and the column
    // after it moves; a long line in that column stops at the outline's width.
    let mut editor = at_end(&section, dragged, &mut engine);
    editor.enter(&mut engine, false).unwrap();
    editor.insert_table(&mut engine, 1, 1).unwrap();
    type_rows(&mut editor, &mut engine, &[&["Dragged", "Fitted"]]);
    step(&mut section, dragged, &mut editor);
    let table = editor
        .active_outline()
        .document()
        .nodes()
        .iter()
        .find_map(|node| match &node.content {
            ParagraphContent::Table(table)
                if table.rows[0].cells[0].paragraphs[0]
                    .text()
                    .is_some_and(|text| text.text.text() == "Dragged") =>
            {
                Some(table.id)
            }
            _ => None,
        })
        .unwrap();
    editor.resize_column(&mut engine, table, 0, 96.0).unwrap();
    step(&mut section, dragged, &mut editor);
    editor
        .insert(
            &mut engine,
            " and then a line long enough to reach the outline's edge and wrap there",
        )
        .unwrap();
    step(&mut section, dragged, &mut editor);
    let resized = tables(&section.page(dragged).unwrap());
    assert!(
        resized[2].1[0].locked && resized[2].1[0].width == 96.0,
        "{resized:?}"
    );
    assert!(
        !resized[2].1[1].locked && resized[2].1[1].width > 200.0,
        "{resized:?}"
    );

    section.seal().unwrap();
    let written = section.image();
    let reread = Section::open(&arena, written.clone()).unwrap();
    assert_eq!(tables(&reread.page(fruit).unwrap()), stored);
    assert_eq!(tables(&reread.page(typed).unwrap()), widened);
    assert_eq!(tables(&reread.page(dragged).unwrap()), resized);
    if let Some(directory) = std::env::var_os("SNOWBOUND_TABLE_WIDTHS_EXPORT") {
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
