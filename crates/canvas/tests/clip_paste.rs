//! Formatted content pasted through the editor and stored: what OneNote 2010 copied as HTML
//! (`corpus/clipboard`), then that outline copied in Snowbound's own format and pasted again
//! elsewhere on the page, each one edit that the section stores and reads back.
//! `CANVAS_CLIP_EXPORT` names a directory receiving the section as a notebook, for a cold
//! reopen in OneNote 2010.

use canvas::{
    editor::{CanvasEditor, Clip, Piece, html_pieces},
    layout::TextEngine,
};
use onestore::{
    Arena, Section, Store,
    document::Kind,
    op::{Edit, Op},
    page::{Outline, Page, PageObject, PageParagraph, ParagraphContent},
};

const ONENOTE: &str = include_str!("../../../corpus/clipboard/onenote-2010.html");

/// Each paragraph of a body outline: its level, list kind, text and bold runs.
fn outline(page: &Page, outline: &Outline) -> Vec<String> {
    fn lines(page: &Page, nodes: &[PageParagraph], out: &mut Vec<String>) {
        for node in nodes {
            let list = match node.lists.last().map(|id| &page.definitions[id].kind) {
                Some(Kind::List {
                    bullet: Some(_), ..
                }) => "• ",
                Some(Kind::List { .. }) => "# ",
                _ => "",
            };
            let indent = "  ".repeat(node.level as usize - 1);
            match &node.content {
                ParagraphContent::Text(text) => {
                    let shown = text.text.project().unwrap();
                    let bold = shown
                        .text()
                        .spans()
                        .iter()
                        .any(|span| span.format.bold == Some(true));
                    let bold = if bold { " (bold)" } else { "" };
                    out.push(format!("{indent}{list}{}{bold}", shown.text().text()));
                }
                ParagraphContent::Table(table) => {
                    out.push(format!("{indent}table"));
                    for cell in table.rows.iter().flat_map(|row| &row.cells) {
                        lines(page, &cell.paragraphs, out);
                    }
                }
                _ => out.push(format!("{indent}other")),
            }
        }
    }
    let mut out = Vec::new();
    lines(page, &outline.paragraphs, &mut out);
    out
}

fn bodies(page: &Page) -> Vec<Vec<String>> {
    page.objects
        .iter()
        .filter_map(|object| match object {
            PageObject::Outline(body) if !body.title => Some(outline(page, body)),
            _ => None,
        })
        .collect()
}

#[test]
fn onenotes_html_and_snowbounds_own_copy_paste_and_store() {
    let source = onestore::create_section("clips.one", "Pasted clips", "Author").unwrap();
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

    editor
        .place_caret(&mut engine, [72.0, 120.0], 400.0)
        .unwrap();
    let pieces = html_pieces(ONENOTE, 0x409, |_, _| None);
    assert!(matches!(pieces[..], [Piece::Clip(_)]));
    editor.paste_pieces(&mut engine, pieces).unwrap();
    store(&mut editor);

    editor.select_all().unwrap();
    let copied = editor.clip().unwrap().unwrap();
    let copied = Clip::decode(&copied.encode()).unwrap();
    editor
        .place_caret(&mut engine, [72.0, 500.0], 400.0)
        .unwrap();
    editor.paste_clip(&mut engine, copied).unwrap();
    store(&mut editor);

    let mut image = source;
    section.seal().unwrap().unwrap().apply(&mut image).unwrap();
    let arena = Arena::default();
    let reopened = Section::open(&arena, image.clone()).unwrap();
    let page = reopened.page(space).unwrap();
    let pasted = [
        "Bold line (bold)",
        "plain italic red big times under hi link",
        "• bullet one",
        "  • nested bullet",
        "• bullet two",
        "# number one",
        "# number two",
        "tagged todo",
        "after",
        "table",
        "a1",
        "b1 (bold)",
        "a2",
        "b2",
        "last line",
    ];
    assert_eq!(
        bodies(&page),
        [vec!["Pasted clips"], pasted.to_vec(), pasted.to_vec()]
    );
    assert_eq!(bodies(&page), bodies(&editor.page().unwrap()));

    if let Some(directory) = std::env::var_os("CANVAS_CLIP_EXPORT") {
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("clips.one"), &image).unwrap();
        let file_id = Store::parse(&image).unwrap().header.file_id;
        std::fs::write(
            directory.join("Open Notebook.onetoc2"),
            onestore::create_table_of_contents("Open Notebook.onetoc2", &[("clips.one", file_id)])
                .unwrap(),
        )
        .unwrap();
    }
}

/// A title takes a clip's text alone, as OneNote 2010's does (lab, 2026-10-02).
#[test]
fn a_title_takes_the_text_alone() {
    let source = onestore::create_section("clips.one", "First page", "Author").unwrap();
    let arena = Arena::default();
    let mut section = Section::open(&arena, source).unwrap();
    let creation = onestore::PageCreation::new(None, Some(""), "Author").unwrap();
    let create = Op::Section(onestore::op::SectionOp::Create(creation));
    let at = 133_000_000_000_000_000;
    section
        .apply(
            "Author",
            &Edit {
                at,
                ops: vec![create],
            },
        )
        .unwrap();
    let (space, ..) = section.pages().unwrap()[1].clone();
    let mut engine = TextEngine::default();
    let mut editor = CanvasEditor::from_page(section.page(space).unwrap(), &mut engine).unwrap();
    let title = editor
        .outlines()
        .iter()
        .find(|outline| outline.title)
        .unwrap()
        .id;
    editor.focus_outline(title).unwrap();
    let pieces = html_pieces("<b>Bold</b> <i>title</i>", 0x409, |_, _| None);
    editor.paste_pieces(&mut engine, pieces).unwrap();
    let ops = editor.take_ops().unwrap();
    let ops = ops.into_iter().map(|op| Op::Page { space, op }).collect();
    section
        .apply(
            "Author",
            &Edit {
                at: at + 10_000_000,
                ops,
            },
        )
        .unwrap();
    let page = section.page(space).unwrap();
    assert_eq!(page.title, "Bold title");
    let title = editor
        .outlines()
        .iter()
        .find(|outline| outline.title)
        .unwrap();
    assert_eq!(title.shown_text(), "Bold title");
}
