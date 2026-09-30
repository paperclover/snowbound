//! Links and equations made as a user makes them, through the editor's typing, Link
//! dialog, Remove Link, Alt+=, Linear and Professional, save as the ops the editor recorded
//! and read back as the page it saved. `CANVAS_LINKS_EQUATIONS_EXPORT` names a new directory
//! receiving the section for a cold OneNote reopen, with the links and MathML its read must
//! show (`corpus/link-edit/editor`).

use canvas::{document::TextPosition, editor::CanvasEditor, layout::TextEngine};
use onestore::{
    Arena, ExGuid, RevisionIndex, Section, Store,
    document::Document,
    page::{
        Math, Page, PageObject,
        link::{LinkTarget, internal_link},
    },
};
use std::path::Path;

const BASE_PATH: &str = r"C:\one-tests\runs\capture\notebook\links.one";

fn pages(section: &[u8]) -> Vec<(ExGuid, Page)> {
    let store = Store::parse(section).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    document
        .pages()
        .unwrap()
        .into_iter()
        .map(|(space, _)| (space, Page::from_space(&document, space).unwrap()))
        .collect()
}

/// Types `text`, each space on its own as a key does.
fn typed(editor: &mut CanvasEditor, engine: &mut TextEngine, text: &str) {
    for (index, word) in text.split(' ').enumerate() {
        if index > 0 {
            editor.insert(engine, " ").unwrap();
        }
        if !word.is_empty() {
            editor.insert(engine, word).unwrap();
        }
    }
}

fn caret(editor: &CanvasEditor) -> TextPosition {
    editor.selection().positions[1]
}

/// End, then Enter, returning the new paragraph.
fn line(editor: &mut CanvasEditor, engine: &mut TextEngine) -> usize {
    let paragraph = caret(editor).paragraph;
    let text = editor
        .active_outline()
        .document()
        .paragraphs()
        .nth(paragraph)
        .unwrap();
    let offset = text.utf16_offset(text.text().len()).unwrap();
    editor
        .select([TextPosition { paragraph, offset }; 2].into())
        .unwrap();
    editor.enter(engine, false).unwrap();
    caret(editor).paragraph
}

fn select(editor: &mut CanvasEditor, paragraph: usize, range: std::ops::Range<u32>) {
    editor
        .select(
            [
                TextPosition {
                    paragraph,
                    offset: range.start,
                },
                TextPosition {
                    paragraph,
                    offset: range.end,
                },
            ]
            .into(),
        )
        .unwrap();
}

#[test]
fn links_and_equations_save_as_the_editor_shows_them() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let source = std::fs::read(root.join("corpus/link-edit/internal/candidate/links.one")).unwrap();
    let section_id = Store::parse(&source).unwrap().header.file_id;
    let all = pages(&source);
    let (space, page) = all
        .iter()
        .find(|(_, page)| page.title.starts_with("Linking page"))
        .cloned()
        .unwrap();
    let target = all
        .iter()
        .map(|(_, page)| page)
        .find(|page| page.title == "Link target")
        .unwrap();
    let paragraph_id = page
        .objects
        .iter()
        .find_map(|object| match object {
            PageObject::Outline(outline) if !outline.title => Some(outline.paragraphs[0].id),
            _ => None,
        })
        .unwrap();
    let mut engine = TextEngine::default();
    let engine = &mut engine;
    let mut editor = CanvasEditor::from_page(page.clone(), engine).unwrap();
    let body = editor
        .outlines()
        .iter()
        .find(|outline| !outline.title)
        .unwrap()
        .id;
    editor.focus_outline(body).unwrap();
    let end = editor
        .active_outline()
        .document()
        .paragraphs()
        .next()
        .unwrap();
    let end = end.utf16_offset(end.text().len()).unwrap();
    editor
        .select(
            [TextPosition {
                paragraph: 0,
                offset: end,
            }; 2]
                .into(),
        )
        .unwrap();
    let mut hrefs = vec![internal_link(
        section_id,
        BASE_PATH,
        LinkTarget::Page {
            identity: target.identity.unwrap(),
            title: "Link target",
        },
    )];
    // Typed URLs ended by a space and by Enter.
    line(&mut editor, engine);
    typed(
        &mut editor,
        engine,
        "Visit www.example.com or (https://example.org/a?b=1). ",
    );
    hrefs.extend([
        "http://www.example.com".into(),
        "https://example.org/a?b=1".into(),
    ]);
    line(&mut editor, engine);
    typed(&mut editor, engine, "Ends with http://example.net/end");
    hrefs.push("http://example.net/end".into());
    // The Link dialog on a selection, on the word at the caret and on nothing.
    let at = line(&mut editor, engine);
    typed(&mut editor, engine, "Read the Rust book today");
    select(&mut editor, at, 9..18);
    assert_eq!(
        editor.link_prefill(),
        Some(("Rust book".into(), String::new()))
    );
    editor
        .set_link(engine, "Rust book", "https://doc.rust-lang.org/book/")
        .unwrap();
    hrefs.push("https://doc.rust-lang.org/book/".into());
    let at = line(&mut editor, engine);
    typed(&mut editor, engine, "A word here");
    select(&mut editor, at, 3..3);
    editor.set_link(engine, "word", "example.net/x").unwrap();
    hrefs.push("http://example.net/x".into());
    line(&mut editor, engine);
    editor
        .set_link(engine, "", "https://example.com/only")
        .unwrap();
    typed(&mut editor, engine, " tail");
    hrefs.push("https://example.com/only".into());
    // A link to the other page and one to this page's first paragraph, as picked or pasted.
    line(&mut editor, engine);
    typed(&mut editor, engine, "See ");
    editor
        .set_link(engine, "the target page", &hrefs[0].clone())
        .unwrap();
    hrefs.push(hrefs[0].clone());
    line(&mut editor, engine);
    let paragraph_link = internal_link(
        section_id,
        BASE_PATH,
        LinkTarget::Object {
            identity: page.identity.unwrap(),
            title: "Linking page",
            object: paragraph_id,
        },
    );
    editor
        .set_link(engine, "Back to the top", &paragraph_link)
        .unwrap();
    hrefs.push(paragraph_link);
    // Remove Link.
    let at = line(&mut editor, engine);
    typed(&mut editor, engine, "Gone ftp://h.example/f z");
    select(&mut editor, at, 8..8);
    assert!(editor.remove_link(engine).unwrap());
    // Equations typed after Alt+=, one shown in its linear form, one after text.
    for linear in [
        "a_1+b_2",
        "\\int_0^1 x dx",
        "(a+b)/(c+d)",
        "\\sqrt x+1",
        "\\sum_(i=1)^n i",
        "\\matrix(1&2@3&4)",
        "e^(x+1)",
    ] {
        line(&mut editor, engine);
        editor.insert_equation(engine).unwrap();
        typed(&mut editor, engine, &format!("{linear} "));
    }
    assert!(editor.linear_equation(engine).unwrap());
    let at = line(&mut editor, engine);
    typed(&mut editor, engine, "Area ");
    editor.insert_equation(engine).unwrap();
    typed(&mut editor, engine, "\\pi r^2 ");
    // Linear, then Professional, rebuilds an equation as it was.
    let built = editor
        .active_outline()
        .document()
        .paragraphs()
        .nth(at)
        .unwrap()
        .clone();
    select(&mut editor, at, 7..7);
    assert!(editor.linear_equation(engine).unwrap());
    assert!(editor.build_equation(engine).unwrap());
    assert_eq!(
        editor
            .active_outline()
            .document()
            .paragraphs()
            .nth(at)
            .unwrap(),
        &built
    );
    // Enter at an equation's start, inside its row, inside a fraction's argument, and at a
    // link's start.
    let paragraph = |editor: &CanvasEditor, at: usize| {
        editor
            .active_outline()
            .document()
            .paragraphs()
            .nth(at)
            .unwrap()
            .clone()
    };
    let at = line(&mut editor, engine);
    typed(&mut editor, engine, "Split ");
    editor.insert_equation(engine).unwrap();
    typed(&mut editor, engine, "y^2+1 ");
    select(&mut editor, at, 6..6);
    editor.enter(engine, false).unwrap();
    let text = paragraph(&editor, at + 1);
    let one = text.utf16_offset(text.text().find('1').unwrap()).unwrap();
    select(&mut editor, at + 1, one..one);
    editor.enter(engine, false).unwrap();
    let at = line(&mut editor, engine);
    editor.insert_equation(engine).unwrap();
    typed(&mut editor, engine, "(a+b)/(c+d) ");
    let text = paragraph(&editor, at);
    let plus = text.utf16_offset(text.text().rfind('+').unwrap()).unwrap();
    select(&mut editor, at, plus..plus);
    editor.enter(engine, false).unwrap();
    let at = line(&mut editor, engine);
    typed(&mut editor, engine, "Moved ");
    editor
        .set_link(engine, "down", "https://example.com/moved")
        .unwrap();
    hrefs.push("https://example.com/moved".into());
    let text = paragraph(&editor, at);
    let label = text
        .utf16_offset(text.text().find("down").unwrap())
        .unwrap();
    select(&mut editor, at, label..label);
    editor.enter(engine, false).unwrap();

    let after = editor.page().unwrap();
    let arena = Arena::default();
    let mut stored = Section::open(&arena, source.clone()).unwrap();
    let edit = onestore::op::Edit {
        at: 133_000_000_000_000_000,
        ops: editor
            .take_ops()
            .unwrap()
            .into_iter()
            .map(|op| onestore::op::Op::Page { space, op })
            .collect(),
    };
    stored.apply("Snowbound", &edit).unwrap();
    stored.seal().unwrap();
    let saved = stored.image();
    let (_, reread) = pages(&saved)
        .into_iter()
        .find(|(id, _)| *id == space)
        .unwrap();
    let body = |page: &Page| -> Vec<onestore::page::PageParagraph> {
        page.objects
            .iter()
            .filter_map(|object| match object {
                PageObject::Outline(outline) if !outline.title => Some(outline),
                _ => None,
            })
            .flat_map(|outline| outline.paragraphs.clone())
            .collect()
    };
    for (stored, shown) in body(&reread).iter().zip(body(&after)) {
        assert_eq!(stored, &shown);
    }
    assert_eq!(
        Page {
            title: after.title.clone(),
            ..reread.clone()
        },
        after
    );

    let paragraphs: Vec<_> = reread
        .objects
        .iter()
        .filter_map(|object| match object {
            PageObject::Outline(outline) if !outline.title => Some(outline),
            _ => None,
        })
        .flat_map(|outline| &outline.paragraphs)
        .filter_map(|paragraph| paragraph.text())
        .collect();
    let stored_hrefs: Vec<String> = paragraphs
        .iter()
        .flat_map(|text| {
            text.text
                .spans()
                .iter()
                .scan(0, |start, span| {
                    let run = &text.text.text()[*start..span.end];
                    *start = span.end;
                    Some((run, span.format.clone()))
                })
                .filter(|(_, format)| format.hyperlink == Some(true) && format.hidden != Some(true))
                .map(|(run, _)| run.to_owned())
                .collect::<Vec<_>>()
        })
        .collect();
    assert_eq!(stored_hrefs.len(), hrefs.len(), "{stored_hrefs:?}");
    // OneNote links URL text again when it opens a page, after its own Remove Link too
    // (`corpus/link-edit/native-typed`, cold read).
    let moved = hrefs.len() - 1;
    hrefs.insert(moved, "ftp://h.example/f".into());
    // The MathML OneNote exports for each run of math, an equation of its own.
    let mut mathml = Vec::new();
    for text in &paragraphs {
        let mut start = 0;
        let mut zone = Vec::new();
        for span in text.text.spans() {
            let run = text.text.text()[start..span.end].to_owned();
            start = span.end;
            if span.format.math == Some(true) {
                zone.push((run, span.format.clone()));
            }
            let ends = span.format.math != Some(true) || start == text.text.text().len();
            if ends && !zone.is_empty() {
                let math = onestore::page::Paragraph::from_runs(std::mem::take(&mut zone));
                mathml.push(Math::mathml(&Math::parse(&math).unwrap()));
            }
        }
    }
    assert_eq!(mathml.len(), 11);
    if let Some(directory) = std::env::var_os("CANVAS_LINKS_EQUATIONS_EXPORT") {
        let directory = Path::new(&directory);
        std::fs::create_dir(directory).unwrap();
        std::fs::write(directory.join("links.one"), &saved).unwrap();
        std::fs::copy(
            root.join("corpus/link-edit/internal/candidate/Open Notebook.onetoc2"),
            directory.join("Open Notebook.onetoc2"),
        )
        .unwrap();
        std::fs::write(
            directory.join("expected.json"),
            serde_json::to_string_pretty(&serde_json::json!({
                "hrefs": hrefs,
                "mathml": mathml,
            }))
            .unwrap(),
        )
        .unwrap();
    }
}

/// A paragraph's runs as (text, hidden, hyperlink, math, italic, run data), with a run
/// between objects stored either with or without run data.
type Runs = Vec<(String, bool, bool, bool, bool, Option<(u32, Option<u32>)>)>;

fn runs(text: &onestore::page::Paragraph) -> Runs {
    let mut start = 0;
    text.spans()
        .iter()
        .map(|span| {
            let run = text.text()[start..span.end].to_owned();
            start = span.end;
            let format = &span.format;
            let object = format
                .math_object
                .as_ref()
                .map(|object| (object.kind, object.arguments))
                .filter(|(kind, _)| *kind != 0x9000_0000);
            (
                run,
                format.hidden == Some(true),
                format.hyperlink == Some(true),
                format.math == Some(true),
                format.italic == Some(true),
                object,
            )
        })
        .collect()
}

/// Enter at an equation's start, inside its row and inside a fraction's argument, and at a
/// link's start, stores what OneNote 2010 stored for the same keys (`tools/native_enter.py`,
/// `corpus/math-edit/native-enter`).
#[test]
fn enter_at_and_inside_equations_and_links_stores_what_onenote_stored() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let native =
        std::fs::read(root.join("corpus/math-edit/native-enter/notebook/links.one")).unwrap();
    let (_, page) = pages(&native).remove(0);
    let native: Vec<Runs> = page
        .objects
        .iter()
        .filter_map(|object| match object {
            PageObject::Outline(outline) if !outline.title => Some(outline),
            _ => None,
        })
        .flat_map(|outline| &outline.paragraphs)
        .filter_map(|paragraph| paragraph.text())
        .skip(1)
        .map(|text| runs(&text.text))
        .collect();
    let mut engine = TextEngine::default();
    let engine = &mut engine;
    let mut editor = CanvasEditor::new(
        engine,
        canvas::document::TextDocument::new(vec![onestore::page::Paragraph::new(
            String::new(),
            onestore::document::Format {
                font: Some("Calibri".into()),
                ..Default::default()
            },
        )])
        .unwrap(),
        400.0,
    )
    .unwrap();
    let text = |editor: &CanvasEditor, paragraph: usize| {
        editor
            .active_outline()
            .document()
            .paragraphs()
            .nth(paragraph)
            .unwrap()
            .clone()
    };
    let before = |editor: &CanvasEditor, paragraph: usize, needle: &str| {
        let text = text(editor, paragraph);
        let at = text.text().find(needle).unwrap();
        text.utf16_offset(at).unwrap()
    };
    typed(&mut editor, engine, "Before ");
    editor.insert_equation(engine).unwrap();
    typed(&mut editor, engine, "y^2+1 ");
    select(&mut editor, 0, 7..7);
    editor.enter(engine, false).unwrap();
    let at = before(&editor, 1, "1");
    select(&mut editor, 1, at..at);
    editor.enter(engine, false).unwrap();
    line(&mut editor, engine);
    editor.insert_equation(engine).unwrap();
    typed(&mut editor, engine, "(a+b)/(c+d) ");
    let at = before(&editor, 2, "+𝑑");
    select(&mut editor, 2, at..at);
    editor.enter(engine, false).unwrap();
    let paragraph = line(&mut editor, engine);
    typed(&mut editor, engine, "Select me please");
    select(&mut editor, paragraph, 7..16);
    editor
        .set_link(engine, "me please", "https://example.com/selected")
        .unwrap();
    let start = before(&editor, paragraph, "me please");
    select(&mut editor, paragraph, start..start);
    editor.enter(engine, false).unwrap();
    let written: Vec<Runs> = editor
        .active_outline()
        .document()
        .paragraphs()
        .map(runs)
        .collect();
    assert_eq!(written, native);
}

/// Enter and Shift+Enter inside a link's label leave it whole, as in OneNote 2010, where
/// Shift+Enter does nothing and Enter follows the link; at its end Enter splits as anywhere
/// else.
#[test]
fn enter_inside_a_link_is_ignored() {
    let mut engine = TextEngine::default();
    let engine = &mut engine;
    let mut editor = CanvasEditor::new(
        engine,
        canvas::document::TextDocument::new(vec![onestore::page::Paragraph::new(
            String::new(),
            Default::default(),
        )])
        .unwrap(),
        400.0,
    )
    .unwrap();
    typed(&mut editor, engine, "Visit the example site today");
    select(&mut editor, 0, 10..22);
    editor
        .set_link(engine, "example site", "https://example.com/")
        .unwrap();
    editor.take_ops().unwrap();
    let document = editor.active_outline().document().clone();
    let text = document.paragraphs().next().unwrap();
    let end = text
        .utf16_offset(text.text().find(" today").unwrap())
        .unwrap();
    let label = editor
        .link_at(TextPosition {
            paragraph: 0,
            offset: end,
        })
        .unwrap()
        .label;
    let inside = label.start + 7;
    for soft in [false, true] {
        select(&mut editor, 0, inside..inside);
        editor.enter(engine, soft).unwrap();
        assert_eq!(*editor.active_outline().document(), document);
        assert!(editor.take_ops().unwrap().is_empty());
    }
    select(&mut editor, 0, label.end..label.end);
    editor.enter(engine, false).unwrap();
    assert_eq!(editor.active_outline().document().paragraphs().count(), 2);
}

/// Link over a selection across paragraphs does nothing, as OneNote 2010's does.
#[test]
fn link_over_paragraphs_does_nothing() {
    let mut engine = TextEngine::default();
    let engine = &mut engine;
    let mut editor = CanvasEditor::new(
        engine,
        canvas::document::TextDocument::new(vec![
            onestore::page::Paragraph::new("One two".into(), Default::default()),
            onestore::page::Paragraph::new("Three four".into(), Default::default()),
        ])
        .unwrap(),
        400.0,
    )
    .unwrap();
    let document = editor.active_outline().document().clone();
    editor
        .select(
            [
                TextPosition {
                    paragraph: 0,
                    offset: 4,
                },
                TextPosition {
                    paragraph: 1,
                    offset: 5,
                },
            ]
            .into(),
        )
        .unwrap();
    assert_eq!(editor.link_prefill(), None);
    editor.set_link(engine, "", "https://example.com/").unwrap();
    assert_eq!(*editor.active_outline().document(), document);
    assert!(editor.take_ops().unwrap().is_empty());
}

/// Deleting from inside one equation into the next joins them into one, as OneNote 2010
/// joined "aa+bb" and "cc+dd" to "aa++dd" in the lab; the section it stores reopens in OneNote
/// (`corpus/equation-join`, exported to `CANVAS_EQUATION_JOIN_EXPORT`).
#[test]
fn deleting_between_equations_joins_them() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let source = std::fs::read(root.join("corpus/link-edit/candidate/links.one")).unwrap();
    let (space, page) = pages(&source).remove(0);
    let mut engine = TextEngine::default();
    let engine = &mut engine;
    let mut editor = CanvasEditor::from_page(page, engine).unwrap();
    let body = editor
        .outlines()
        .iter()
        .find(|outline| !outline.title)
        .unwrap()
        .id;
    editor.focus_outline(body).unwrap();
    let first = line(&mut editor, engine);
    editor.insert_equation(engine).unwrap();
    typed(&mut editor, engine, "aa+bb ");
    let second = line(&mut editor, engine);
    editor.insert_equation(engine).unwrap();
    typed(&mut editor, engine, "cc+dd ");
    let text = |editor: &CanvasEditor, paragraph: usize| {
        editor
            .active_outline()
            .document()
            .paragraphs()
            .nth(paragraph)
            .unwrap()
            .clone()
    };
    let before = |editor: &CanvasEditor, paragraph: usize, needle: &str| {
        let text = text(editor, paragraph);
        text.utf16_offset(text.text().find(needle).unwrap())
            .unwrap()
    };
    let (from, to) = (before(&editor, first, "𝑏𝑏"), before(&editor, second, "+𝑑"));
    editor
        .select(
            [
                TextPosition {
                    paragraph: first,
                    offset: from,
                },
                TextPosition {
                    paragraph: second,
                    offset: to,
                },
            ]
            .into(),
        )
        .unwrap();
    editor.delete(engine, false).unwrap();
    let joined = text(&editor, first);
    assert_eq!(joined.text().trim_end(), "𝑎𝑎++𝑑𝑑");
    assert!(
        joined
            .spans()
            .iter()
            .all(|span| span.format.math == Some(true))
    );
    let arena = Arena::default();
    let mut section = Section::open(&arena, source).unwrap();
    let ops = editor
        .take_ops()
        .unwrap()
        .into_iter()
        .map(|op| onestore::op::Op::Page { space, op })
        .collect();
    section
        .apply(
            "Author",
            &onestore::op::Edit {
                at: 134_000_000_000_000_000,
                ops,
            },
        )
        .unwrap();
    section.seal().unwrap();
    let written = section.image();
    let (_, stored) = pages(&written).remove(0);
    let equations: Vec<String> = stored
        .objects
        .iter()
        .filter_map(|object| match object {
            PageObject::Outline(outline) if !outline.title => Some(outline),
            _ => None,
        })
        .flat_map(|outline| &outline.paragraphs)
        .filter_map(|paragraph| paragraph.text())
        .filter(|text| {
            text.text
                .spans()
                .iter()
                .any(|span| span.format.math == Some(true))
        })
        .map(|text| text.text.text().trim_end().to_owned())
        .collect();
    assert_eq!(equations, ["𝑎𝑎++𝑑𝑑"]);
    if let Some(directory) = std::env::var_os("CANVAS_EQUATION_JOIN_EXPORT") {
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(directory.join("links.one"), &written).unwrap();
        let file_id = Store::parse(&written).unwrap().header.file_id;
        std::fs::write(
            directory.join("Open Notebook.onetoc2"),
            onestore::create_table_of_contents("Open Notebook.onetoc2", &[("links.one", file_id)])
                .unwrap(),
        )
        .unwrap();
    }
}

/// Alt+= over a selection across paragraphs makes each paragraph's part an equation of its
/// own, as OneNote 2010 does, and one undo takes them back.
#[test]
fn equation_over_paragraphs_makes_one_each() {
    let mut engine = TextEngine::default();
    let engine = &mut engine;
    let mut editor = CanvasEditor::new(
        engine,
        canvas::document::TextDocument::new(vec![
            onestore::page::Paragraph::new("One two".into(), Default::default()),
            onestore::page::Paragraph::new("Three four".into(), Default::default()),
        ])
        .unwrap(),
        400.0,
    )
    .unwrap();
    let document = editor.active_outline().document().clone();
    editor
        .select(
            [
                TextPosition {
                    paragraph: 0,
                    offset: 4,
                },
                TextPosition {
                    paragraph: 1,
                    offset: 5,
                },
            ]
            .into(),
        )
        .unwrap();
    editor.insert_equation(engine).unwrap();
    let math: Vec<Vec<(String, bool)>> = editor
        .active_outline()
        .document()
        .paragraphs()
        .map(|text| {
            let mut start = 0;
            text.spans()
                .iter()
                .map(|span| {
                    let part = text.text()[start..span.end].to_owned();
                    start = span.end;
                    (part, span.format.math == Some(true))
                })
                .collect()
        })
        .collect();
    assert_eq!(
        math,
        [
            vec![("One ".to_owned(), false), ("𝑡𝑤𝑜".to_owned(), true)],
            vec![("𝑇ℎ𝑟𝑒𝑒".to_owned(), true), (" four".to_owned(), false)],
        ]
    );
    assert!(editor.undo(engine).unwrap());
    assert_eq!(*editor.active_outline().document(), document);
}

/// An equation among text draws in its line, which grows to hold it, and URL text shows as a
/// link without being stored as one.
#[test]
fn equations_draw_inline_and_url_text_shows_as_a_link() {
    let mut engine = TextEngine::default();
    let engine = &mut engine;
    let mut editor = CanvasEditor::new(
        engine,
        canvas::document::TextDocument::new(vec![onestore::page::Paragraph::new(
            String::new(),
            Default::default(),
        )])
        .unwrap(),
        400.0,
    )
    .unwrap();
    typed(&mut editor, engine, "Area ");
    editor.insert_equation(engine).unwrap();
    typed(&mut editor, engine, "(a+b)/(c+d) ");
    let outline = editor.active_outline();
    let layout = outline.paragraph_layout(0).unwrap();
    assert_eq!(layout.math.len(), 1);
    let spaces: Vec<_> = layout.text.spaces().collect();
    let [(0, [x, baseline])] = spaces[..] else {
        panic!("{spaces:?}")
    };
    let (_, line) = layout.text.lines().next().unwrap();
    assert!(x > 20.0, "the equation follows the text");
    assert!(line.height >= layout.math[0].size[1]);
    assert!(baseline - line.top >= layout.math[0].baseline);

    let url = onestore::page::Paragraph::new("Gone ftp://h.example/f z".into(), Default::default());
    assert_eq!(canvas::editor::shown_urls(&url), vec![5..22]);
    assert!(url.spans()[0].format.hyperlink.is_none());
}
