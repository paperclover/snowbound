//! Deleting or typing over a selection that crosses a table's edge does what OneNote 2010 did
//! with the same keys (`tools/native_cross_container.py`, `corpus/cross-container`): nothing
//! joins across the edge, cells inside are emptied, and where the selection runs on past the
//! table its rows inside go, the table with them when all do.

use canvas::{document::TextPosition, editor::CanvasEditor, layout::TextEngine};
use onestore::{
    Arena, ExGuid, Section, Store,
    op::{Edit, Op},
    page::{Page, PageObject, PageParagraph, ParagraphContent},
};

const CROSS: &[u8] = include_bytes!("../../../corpus/cross-container/native/notebook/Cross.one");

/// The body's paragraphs in order: a text, or a table's rows of cell texts.
#[derive(Debug, PartialEq)]
enum Block {
    Text(String),
    Table(Vec<Vec<String>>),
}

fn blocks(page: &Page) -> Vec<Block> {
    let text = |node: &PageParagraph| node.text().unwrap().text.text().to_owned();
    page.objects
        .iter()
        .find_map(|object| match object {
            PageObject::Outline(outline) => Some(&outline.paragraphs),
            _ => None,
        })
        .unwrap()
        .iter()
        .map(|node| match &node.content {
            ParagraphContent::Table(table) => Block::Table(
                table
                    .rows
                    .iter()
                    .map(|row| {
                        row.cells
                            .iter()
                            .map(|cell| cell.paragraphs.iter().map(text).collect())
                            .collect()
                    })
                    .collect(),
            ),
            _ => Block::Text(text(node)),
        })
        .collect()
}

fn at(paragraph: usize, offset: u32) -> TextPosition {
    TextPosition { paragraph, offset }
}

#[test]
fn deleting_across_a_tables_edge_keeps_each_side() {
    let arena = Arena::default();
    let mut section = Section::open(&arena, CROSS.to_vec()).unwrap();
    let pages = section.pages().unwrap();
    let mut engine = TextEngine::default();
    let text = |value: &str| Block::Text(value.into());
    let table = |rows: &[[&str; 2]]| {
        Block::Table(
            rows.iter()
                .map(|row| row.iter().map(|cell| cell.to_string()).collect())
                .collect(),
        )
    };
    // Each page's selection, whether Delete is pressed, and what is typed then.
    let cases = [
        (
            "Delete into a table",
            [at(0, 6), at(1, 5)],
            true,
            "",
            vec![
                text("Before"),
                table(&[[" one", "Alpha two"], ["Beta one", "Beta two"]]),
                text("After text"),
            ],
        ),
        (
            "Delete out of a table",
            [at(2, 5), at(5, 5)],
            true,
            "X",
            vec![
                text("Before text"),
                table(&[["Alpha one", "AlphaX"]]),
                text(" text"),
            ],
        ),
        (
            "Type over a row",
            [at(0, 6), at(3, 4)],
            false,
            "Y",
            vec![
                text("BeforeY"),
                table(&[["", ""], [" one", "Beta two"]]),
                text("After text"),
            ],
        ),
        (
            "Delete a whole table",
            [at(0, 6), at(5, 5)],
            true,
            "",
            vec![text("Before"), text(" text")],
        ),
    ];
    let mut expected = Vec::new();
    for (index, (title, selection, delete, typed, blocks_after)) in cases.into_iter().enumerate() {
        let space: ExGuid = pages.iter().find(|page| page.1 == title).unwrap().0;
        let page = section.page(space).unwrap();
        let body = page
            .objects
            .iter()
            .find_map(|object| match object {
                PageObject::Outline(outline) => Some(outline.id),
                _ => None,
            })
            .unwrap();
        let mut editor = CanvasEditor::from_page(page, &mut engine).unwrap();
        editor.focus_outline(body).unwrap();
        editor.select(selection.into()).unwrap();
        if delete {
            editor.delete(&mut engine, false).unwrap();
        }
        if !typed.is_empty() {
            editor.insert(&mut engine, typed).unwrap();
        }
        assert_eq!(
            editor.selection().positions[1].paragraph,
            selection[0].paragraph
        );
        let ops = editor
            .take_ops()
            .unwrap()
            .into_iter()
            .map(|op| Op::Page { space, op })
            .collect();
        let at = 134_000_000_000_000_000 + index as u64 * 10_000_000;
        section.apply("Author", &Edit { at, ops }).unwrap();
        assert_eq!(
            blocks(&section.page(space).unwrap()),
            blocks_after,
            "{title}"
        );
        expected.push((space, blocks_after));
    }
    section.seal().unwrap();
    let written = section.image();
    let reread = Section::open(&arena, written.clone()).unwrap();
    for (space, blocks_after) in &expected {
        assert_eq!(blocks(&reread.page(*space).unwrap()), *blocks_after);
    }
    if let Some(directory) = std::env::var_os("SNOWBOUND_CROSS_CONTAINER_EXPORT") {
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(directory.join("Cross.one"), &written).unwrap();
        let file_id = Store::parse(&written).unwrap().header.file_id;
        std::fs::write(
            directory.join("Open Notebook.onetoc2"),
            onestore::create_table_of_contents("Open Notebook.onetoc2", &[("Cross.one", file_id)])
                .unwrap(),
        )
        .unwrap();
    }
}
