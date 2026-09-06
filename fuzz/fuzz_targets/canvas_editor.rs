#![no_main]
use libfuzzer_sys::fuzz_target;
use one_canvas::{
    document::{TextDocument, TextPosition},
    editor::{CanvasEditor, TextOutline},
    layout::TextEngine,
    text::Paragraph,
};
use onestore::document::Format;
use std::cell::RefCell;

thread_local! {
    static ENGINE: RefCell<TextEngine> = RefCell::new(TextEngine::default());
}

fuzz_target!(|input: &[u8]| {
    ENGINE.with_borrow_mut(|engine| {
        let mut width =
            [1.0, 10.0, 72.0, 240.0][usize::from(input.first().copied().unwrap_or(0)) % 4];
        let document = TextDocument::new(
            ["office e\u{301} 👩🏽‍💻", "العربية 日本"]
                .map(|text| Paragraph::new(text.into(), Format::default()))
                .into(),
        )
        .unwrap();
        let mut editor = CanvasEditor::new(engine, document, width).unwrap();
        for step in input.chunks_exact(8).take(32) {
            if step[6] & 128 != 0 {
                editor.place_caret(engine, [0.0; 2], width).unwrap();
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
            let selection = editor.selection();
            let text = [
                "",
                "a",
                "e\u{301}",
                "👩🏽‍💻",
                "\n",
                "left\nright",
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
            let after_selection = editor.selection();
            assert!(editor.undo(engine).unwrap());
            assert_eq!(editor.active_outline().document(), &before);
            assert_eq!(editor.selection(), selection);
            assert!(editor.redo(engine).unwrap());
            assert_eq!(editor.active_outline().document(), &after);
            assert_eq!(editor.selection(), after_selection);
            if step[5] & 128 != 0 && editor.caret_outline().is_none() {
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
            if step[6] & 1 != 0 {
                width = [1.0, 10.0, 72.0, 240.0][usize::from(step[7]) % 4];
                editor.resize(engine, width).unwrap();
            }
            let rebuilt = CanvasEditor::new(engine, after, width).unwrap();
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
            assert_eq!(
                geometry(editor.active_outline()),
                geometry(rebuilt.active_outline())
            );
            assert_eq!(
                editor.active_outline().bounds(),
                rebuilt.active_outline().bounds()
            );
            let caret = editor.caret(1.0).unwrap();
            assert!(
                [caret.x0, caret.y0, caret.x1, caret.y1]
                    .iter()
                    .all(|v| v.is_finite())
            );
        }
    });
});
