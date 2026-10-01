#![no_main]
use canvas::{date::PageDate, document::{TextDocument, TextPosition}, editor::{CanvasEditor, TextOutline}, layout::TextEngine};
use draw::edit::{Movement, SelectionUnit};
use onestore::page::text::{Paragraph};
use libfuzzer_sys::fuzz_target;
use onestore::document::Format;
use std::{cell::RefCell, collections::BTreeMap};

thread_local! {
    static ENGINE: RefCell<TextEngine> = RefCell::new(TextEngine::default());
}

fuzz_target!(|input: &[u8]| {
    ENGINE.with_borrow_mut(|engine| {
        if input.first().is_some_and(|byte| byte & 32 != 0) {
            let document = TextDocument::new(vec![Paragraph::new(
                "ALPHA".into(),
                Format {
                    font_size: (input[0] & 64 != 0).then_some(130.0),
                    ..Default::default()
                },
            )])
            .unwrap();
            let mut editor = CanvasEditor::new(engine, document, 180.0).unwrap();
            for step in input.chunks_exact(4).take(24) {
                let positions = editor
                    .active_outline()
                    .document()
                    .paragraphs()
                    .enumerate()
                    .flat_map(|(paragraph, text)| {
                        text.text()
                            .char_indices()
                            .map(move |(byte, _)| TextPosition {
                                paragraph,
                                offset: text.utf16_offset(byte).unwrap(),
                            })
                            .chain(std::iter::once(TextPosition {
                                paragraph,
                                offset: text.utf16_offset(text.text().len()).unwrap(),
                            }))
                    })
                    .collect::<Vec<_>>();
                let focus = positions[usize::from(step[0]) % positions.len()];
                let anchor = if step[3] & 16 == 0 {
                    focus
                } else {
                    positions[usize::from(step[3]) % positions.len()]
                };
                editor.select([anchor, focus].into()).unwrap();
                let before = editor.active_outline().document().clone();
                let stored = editor
                    .outlines()
                    .iter()
                    .map(|outline| (outline.id, outline.document().clone()))
                    .collect::<Vec<_>>();
                let selection = editor.selection();
                let result = match step[1] % 10 {
                    0 => editor.tab(engine, false),
                    1 => editor.tab(engine, true),
                    2 => editor.enter(engine, false),
                    3 => editor.enter(engine, true),
                    4 => editor.insert(
                        engine,
                        ["x", "🧊", "left\nright", "العربية"][usize::from(step[2]) % 4],
                    ),
                    5 => editor.delete(engine, step[2] & 1 == 0).map(|_| ()),
                    6 => editor.indent(engine, step[2] & 1 == 0).map(|_| ()),
                    7 => editor
                        .move_selection(
                            engine,
                            [
                                Movement::Left,
                                Movement::Right,
                                Movement::Up,
                                Movement::Down,
                            ][usize::from(step[2]) % 4],
                            step[3] & 32 != 0,
                        )
                        .inspect_err(|error| {
                            assert!(!matches!(
                                error,
                                canvas::editor::EditorError::Edit(
                                    onestore::page::text::EditError::InvalidStructure
                                )
                            ));
                        }),
                    8 => {
                        let unit = [
                            SelectionUnit::Grapheme,
                            SelectionUnit::Word,
                            SelectionUnit::Paragraph,
                        ][usize::from(step[2]) % 3];
                        editor
                            .selection_at(
                                f32::from(step[2]) - 32.0,
                                f32::from(step[3]) - 32.0,
                                unit,
                            )
                            .and_then(|selection| editor.select(selection))
                            .map_err(Into::into)
                    }
                    _ => editor.compose(engine, "🧊\n中".into(), 4..4).and_then(|_| {
                        if step[3] & 64 != 0 {
                            editor.cancel_composition(engine).map(|_| ())
                        } else {
                            editor.commit_text(engine, "done".into())
                        }
                    }),
                };
                if result.is_err() {
                    assert_eq!(editor.active_outline().document(), &before);
                    assert_eq!(editor.selection(), selection);
                    assert_eq!(
                        editor
                            .outlines()
                            .iter()
                            .map(|outline| (outline.id, outline.document().clone()))
                            .collect::<Vec<_>>(),
                        stored
                    );
                    continue;
                }
                let after = editor.active_outline().document().clone();
                let after_selection = editor.selection();
                let committed = editor
                    .outlines()
                    .iter()
                    .map(|outline| (outline.id, outline.document().clone()))
                    .collect::<Vec<_>>();
                if committed != stored {
                    assert!(editor.undo(engine).unwrap());
                    assert_eq!(
                        editor
                            .outlines()
                            .iter()
                            .map(|outline| (outline.id, outline.document().clone()))
                            .collect::<Vec<_>>(),
                        stored
                    );
                    assert_eq!(editor.active_outline().document(), &before);
                    assert_eq!(editor.selection(), selection);
                    assert!(editor.redo(engine).unwrap());
                    assert_eq!(
                        editor
                            .outlines()
                            .iter()
                            .map(|outline| (outline.id, outline.document().clone()))
                            .collect::<Vec<_>>(),
                        committed
                    );
                    assert_eq!(editor.active_outline().document(), &after);
                    assert_eq!(editor.selection(), after_selection);
                }
            }
            return;
        }
        let width = [1.0, 10.0, 72.0, 240.0][usize::from(input.first().copied().unwrap_or(0)) % 4];
        let document = TextDocument::new(
            ["office e\u{301} 👩🏽‍💻", "العربية 日本"]
                .map(|text| Paragraph::new(text.into(), Format::default()))
                .into(),
        )
        .unwrap();
        let position = [
            [0.0, 179.9, 180.0, 270.0][usize::from(input.first().copied().unwrap_or(0) >> 2) % 4],
            158.4,
        ];
        let mut outline = TextOutline::new(engine, document, width, position).unwrap();
        if input.first().is_some_and(|byte| byte & 128 != 0) {
            let mut source = outline.snapshot();
            source.title = true;
            source.min_width = Some(162.0);
            source.layout.max_width = None;
            source.layout.width_set_by_user = None;
            outline = TextOutline::from_outline(engine, &source, &BTreeMap::new()).unwrap();
        }
        let date = if input.first().is_some_and(|byte| byte & 64 != 0) {
            let document = TextDocument::new(
                ["Monday", "6:14 AM"]
                    .map(|text| Paragraph::new(text.into(), Format::default()))
                    .into(),
            )
            .unwrap();
            let source = TextOutline::new(engine, document, 468.0, [0.0; 2])
                .unwrap()
                .snapshot();
            Some(PageDate::new(100, source, engine, &BTreeMap::new()).unwrap())
        } else {
            None
        };
        let mut editor = if outline.title {
            let mut fields = vec![outline.snapshot()];
            let date_id = date.as_ref().map(|date| date.source().id);
            if let Some(date) = &date {
                fields.push(date.source().clone());
            }
            let mut objects = vec![onestore::page::PageObject::Title(onestore::page::Title {
                id: onestore::ExGuid::default(),
                layout: Default::default(),
                date: date_id,
                outlines: fields,
            })];
            for (index, (x, y, background)) in [
                (120.0, 70.0, false),
                (600.0, 180.0, false),
                (120.0, 70.0, true),
            ]
            .into_iter()
            .enumerate()
            {
                let mut image =
                    canvas::editor::picture(b"deferred decoding".to_vec(), [72.0, 40.0]).unwrap();
                image.id = onestore::ExGuid {
                    n: index.try_into().unwrap(),
                    ..Default::default()
                };
                [image.layout.x, image.layout.y] = [Some(x), Some(position[1] + y)];
                image.background = background;
                objects.push(onestore::page::PageObject::Image(image));
            }
            // An empty editor's page is a page with every property at its default.
            let blank = TextDocument::new(vec![Paragraph::new(String::new(), Format::default())]);
            let mut page = CanvasEditor::new(engine, blank.unwrap(), 468.0)
                .unwrap()
                .page()
                .unwrap();
            page.created = date.map(|date| date.timestamp());
            page.objects = objects;
            CanvasEditor::from_page(page, engine).unwrap()
        } else {
            CanvasEditor::from_text_outlines(vec![outline], BTreeMap::new(), date).unwrap()
        };
        let layouts = |editor: &CanvasEditor| {
            editor
                .object_layouts()
                .map(|(id, layout)| (id, layout.clone()))
                .collect::<Vec<_>>()
        };
        for step in input.chunks_exact(8).take(32) {
            let width = editor.active_outline().wrap_width();
            if step[6] & 128 != 0 {
                editor.place_caret(engine, position, width).unwrap();
            }
            let mut positions = Vec::new();
            let mut source = String::new();
            for (paragraph, text) in editor.active_outline().document().paragraphs().enumerate() {
                if paragraph != 0 {
                    source.push('\n');
                }
                let base = source.len();
                let mut offset = 0;
                for (byte, character) in text.text().char_indices() {
                    positions.push((TextPosition { paragraph, offset }, base + byte));
                    offset += character.len_utf16() as u32;
                }
                positions.push((TextPosition { paragraph, offset }, base + text.text().len()));
                source.push_str(text.text());
            }
            let (anchor, a) = positions[usize::from(step[0]) % positions.len()];
            let (focus, b) = positions[usize::from(step[1]) % positions.len()];
            editor.select([anchor, focus].into()).unwrap();
            let before = editor.active_outline().document().clone();
            let layouts_before = layouts(&editor);
            let selection = editor.selection();
            let text = [
                "",
                "a",
                "e\u{301}",
                "👩🏽‍💻",
                "\n",
                "left\nright\u{000b}soft",
                "العربية",
                "日本",
            ][usize::from(step[2]) % 8];
            let text = if text.is_empty() && editor.caret_outline().is_some() {
                "x"
            } else {
                text
            };
            if step[3] & 1 != 0 {
                let end = text.encode_utf16().count() as u32;
                editor.compose(engine, text.into(), end..end).unwrap();
                assert!(editor.cancel_composition(engine).unwrap());
                assert_eq!(editor.active_outline().document(), &before);
                assert_eq!(editor.selection(), selection);
                assert_eq!(layouts(&editor), layouts_before);
            }
            let format = Format {
                font_size: Some(10.0 + f32::from(step[4] % 8)),
                bold: Some(step[5] & 1 != 0),
                italic: Some(step[5] & 2 != 0),
                ..Format::default()
            };
            editor
                .replace(
                    engine,
                    text.split('\n')
                        .map(|part| Paragraph::new(part.into(), format.clone()))
                        .collect(),
                )
                .unwrap();
            source.replace_range(a.min(b)..a.max(b), text);
            assert_eq!(
                editor
                    .active_outline()
                    .document()
                    .paragraphs()
                    .map(|p| p.text())
                    .collect::<Vec<_>>()
                    .join("\n"),
                source
            );
            let after = editor.active_outline().document().clone();
            let layouts_after = layouts(&editor);
            let after_selection = editor.selection();
            assert!(editor.undo(engine).unwrap());
            assert_eq!(editor.active_outline().document(), &before);
            assert_eq!(editor.selection(), selection);
            assert_eq!(layouts(&editor), layouts_before);
            assert!(editor.redo(engine).unwrap());
            assert_eq!(editor.active_outline().document(), &after);
            assert_eq!(editor.selection(), after_selection);
            assert_eq!(layouts(&editor), layouts_after);
            if step[5] & 128 != 0
                && editor.caret_outline().is_none()
                && !editor.active_outline().title
            {
                let outlines = editor
                    .outlines()
                    .iter()
                    .map(|outline| {
                        (
                            outline.id,
                            outline.layout().clone(),
                            outline.document().clone(),
                        )
                    })
                    .collect::<Vec<_>>();
                let id = editor.active_outline().id;
                editor.select_all().unwrap();
                let selection = editor.selection();
                editor.delete(engine, true).unwrap();
                assert!(editor.outlines().iter().all(|outline| outline.id != id));
                assert_eq!(editor.caret_outline().unwrap().id, id);
                editor.place_caret(engine, [50.0, 90.0], width).unwrap();
                for _ in 0..2 {
                    assert!(editor.undo(engine).unwrap());
                    assert_eq!(editor.selection(), selection);
                    assert_eq!(
                        editor
                            .outlines()
                            .iter()
                            .map(|outline| {
                                (
                                    outline.id,
                                    outline.layout().clone(),
                                    outline.document().clone(),
                                )
                            })
                            .collect::<Vec<_>>(),
                        outlines
                    );
                    assert!(editor.redo(engine).unwrap());
                    assert_eq!(editor.caret_outline().unwrap().id, id);
                }
                assert!(editor.undo(engine).unwrap());
            }
            if step[6] & 1 != 0 && !editor.active_outline().title {
                let width = [1.0, 10.0, 72.0, 240.0][usize::from(step[7]) % 4];
                let before = editor.active_outline().layout().clone();
                let document = editor.active_outline().document().clone();
                let selection = editor.selection();
                let preview = editor.preview_resize(engine, width).unwrap();
                assert_eq!(editor.active_outline().layout(), &before);
                assert_eq!(editor.active_outline().document(), &document);
                editor.resize(engine, width).unwrap();
                assert_eq!(editor.active_outline().layout(), preview.layout());
                if before != *preview.layout() && editor.caret_outline().is_none() {
                    editor.undo(engine).unwrap();
                    assert_eq!(editor.active_outline().layout(), &before);
                    assert_eq!(editor.active_outline().document(), &document);
                    assert_eq!(editor.selection(), selection);
                    editor.redo(engine).unwrap();
                    assert_eq!(editor.active_outline().layout(), preview.layout());
                }
            }
            if step[6] & 2 != 0 {
                let before = editor
                    .outlines()
                    .iter()
                    .map(|outline| (outline.id, outline.document().clone()))
                    .collect::<Vec<_>>();
                let movement = [
                    Movement::Left,
                    Movement::Right,
                    Movement::Up,
                    Movement::Down,
                    Movement::WordLeft,
                    Movement::WordRight,
                    Movement::LineStart,
                    Movement::LineEnd,
                    Movement::ParagraphStart,
                    Movement::ParagraphEnd,
                    Movement::DocumentStart,
                    Movement::DocumentEnd,
                ][usize::from(step[7] >> 2) % 12];
                editor
                    .move_selection(engine, movement, step[6] & 4 != 0)
                    .unwrap();
                assert_eq!(
                    editor
                        .outlines()
                        .iter()
                        .map(|outline| (outline.id, outline.document().clone()))
                        .collect::<Vec<_>>(),
                    before
                );
                let caret = editor.caret(1.0).unwrap();
                for unit in [
                    SelectionUnit::Grapheme,
                    SelectionUnit::Word,
                    SelectionUnit::Paragraph,
                ] {
                    let selection = editor
                        .selection_at(caret.x0 as f32, ((caret.y0 + caret.y1) * 0.5) as f32, unit)
                        .unwrap();
                    editor.select(selection).unwrap();
                }
            }
            if step[5] & 32 != 0 {
                let id = editor.active_outline().id;
                let point = editor
                    .outlines()
                    .iter()
                    .find(|outline| outline.id == id)
                    .map(|outline| {
                        [
                            outline.bounds().width() as f32 * 0.5,
                            outline.bounds().height() as f32
                                + [1.0, 20.0, 26.0, 28.0][usize::from(step[0]) % 4],
                        ]
                    })
                    .unwrap_or([0.0; 2]);
                let stored = editor
                    .outlines()
                    .iter()
                    .map(|outline| {
                        (
                            outline.id,
                            outline.document().clone(),
                            outline.layout().clone(),
                        )
                    })
                    .collect::<Vec<_>>();
                let before = editor.active_outline().document().clone();
                let selection = editor.selection();
                let extended = editor.select_below(engine, id, point).unwrap();
                assert_eq!(
                    editor
                        .outlines()
                        .iter()
                        .map(|outline| (
                            outline.id,
                            outline.document().clone(),
                            outline.layout().clone()
                        ))
                        .collect::<Vec<_>>(),
                    stored
                );
                if !extended {
                    assert_eq!(editor.active_outline().document(), &before);
                    assert_eq!(editor.selection(), selection);
                }
            }
            if editor
                .outlines()
                .iter()
                .find(|outline| outline.id == editor.active_outline().id)
                .is_some_and(|outline| outline.document() != editor.active_outline().document())
            {
                let pending = editor.active_outline().document().clone();
                let stored = editor
                    .outlines()
                    .iter()
                    .map(|outline| (outline.id, outline.document().clone()))
                    .collect::<Vec<_>>();
                editor
                    .move_selection(engine, Movement::DocumentEnd, false)
                    .unwrap();
                let selection = editor.selection();
                let mut expected = pending
                    .paragraphs()
                    .map(|paragraph| paragraph.text())
                    .collect::<Vec<_>>()
                    .join("\n");
                expected.push('x');
                editor.insert(engine, "x").unwrap();
                assert_eq!(
                    editor
                        .active_outline()
                        .document()
                        .paragraphs()
                        .map(|paragraph| paragraph.text())
                        .collect::<Vec<_>>()
                        .join("\n"),
                    expected
                );
                assert!(editor.undo(engine).unwrap());
                assert_eq!(editor.active_outline().document(), &pending);
                assert_eq!(editor.selection(), selection);
                assert_eq!(
                    editor
                        .outlines()
                        .iter()
                        .map(|outline| (outline.id, outline.document().clone()))
                        .collect::<Vec<_>>(),
                    stored
                );
                editor.place_caret(engine, [0.0; 2], width).unwrap();
            }
            if step[6] & 8 != 0 && editor.caret_outline().is_none() {
                let before = editor.active_outline().document().clone();
                let selection = editor.selection();
                match editor.indent(engine, step[6] & 16 != 0) {
                    Ok(true) => {
                        let indented = editor.active_outline().document().clone();
                        assert!(editor.undo(engine).unwrap());
                        assert_eq!(editor.active_outline().document(), &before);
                        assert_eq!(editor.selection(), selection);
                        assert!(editor.redo(engine).unwrap());
                        assert_eq!(editor.active_outline().document(), &indented);
                        assert!(editor.undo(engine).unwrap());
                    }
                    Ok(false) | Err(_) => {
                        assert_eq!(editor.active_outline().document(), &before);
                        assert_eq!(editor.selection(), selection);
                    }
                }
            }
            if step[6] & 32 != 0 {
                let provisional = editor.caret_outline().is_some();
                let stored = editor
                    .outlines()
                    .iter()
                    .map(|outline| (outline.id, outline.document().clone()))
                    .collect::<Vec<_>>();
                let before = editor.active_outline().document().clone();
                let selection = editor.selection();
                let movement = [
                    Movement::WordLeft,
                    Movement::WordRight,
                    Movement::LineStart,
                    Movement::LineEnd,
                ][usize::from(step[7]) % 4];
                if editor.delete_to(engine, movement).unwrap() {
                    if provisional && editor.active_outline().is_empty() {
                        assert_eq!(
                            editor
                                .outlines()
                                .iter()
                                .map(|outline| (outline.id, outline.document().clone()))
                                .collect::<Vec<_>>(),
                            stored
                        );
                    } else {
                        assert!(editor.undo(engine).unwrap());
                        assert_eq!(editor.active_outline().document(), &before);
                        assert_eq!(editor.selection(), selection);
                    }
                } else {
                    assert_eq!(editor.active_outline().document(), &before);
                    assert_eq!(editor.selection(), selection);
                }
            }
            if step[5] & 64 != 0
                && let Some(date) = editor.date()
            {
                let before = date.source().paragraphs.clone();
                let timestamp = date.timestamp();
                let selection = editor.selection();
                let updated = timestamp.wrapping_add(1 + u64::from(step[0]));
                assert!(
                    editor
                        .change_date(engine, updated, [text.into(), "7:15 AM".into()])
                        .unwrap()
                );
                let after = editor.date().unwrap().source().paragraphs.clone();
                assert_eq!(editor.selection(), selection);
                assert!(editor.undo(engine).unwrap());
                assert_eq!(editor.date().unwrap().source().paragraphs, before);
                assert_eq!(editor.date().unwrap().timestamp(), timestamp);
                assert!(editor.redo(engine).unwrap());
                assert_eq!(editor.date().unwrap().source().paragraphs, after);
                assert_eq!(editor.date().unwrap().timestamp(), updated);
                let date = editor.date().unwrap();
                let rebuilt =
                    PageDate::new(updated, date.source().clone(), engine, &BTreeMap::new())
                        .unwrap();
                assert_eq!(date.layout().size, rebuilt.layout().size);
            }
            let rebuilt = TextOutline::from_outline(
                engine,
                &editor.active_outline().snapshot(),
                &BTreeMap::new(),
            )
            .unwrap();
            let geometry = |outline: &TextOutline| {
                outline
                    .layouts()
                    .map(|(index, paragraph)| {
                        (
                            index,
                            paragraph.origin,
                            paragraph
                                .text
                                .lines()
                                .map(|(line, bounds)| {
                                    (
                                        bounds.source.clone(),
                                        bounds.top,
                                        bounds.baseline,
                                        bounds.height,
                                        line.metrics().advance,
                                    )
                                })
                                .collect::<Vec<_>>(),
                        )
                    })
                    .collect::<Vec<_>>()
            };
            assert_eq!(geometry(editor.active_outline()), geometry(&rebuilt));
            assert_eq!(editor.active_outline().bounds(), rebuilt.bounds());
            let caret = editor.caret(1.0).unwrap();
            assert!(
                [caret.x0, caret.y0, caret.x1, caret.y1]
                    .iter()
                    .all(|v| v.is_finite())
            );
        }
    });
});
