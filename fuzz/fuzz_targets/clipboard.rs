#![no_main]

use canvas::{
    document::TextDocument,
    editor::{CanvasEditor, Clip},
    layout::TextEngine,
};
use libfuzzer_sys::fuzz_target;
use onestore::{document::Format, page::text::Paragraph};
use std::cell::RefCell;

thread_local! {
    static ENGINE: RefCell<TextEngine> = RefCell::new(TextEngine::default());
}

fuzz_target!(|bytes: &[u8]| {
    if let Ok(text) = serde_json::from_slice::<Paragraph>(bytes) {
        let _ = text.project();
        let _ = text.format_at(0);
    }
    if let Ok(json) = std::str::from_utf8(bytes)
        && let Some(clip) = Clip::decode(json)
    {
        let _ = clip.text();
        ENGINE.with_borrow_mut(|engine| {
            let document =
                TextDocument::new(vec![Paragraph::new(String::new(), Format::default())]).unwrap();
            let mut editor = CanvasEditor::new(engine, document, 400.0).unwrap();
            let _ = editor.paste_clip(engine, clip);
        });
    }
});
