//! OneNote 2010's Enter, Backspace, Delete, Tab and paste replayed against the page XML it
//! reported after each keystroke: each outline is built from, and compared with, the
//! one-line-per-paragraph summaries in `corpus/structural-probe/summaries/`, whose `L` counts
//! paragraph nesting.

use super::format::{ListStyle, NoteTag, list_definition};
use super::*;
use onestore::document::Tag;
use onestore::page::ParagraphContent;
use onestore::page::text::new_id;

fn summary(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/../../corpus/structural-probe/summaries/{name}.txt",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
    .replace("\r\n", "\n")
}

fn calibri(size: f32, bold: bool) -> Format {
    Format {
        font: Some("Calibri".into()),
        font_size: Some(size),
        bold: Some(bold),
        ..Format::default()
    }
}

/// The quick styles a summary's `qs0` and `qs1` name.
const STYLES: [&str; 2] = ["p", "h1"];

/// An editor holding one outline shaped as a summary describes it.
fn open(engine: &mut TextEngine, summary: &str) -> CanvasEditor {
    let mut definitions = BTreeMap::new();
    let mut ids = BTreeMap::new();
    let mut define = |key: String, definition: Definition| {
        *ids.entry(key).or_insert_with(|| {
            let id = new_id().unwrap();
            definitions.insert(id, definition);
            id
        })
    };
    let styles = STYLES.map(|name| {
        define(
            name.into(),
            Definition {
                kind: Kind::Style {
                    name: Some(name.into()),
                    next: (name == "h1").then(|| "p".into()),
                },
                format: calibri(if name == "p" { 11.0 } else { 16.0 }, name != "p"),
            },
        )
    });
    let mut nodes: Vec<PageParagraph> = Vec::new();
    for line in summary.lines() {
        let line = line.trim_start().strip_prefix('L').unwrap();
        let (level, line) = line.split_once(" [").unwrap();
        let level = level.parse::<u32>().unwrap();
        let (tags, line) = line.split_once("] [").unwrap();
        let (list, line) = line.split_once("] ").unwrap();
        let (style, text) = line.split_once(' ').unwrap();
        let style = STYLES
            .iter()
            .position(|name| style == format!("qs{}", usize::from(*name == "h1")))
            .map(|index| styles[index]);
        let format = calibri(11.0, false);
        let mut node =
            crate::document::node(Paragraph::new(text.into(), format.clone()), format.clone())
                .unwrap();
        node.level = level;
        node.style = style;
        node.parent = nodes
            .iter()
            .rev()
            .find(|parent| parent.level + 1 == level)
            .map(|parent| parent.id);
        if !list.is_empty() {
            let definition = list_definition(
                if list.starts_with("num") {
                    ListStyle::NUMBER
                } else {
                    ListStyle::BULLET
                },
                &format,
            );
            node.lists = vec![define(format!("list {}", nodes.len()), definition)];
        }
        for tag in tags.split(',').filter(|tag| !tag.is_empty()) {
            let (label, completed) = match tag.strip_suffix("[x]") {
                Some(label) => (label, true),
                None => (tag, false),
            };
            let defaults = NoteTag::defaults();
            let place = defaults.iter().position(|tag| tag.label == label).unwrap();
            let definition = define(label.into(), defaults[place].definition(place as u16));
            node.text_mut().unwrap().tags.push(Tag {
                definition: Some(definition),
                action_type: None,
                shape: None,
                property_status: None,
                status: u16::from(completed),
                created: Some(1),
                completed: Some(u32::from(completed)),
                start: None,
                due: None,
                task_id: None,
                extra_set: 0,
            });
        }
        nodes.push(node);
    }
    let outline = Outline {
        id: new_id().unwrap(),
        title: false,
        min_width: None,
        layout: onestore::document::Layout {
            max_width: Some(400.0),
            ..Default::default()
        },
        indents: vec![18.0, 0.0, 27.0, 27.0],
        paragraphs: nodes,
        unsupported: Vec::new(),
    };
    CanvasEditor::from_outlines(engine, vec![outline], definitions).unwrap()
}

/// The outline in the summaries' form.
fn render(editor: &CanvasEditor) -> String {
    let outline = editor.active_outline();
    let mut depths = BTreeMap::new();
    let mut lines = Vec::new();
    for node in outline.document.nodes() {
        let depth = node.parent.map_or(1, |parent| depths[&parent] + 1);
        depths.insert(node.id, depth);
        let tags = node
            .tags
            .iter()
            .chain(&node.text().unwrap().tags)
            .map(|tag| {
                let kind = &editor.definitions[&tag.definition.unwrap()].kind;
                let Kind::TagDefinition { label, .. } = kind else {
                    panic!("{kind:?}")
                };
                let checked = if tag.status & 1 == 1 { "[x]" } else { "" };
                (
                    NoteTag::of(kind).and_then(|(tag, action_type)| {
                        NoteTag::defaults()
                            .iter()
                            .position(|known| *known == tag)
                            .filter(|place| *place == usize::from(action_type))
                    }),
                    format!("{}{checked}", label.as_deref().unwrap()),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let list = match node.lists.first().map(|id| &editor.definitions[id].kind) {
            None => String::new(),
            Some(Kind::List {
                bullet: Some(_), ..
            }) => "bullet".into(),
            Some(Kind::List {
                format: Some(format),
                ..
            }) => {
                let (_, rest) = format.split_once('\u{fffd}').unwrap();
                let mut rest = rest.chars();
                let number = outline
                    .shaped
                    .paragraphs
                    .iter()
                    .find(|paragraph| paragraph.id == node.id)
                    .and_then(|paragraph| paragraph.number.as_ref())
                    .unwrap()
                    .0
                    .number;
                let numeral = crate::outline::numeral(rest.next(), number).unwrap();
                format!("num:{numeral}{}", rest.as_str())
            }
            Some(kind) => panic!("{kind:?}"),
        };
        let style = node
            .style
            .map(|style| match &editor.definitions[&style].kind {
                Kind::Style { name, .. } => {
                    format!("qs{}", usize::from(name.as_deref() == Some("h1")))
                }
                kind => panic!("{kind:?}"),
            })
            .unwrap_or_default();
        lines.push(format!(
            "{}L{depth} [{}] [{list}] {style} {}",
            "  ".repeat(depth as usize),
            tags.into_values().collect::<Vec<_>>().join(","),
            node.text().unwrap().text.text()
        ));
    }
    lines.join("\n").trim_end().to_owned()
}

#[derive(Clone, Copy)]
enum Offset {
    Start,
    End,
    /// Characters before the end.
    Back(u32),
    /// Characters after the start.
    Forward(u32),
}

#[derive(Clone, Copy)]
enum Key {
    Enter,
    Backspace,
    Delete,
    Tab,
    ShiftTab,
    Type(&'static str),
    Paste(&'static str, u32),
    Bullets,
}

/// Puts the caret at `offset` in paragraph `paragraph`.
fn place(editor: &mut CanvasEditor, (paragraph, offset): (usize, Offset)) {
    let text = editor
        .active_outline()
        .document
        .paragraph(paragraph)
        .unwrap();
    let length = text.text().encode_utf16().count() as u32;
    let offset = match offset {
        Offset::Start => 0,
        Offset::End => length,
        Offset::Back(count) => length - count,
        Offset::Forward(count) => count,
    };
    editor
        .select([TextPosition { paragraph, offset }; 2].into())
        .unwrap();
}

fn press(editor: &mut CanvasEditor, engine: &mut TextEngine, key: Key) {
    match key {
        Key::Enter => editor.enter(engine, false).unwrap(),
        Key::Backspace => assert!(editor.delete(engine, true).unwrap()),
        Key::Delete => assert!(editor.delete(engine, false).unwrap()),
        Key::Tab => editor.tab(engine, false).unwrap(),
        Key::ShiftTab => editor.tab(engine, true).unwrap(),
        Key::Type(text) => editor.insert(engine, text).unwrap(),
        Key::Paste(text, language) => editor.paste(engine, text, language).unwrap(),
        Key::Bullets => editor.format(engine, Formatting::Bullets).unwrap(),
    }
}

/// Replays `steps` from `caret` in `before`, comparing the outline after each key with the
/// summary it names; returns the editor for further checks.
fn replay(
    engine: &mut TextEngine,
    before: &str,
    caret: (usize, Offset),
    steps: &[(Key, &str)],
) -> CanvasEditor {
    let steps = steps
        .iter()
        .map(|(key, name)| (*key, summary(name).trim_end().to_owned()))
        .collect::<Vec<_>>();
    expect(engine, before, caret, &steps)
}

/// [`replay`] against outlines given in the summaries' form.
fn expect(
    engine: &mut TextEngine,
    before: &str,
    caret: (usize, Offset),
    steps: &[(Key, String)],
) -> CanvasEditor {
    let mut editor = open(engine, before);
    place(&mut editor, caret);
    let original = editor.active_outline().document.clone();
    for (key, after) in steps {
        press(&mut editor, engine, *key);
        assert_eq!(&render(&editor), after);
        let outline = editor.active_outline();
        let fresh = OutlineLayout::flow(
            outline.document.nodes().iter(),
            &outline.indents,
            outline.wrap_width(),
            true,
            0,
            None,
            &mut |node, previous, width, indents| {
                ParagraphLayout::shape(engine, node, previous, width, indents, &editor.definitions)
            },
        )
        .unwrap();
        let numbers = |layout: &OutlineLayout| {
            layout
                .paragraphs
                .iter()
                .map(|paragraph| paragraph.number.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(numbers(&outline.shaped), numbers(&fresh), "{after}");
    }
    let edited = editor.active_outline().document.clone();
    while editor.undo(engine).unwrap() {}
    assert_eq!(editor.active_outline().document, original);
    while editor.redo(engine).unwrap() {}
    assert_eq!(editor.active_outline().document, edited);
    editor
}

/// The run that recorded Enter at the end of a paragraph of `kind`.
fn first_run(kind: &str) -> &'static str {
    match kind {
        "num" | "child" | "tagbullet" => "c1b",
        _ => "c1",
    }
}

/// The setup every single-paragraph case starts from: `Above`, then `Target text` of `kind`.
fn target(kind: &str) -> String {
    summary(&format!("{}-{kind}-0", first_run(kind)))
}

const KINDS: [&str; 9] = [
    "todo",
    "done",
    "imp",
    "q",
    "rem",
    "bullet",
    "num",
    "child",
    "tagbullet",
];

#[test]
fn enter_carries_level_lists_and_children_but_no_other_tags() {
    let mut engine = TextEngine::default();
    for kind in KINDS.into_iter().filter(|kind| !CHECKED.contains(kind)) {
        let prefix = first_run(kind);
        replay(
            &mut engine,
            &target(kind),
            (1, Offset::End),
            &[
                (Key::Enter, &format!("{prefix}-{kind}-1")),
                (Key::Type("New"), &format!("{prefix}-{kind}-2")),
            ],
        );
        replay(
            &mut engine,
            &target(kind),
            (1, Offset::Back(4)),
            &[
                (Key::Enter, &format!("c2-{kind}-1")),
                (Key::Type("X"), &format!("c2-{kind}-2")),
            ],
        );
        replay(
            &mut engine,
            &target(kind),
            (1, Offset::Start),
            &[
                (Key::Enter, &format!("c2s-{kind}-1")),
                (Key::Type("X"), &format!("c2s-{kind}-2")),
            ],
        );
        replay(
            &mut engine,
            &summary(&format!("c3-{kind}-0")),
            (2, Offset::Start),
            &[
                (Key::Enter, &format!("c3-{kind}-1")),
                (Key::Enter, &format!("c3-{kind}-2")),
                (Key::Type("New"), &format!("c3-{kind}-3")),
            ],
        );
    }
    replay(
        &mut engine,
        &summary("pbe-0"),
        (2, Offset::Start),
        &[(Key::Enter, "pbe-1")],
    );
    replay(
        &mut engine,
        &summary("c8-enter-empty-0"),
        (1, Offset::End),
        &[
            (Key::Enter, "c8-enter-empty-1"),
            (Key::Enter, "c8-enter-empty-2"),
        ],
    );
    replay(
        &mut engine,
        &summary("c8-split-0"),
        (2, Offset::Forward(1)),
        &[(Key::Enter, "c8-split-1")],
    );
    replay(
        &mut engine,
        &summary("c10-enter-end-0"),
        (1, Offset::End),
        &[(Key::Enter, "c10-enter-end-1")],
    );
}

/// The kinds with a To Do box, whose Enter departs from OneNote 2010's.
const CHECKED: [&str; 3] = ["todo", "done", "tagbullet"];

/// Enter continues a to-do list with unchecked items and an empty item ends it, as Enter does
/// a bulleted list; OneNote 2010 gives the new paragraph no tag and keeps an empty item's.
#[test]
fn enter_continues_a_to_do_list_unchecked_and_an_empty_item_ends_it() {
    let mut engine = TextEngine::default();
    for kind in CHECKED {
        let (level, tag, list) = match kind {
            "todo" => ("  L1", "To Do", ""),
            "done" => ("  L1", "To Do[x]", ""),
            _ => ("    L2", "To Do", "bullet"),
        };
        // Below `Above`, a paragraph per `(tag, list, text)`.
        let page = |lines: &[(&str, &str, &str)]| {
            std::iter::once("  L1 [] [] qs0 Above".to_owned())
                .chain(
                    lines
                        .iter()
                        .map(|(tag, list, text)| format!("{level} [{tag}] [{list}] qs0 {text}")),
                )
                .collect::<Vec<_>>()
                .join("\n")
                .trim_end()
                .to_owned()
        };
        let mut enter = |caret, [split, typed]: [&[(&str, &str, &str)]; 2], text| {
            expect(
                &mut engine,
                &target(kind),
                caret,
                &[(Key::Enter, page(split)), (Key::Type(text), page(typed))],
            );
        };
        enter(
            (1, Offset::End),
            [
                &[(tag, list, "Target text"), ("To Do", list, "")],
                &[(tag, list, "Target text"), ("To Do", list, "New")],
            ],
            "New",
        );
        enter(
            (1, Offset::Back(4)),
            [
                &[(tag, list, "Target "), ("To Do", list, "text")],
                &[(tag, list, "Target "), ("To Do", list, "Xtext")],
            ],
            "X",
        );
        enter(
            (1, Offset::Start),
            [
                &[("To Do", list, ""), (tag, list, "Target text")],
                &[("To Do", list, ""), (tag, list, "XTarget text")],
            ],
            "X",
        );
        let item = (tag, list, "Target text");
        expect(
            &mut engine,
            &summary(&format!("c3-{kind}-0")),
            (2, Offset::Start),
            &[
                (Key::Enter, page(&[item, ("", "", ""), ("", "", "")])),
                (
                    Key::Enter,
                    page(&[item, ("", "", ""), ("", "", ""), ("", "", "")]),
                ),
            ],
        );
    }
}

#[test]
fn backspace_removes_the_list_then_outdents_then_joins_and_the_upper_paragraph_wins() {
    let mut engine = TextEngine::default();
    for kind in KINDS {
        let steps = ["-1", "-2", "-3"].map(|step| format!("c4-{kind}{step}"));
        replay(
            &mut engine,
            &summary(&format!("c4-{kind}-0")),
            (2, Offset::Start),
            &steps.each_ref().map(|step| (Key::Backspace, step.as_str())),
        );
    }
    for pair in [
        "imp-todo",
        "plain-todo",
        "todo-plain",
        "todo-done",
        "done-todo",
        "q-rem",
    ] {
        replay(
            &mut engine,
            &summary(&format!("c4m-{pair}-0")),
            (1, Offset::Start),
            &[(Key::Backspace, &format!("c4m-{pair}-1"))],
        );
    }
    replay(
        &mut engine,
        &summary("c4m-emptybullet-0"),
        (2, Offset::Start),
        &[
            (Key::Backspace, "c4m-emptybullet-1"),
            (Key::Backspace, "c4m-emptybullet-2"),
            (Key::Backspace, "c4m-emptybullet-3"),
        ],
    );
    replay(
        &mut engine,
        &summary("c4m-emptytag-0"),
        (1, Offset::Start),
        &[
            (Key::Backspace, "c4m-emptytag-1"),
            (Key::Backspace, "c4m-emptytag-2"),
        ],
    );
    replay(
        &mut engine,
        &summary("c8-bs-0"),
        (2, Offset::Start),
        &[(Key::Backspace, "c8-bs-1"), (Key::Backspace, "c8-bs-2")],
    );
    replay(
        &mut engine,
        &summary("c10-bs-parent-0"),
        (1, Offset::Start),
        &[(Key::Backspace, "c10-bs-parent-1")],
    );
}

#[test]
fn delete_at_the_end_joins_the_next_paragraph_into_this_one() {
    let mut engine = TextEngine::default();
    for (pair, paragraph) in [
        ("imp-todo", 0),
        ("plain-todo", 0),
        ("todo-plain", 0),
        ("done-todo", 0),
        ("plain-bullet", 0),
        ("plain-num", 0),
        ("bullet-plain", 1),
        ("bullet-bullet", 1),
    ] {
        replay(
            &mut engine,
            &summary(&format!("c5b-{pair}-0")),
            (paragraph, Offset::End),
            &[(Key::Delete, &format!("c5b-{pair}-1"))],
        );
    }
    replay(
        &mut engine,
        &summary("c8-del-0"),
        (1, Offset::End),
        &[(Key::Delete, "c8-del-1")],
    );
    for (name, paragraph) in [("del-alpha", 0), ("del-parent", 1)] {
        replay(
            &mut engine,
            &summary(&format!("c10-{name}-0")),
            (paragraph, Offset::End),
            &[(Key::Delete, &format!("c10-{name}-1"))],
        );
    }
}

/// The list definition of the outline's paragraph `index`.
fn list(editor: &CanvasEditor, index: usize) -> &Kind<'static> {
    let node = &editor.active_outline().document.nodes()[index];
    &editor.definitions[&node.lists[0]].kind
}

#[test]
fn tab_nests_under_the_previous_sibling_and_shift_tab_adopts_the_siblings_after() {
    let mut engine = TextEngine::default();
    for kind in ["plain", "todo", "bullet", "num", "child", "tagbullet"] {
        let editor = replay(
            &mut engine,
            &summary(&format!("c6-{kind}-0")),
            (1, Offset::Start),
            &[(Key::Tab, &format!("c6-{kind}-tab"))],
        );
        // Without a previous sibling OneNote indents within the group (`<OEChildren indent="2">`
        // in `c6-{bullet,num,child,tagbullet}-tab.xml`), stepping bullets to ○ and numbers to a.
        let first_child = matches!(kind, "bullet" | "num" | "child" | "tagbullet");
        let node = &editor.active_outline().document.nodes()[1];
        assert_eq!(node.level, if first_child { 3 } else { 2 }, "{kind}");
        if matches!(kind, "bullet" | "tagbullet") {
            assert!(matches!(
                list(&editor, 1),
                Kind::List { format: Some(glyph), bullet: Some(4), .. } if glyph == "\u{25cb}"
            ));
        }
        replay(
            &mut engine,
            &summary(&format!("c6-{kind}-0")),
            (1, Offset::Start),
            &[
                (Key::Tab, &format!("c6-{kind}-tab")),
                (Key::ShiftTab, &format!("c6-{kind}-stab1")),
                (Key::ShiftTab, &format!("c6-{kind}-stab2")),
            ],
        );
    }
    let first = replay(
        &mut engine,
        "  L1 [] [] qs0 Target text",
        (0, Offset::Start),
        &[(Key::Tab, "c6-first-tab")],
    );
    assert_eq!(
        (
            first.active_outline().document.nodes()[0].level,
            first.active_outline().document.nodes()[0].parent
        ),
        (2, None)
    );
    replay(
        &mut engine,
        &summary("c8-stab-0"),
        (2, Offset::Start),
        &[(Key::ShiftTab, "c8-stab-1")],
    );
    replay(
        &mut engine,
        &summary("c8-tab-0"),
        (2, Offset::Start),
        &[(Key::Tab, "c8-tab-1"), (Key::ShiftTab, "c8-tab-2")],
    );
    replay(
        &mut engine,
        &summary("c10-stab-child-0"),
        (2, Offset::Start),
        &[(Key::ShiftTab, "c10-stab-child-1")],
    );
}

#[test]
fn pasted_lines_are_plain_paragraphs_between_the_halves() {
    let mut engine = TextEngine::default();
    for kind in ["todo", "done", "bullet", "num", "tagbullet", "child"] {
        // The native run's clipboard held a stray backquote for the numbered case.
        let text = if kind == "num" {
            "Line one\r\nLine two`"
        } else {
            "Line one\r\nLine two"
        };
        let editor = replay(
            &mut engine,
            &target(kind),
            (1, Offset::Back(4)),
            &[(Key::Paste(text, 1033), &format!("c7-{kind}-1"))],
        );
        let document = &editor.active_outline().document;
        let format = Format {
            bold: None,
            language: Some(1033),
            ..calibri(11.0, false)
        };
        assert_eq!(document.paragraph(2).unwrap().spans()[0].format, format);
        let last = document.paragraph(3).unwrap().text();
        assert_eq!(
            editor.selection().positions[1],
            TextPosition {
                paragraph: 3,
                offset: last.len() as u32
            }
        );
    }
    replay(
        &mut engine,
        "  L1 [] [] qs0 Above\n    L2 [] [num:1.] qs0 First\n    L2 [] [num:2.] qs0 Second",
        (1, Offset::Back(1)),
        &[(Key::Paste("Line one\r\nLine two", 1033), "c7-num2-1")],
    );
}

/// OneNote 2010 typing and pasting into a French run: typed text stays French, pasted text,
/// one line or several, takes the clipboard's language (en-US in the captures).
#[test]
fn pasted_text_takes_the_clipboard_language_and_typed_text_the_runs() {
    let mut engine = TextEngine::default();
    let (fr, en, de) = (Some(1036), Some(1033), Some(1031));
    let french = Format {
        language: fr,
        ..calibri(11.0, false)
    };
    for (key, languages) in [
        (Key::Type("Typed"), vec![fr]),
        (Key::Paste("Pasted", 1033), vec![fr, en, fr]),
        (
            Key::Paste("Line one\r\nLine two", 1033),
            vec![fr, en, en, fr],
        ),
        (Key::Paste("Pasted", 1031), vec![fr, de, fr]),
        (
            Key::Paste("Line one\r\nLine two", 1031),
            vec![fr, de, de, fr],
        ),
    ] {
        let mut editor = open(&mut engine, "  L1 [] [] qs0 Target text");
        editor
            .select(
                [0, 11]
                    .map(|offset| TextPosition {
                        paragraph: 0,
                        offset,
                    })
                    .into(),
            )
            .unwrap();
        editor
            .replace(
                &mut engine,
                vec![Paragraph::new("Target text".into(), french.clone())],
            )
            .unwrap();
        place(&mut editor, (0, Offset::Back(4)));
        press(&mut editor, &mut engine, key);
        let document = &editor.active_outline().document;
        let paragraphs = (0..).map_while(|paragraph| document.paragraph(paragraph));
        assert_eq!(
            paragraphs
                .flat_map(|paragraph| paragraph.spans())
                .map(|span| span.format.language)
                .collect::<Vec<_>>(),
            languages
        );
    }
}

#[test]
fn a_list_applied_after_a_plain_paragraph_nests_under_it_and_stays_nested() {
    let mut engine = TextEngine::default();
    replay(
        &mut engine,
        &summary("pb-0"),
        (1, Offset::Start),
        &[(Key::Bullets, "pb-1"), (Key::Bullets, "pb-2")],
    );
}

#[test]
fn a_heading_continues_as_body_text_unless_split() {
    let mut engine = TextEngine::default();
    // `c9-h1-1.xml` and `c9-h1-2.xml`; their summaries also carry the heading's inline runs.
    let mut editor = open(&mut engine, "  L1 [] [] qs1 Heading");
    let end = TextPosition {
        paragraph: 0,
        offset: 7,
    };
    editor.select([end; 2].into()).unwrap();
    editor.enter(&mut engine, false).unwrap();
    editor.insert(&mut engine, "body").unwrap();
    assert_eq!(
        render(&editor),
        "  L1 [] [] qs1 Heading\n  L1 [] [] qs0 body"
    );
    let body = editor.active_outline().document.paragraph(1).unwrap();
    assert_eq!(body.spans()[0].format, calibri(11.0, false));
    let middle = TextPosition {
        paragraph: 0,
        offset: 4,
    };
    editor.select([middle; 2].into()).unwrap();
    editor.enter(&mut engine, false).unwrap();
    assert_eq!(
        render(&editor),
        "  L1 [] [] qs1 Head\n  L1 [] [] qs1 ing\n  L1 [] [] qs0 body"
    );
}

#[test]
fn a_split_keeps_every_tag_above_and_a_join_takes_the_upper_tags() {
    let mut engine = TextEngine::default();
    // `c9-multitag-{1,2,3}.xml`.
    let mut editor = open(
        &mut engine,
        "  L1 [] [] qs0 Alpha\n  L1 [To Do,Important[x]] [] qs0 Two tags",
    );
    let at = |offset| TextPosition {
        paragraph: 1,
        offset,
    };
    editor.select([at(4); 2].into()).unwrap();
    editor.enter(&mut engine, false).unwrap();
    // OneNote 2010 leaves the lower half untagged (`c9-multitag-1`); its To Do continues here.
    assert_eq!(
        render(&editor),
        "  L1 [] [] qs0 Alpha\n  L1 [To Do,Important[x]] [] qs0 Two \n  L1 [To Do] [] qs0 tags"
    );
    editor.delete(&mut engine, true).unwrap();
    assert_eq!(render(&editor), summary("c9-multitag-2").trim_end());
    editor.select([at(0); 2].into()).unwrap();
    editor.delete(&mut engine, true).unwrap();
    assert_eq!(render(&editor), summary("c9-multitag-3").trim_end());
}

/// Nesting, adopted siblings, restyled and removed lists, an outdent, a tagged split and a join
/// that hands on children, on pages OneNote wrote, read back from the page writer unchanged.
#[test]
fn structural_edits_on_native_pages_survive_the_page_writer() {
    use onestore::{RevisionIndex, Store, document::Document};
    let section = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../corpus/structural-probe/probe.one"
    ))
    .unwrap();
    let page = |bytes: &[u8], title: &str| {
        let store = Store::parse(bytes).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let document = Document::parse(&index).unwrap();
        let (space, _) = document
            .pages()
            .unwrap()
            .into_iter()
            .find(|(space, _)| Page::from_space(&document, *space).unwrap().title == title)
            .unwrap();
        (space, Page::from_space(&document, space).unwrap())
    };
    // Tags keep their place in the element's property arena only once read.
    let settled = |mut page: Page| {
        for object in &mut page.objects {
            if let PageObject::Outline(outline) = object {
                for paragraph in &mut outline.paragraphs {
                    for tag in &mut paragraph.text_mut().unwrap().tags {
                        tag.extra_set = 0;
                    }
                }
            }
        }
        page
    };
    let mut engine = TextEngine::default();
    for (title, keys) in [
        (
            "c8-tab",
            &[
                ((2, Offset::Start), Key::ShiftTab),
                ((4, Offset::Start), Key::Tab),
                ((3, Offset::Start), Key::Backspace),
            ][..],
        ),
        (
            "c4-tagbullet",
            &[
                ((1, Offset::Start), Key::Backspace),
                ((1, Offset::Start), Key::Backspace),
            ],
        ),
        ("c2-todo", &[((1, Offset::Forward(3)), Key::Enter)]),
        (
            "c10-stab-child",
            &[
                ((2, Offset::Start), Key::Tab),
                ((3, Offset::Start), Key::Tab),
                ((1, Offset::End), Key::Delete),
            ],
        ),
    ] {
        let (space, source) = page(&section, title);
        let mut editor = CanvasEditor::from_page(source, &mut engine).unwrap();
        let body = editor
            .outlines()
            .iter()
            .find(|outline| !outline.title)
            .unwrap()
            .id;
        editor.focus_outline(body).unwrap();
        for (caret, key) in keys {
            place(&mut editor, *caret);
            press(&mut editor, &mut engine, *key);
        }
        let edited = settled(editor.page().unwrap());
        let written = super::ops::saved(&section, space, &mut editor);
        let (_, reread) = page(&written, title);
        assert_eq!(settled(reread).objects, edited.objects, "{title}");
    }
}

#[test]
fn tab_inside_text_makes_a_table_whose_paragraph_keeps_the_list_tags_and_children() {
    let mut engine = TextEngine::default();
    // `c6-tagbullet-midtab.xml`: the table's paragraph holds the tag and bullet, its cells none.
    let mut editor = open(&mut engine, &summary("c6-tagbullet-stab2"));
    place(&mut editor, (1, Offset::Back(4)));
    press(&mut editor, &mut engine, Key::Tab);
    let nodes = editor.active_outline().document.nodes();
    let ParagraphContent::Table(table) = &nodes[1].content else {
        panic!("{:?}", nodes[1].content)
    };
    assert_eq!(
        (nodes[1].level, nodes[1].lists.len(), nodes[1].tags.len()),
        (1, 1, 1)
    );
    for cell in table.rows.iter().flat_map(|row| &row.cells) {
        let text = cell.paragraphs[0].text().unwrap();
        assert!(cell.paragraphs[0].lists.is_empty() && text.tags.is_empty());
    }
    // `c10-tab-parent-1.xml`: the children stay under the table's paragraph.
    let mut editor = open(&mut engine, &summary("c10-tab-parent-0"));
    place(&mut editor, (1, Offset::End));
    press(&mut editor, &mut engine, Key::Tab);
    let nodes = editor.active_outline().document.nodes();
    assert!(matches!(nodes[1].content, ParagraphContent::Table(_)));
    assert_eq!((nodes[2].parent, nodes[2].level), (Some(nodes[1].id), 2));
}
