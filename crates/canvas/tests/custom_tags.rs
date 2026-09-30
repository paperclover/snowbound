//! Tags of a customized list, applied through the editor, store the definitions OneNote
//! 2010's Customize Tags stores (`corpus/custom-tags`).

use canvas::{
    document::TextPosition,
    editor::{CanvasEditor, Formatting, NoteTag},
    layout::TextEngine,
};
use onestore::{RevisionIndex, Store, document::Document, op, page::Page};

fn page(bytes: &[u8]) -> (onestore::ExGuid, Page) {
    let store = Store::parse(bytes).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let (space, _) = document.pages().unwrap()[0];
    (space, Page::from_space(&document, space).unwrap())
}

/// The list's tags: a symbol with both colours, a check box, a symbol alone and colours
/// alone, at the places the definitions store as action types.
fn list() -> Vec<NoteTag> {
    let tag = |label: &str, shape, color, highlight| NoteTag {
        label: label.into(),
        shape,
        color,
        highlight,
        art: None,
    };
    vec![
        tag("Snow check", 61, Some(0x0000_0080), Some(0x00ff_cc00)),
        tag("Snow task", 48, None, None),
        tag("Snow circle", 31, None, None),
        tag("Snow ink", 0, Some(0x00ff_0000), None),
    ]
}

/// Each paragraph takes the tags at these places in `list`; the third's box is checked.
const TAGGED: [(&str, &[u16]); 5] = [
    ("Custom symbol and colours", &[0]),
    ("Custom check box", &[1]),
    ("Custom check box, checked", &[1]),
    ("Two custom tags", &[2, 3]),
    // Typed before the new page's own "Untagged".
    ("", &[]),
];

/// `ONESTORE_CUSTOM_TAGS_EXPORT` names a new directory receiving the candidate for a cold
/// reopen.
#[test]
fn custom_tags_store_what_customize_tags_stores() {
    let source = onestore::create_section("custom-tags.one", "Untagged", "Author").unwrap();
    let (space, before) = page(&source);
    let mut engine = TextEngine::default();
    let mut editor = CanvasEditor::from_page(before, &mut engine).unwrap();
    let body = editor
        .outlines()
        .iter()
        .find(|outline| !outline.title)
        .unwrap()
        .id;
    editor.focus_outline(body).unwrap();
    let at = |paragraph| {
        [TextPosition {
            paragraph,
            offset: 0,
        }; 2]
            .into()
    };
    let list = list();
    for (paragraph, (text, places)) in TAGGED.iter().enumerate() {
        if paragraph > 0 {
            editor.enter(&mut engine, false).unwrap();
        }
        editor.insert(&mut engine, text).unwrap();
        editor.select(at(paragraph)).unwrap();
        for &place in *places {
            let tag = list[usize::from(place)].clone();
            editor
                .format(&mut engine, Formatting::Tag(tag, place))
                .unwrap();
        }
        if paragraph == 2 {
            editor.format(&mut engine, Formatting::Check).unwrap();
        }
        let offset = text.len() as u32;
        editor
            .select([TextPosition { paragraph, offset }; 2].into())
            .unwrap();
    }
    let arena = onestore::Arena::default();
    let mut section = onestore::Section::open(&arena, source.to_vec()).unwrap();
    let ops = editor
        .take_ops()
        .unwrap()
        .into_iter()
        .map(|op| op::Op::Page { space, op })
        .collect();
    section
        .apply(
            "Author",
            &op::Edit {
                at: 134_000_000_000_000_000,
                ops,
            },
        )
        .unwrap();
    section.seal().unwrap();
    let written = section.image();
    let (_, stored) = page(&written);
    let mut definitions: Vec<_> = stored
        .definitions
        .values()
        .filter_map(|definition| NoteTag::of(&definition.kind))
        .collect();
    definitions.sort_by_key(|(_, action_type)| *action_type);
    let expected: Vec<_> = list.into_iter().zip(0..).collect();
    assert_eq!(definitions, expected);
    let mut reread = CanvasEditor::from_page(stored, &mut engine).unwrap();
    reread.focus_outline(body).unwrap();
    for (paragraph, (_, places)) in TAGGED.iter().enumerate() {
        reread.select(at(paragraph)).unwrap();
        let tags: Vec<u16> = reread
            .format_state()
            .unwrap()
            .tags
            .into_iter()
            .map(|(_, action_type)| action_type)
            .collect();
        assert_eq!(tags, *places, "paragraph {paragraph}");
    }
    if let Some(directory) = std::env::var_os("ONESTORE_CUSTOM_TAGS_EXPORT") {
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(directory.join("custom-tags.one"), &written).unwrap();
        let file_id = Store::parse(&written).unwrap().header.file_id;
        std::fs::write(
            directory.join("Open Notebook.onetoc2"),
            onestore::create_table_of_contents(
                "Open Notebook.onetoc2",
                &[("custom-tags.one", file_id)],
            )
            .unwrap(),
        )
        .unwrap();
    }
}

/// A tag OneNote 2010's New Tag made, applied from two places in its list, reads back with
/// its name, symbol and colours (`corpus/custom-tags/native`).
#[test]
fn onenotes_custom_tags_read_back() {
    let bytes = include_bytes!("../../../corpus/custom-tags/native/shapes.one");
    let store = Store::parse(bytes).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let page = document
        .pages()
        .unwrap()
        .into_iter()
        .map(|(space, _)| Page::from_space(&document, space).unwrap())
        .find(|page| page.title.starts_with("Custom tags"))
        .unwrap();
    let mut engine = TextEngine::default();
    let mut editor = CanvasEditor::from_page(page, &mut engine).unwrap();
    let body = editor
        .outlines()
        .iter()
        .find(|outline| !outline.title)
        .unwrap()
        .id;
    editor.focus_outline(body).unwrap();
    let snow = NoteTag {
        label: "Snow check".into(),
        shape: 61,
        color: Some(0x0000_0080),
        highlight: Some(0x00ff_cc00),
        art: None,
    };
    let to_do = NoteTag::defaults()[0].clone();
    // Snow check went in at the top of the list, then moved below To Do.
    for (paragraph, expected) in [
        (0, (snow.clone(), 0)),
        (1, (to_do.clone(), 1)),
        (2, (snow, 1)),
        (3, (to_do, 0)),
    ] {
        editor
            .select(
                [TextPosition {
                    paragraph,
                    offset: 0,
                }; 2]
                    .into(),
            )
            .unwrap();
        assert_eq!(editor.format_state().unwrap().tags, [expected]);
    }
    let shaped = &editor.active_outline().shaped().paragraphs[0];
    assert_eq!(
        shaped.tags[0].icon,
        canvas::outline::TagIcon::of(61, false).unwrap()
    );
    assert!(
        shaped
            .text
            .backgrounds()
            .all(|(_, color)| color == 0x00ff_cc00)
    );
}
