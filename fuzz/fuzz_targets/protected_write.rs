#![no_main]
//! Chains page edits on a protected section: every written image unlocks with the same
//! password, reads back as the edited model and stores none of the new text in clear.
use libfuzzer_sys::fuzz_target;
use onestore::{
    ExGuid, PreparedEdit, RevisionIndex, Store,
    page::{Page, PageObject, Paragraph, text::Edit},
    protected::{Limits, UnlockedSection},
};

const SOURCE: &[u8] =
    include_bytes!("../../corpus/native-encrypted/encrypted-01/notebook/synthetic.one");
const PASSWORD: &str = "fictitious-only";

fn page(bytes: &[u8]) -> (ExGuid, Page) {
    let store = Store::parse(bytes).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let unlocked = UnlockedSection::open(&index, PASSWORD, Limits::default()).unwrap();
    let document = unlocked.document().unwrap();
    let (space, _) = document.pages().unwrap()[0];
    (space, Page::from_space(&document, space).unwrap())
}

fuzz_target!(|data: &[u8]| {
    let mut bytes = SOURCE.to_vec();
    for step in data.chunks(12).take(4) {
        if step.len() < 4 {
            return;
        }
        let (space, mut after) = page(&bytes);
        let mut texts: Vec<_> = after
            .objects
            .iter_mut()
            .filter_map(|object| match object {
                PageObject::Outline(outline) => Some(&mut outline.paragraphs),
                _ => None,
            })
            .flatten()
            .filter_map(|paragraph| paragraph.text_mut())
            .collect();
        let count = texts.len();
        let text = &mut texts[usize::from(step[0]) % count].text;
        let end = text.utf16_offset(text.text().len()).unwrap();
        let start = u32::from(step[1]) % (end + 1);
        let stop = (start + u32::from(step[2]) % 8).min(end);
        // Marked so its absence from the stored bytes is checkable.
        let inserted = format!("\u{1f512}sealed\u{1f512}{}", String::from_utf8_lossy(&step[3..]).replace('\0', ""));
        let format = text.format_at(start.min(end.saturating_sub(1))).unwrap().clone();
        let edit = Edit {
            range: start..stop,
            replacement: Paragraph::new(inserted.clone(), format),
        };
        let (Ok(_), Ok(_)) = (text.byte_offset(start), text.byte_offset(stop)) else {
            return;
        };
        if text.apply(edit).is_err() {
            return;
        }
        let Ok(edit) = PreparedEdit::page_protected(&bytes, PASSWORD, space, &after, "Fuzz") else {
            return;
        };
        let written = edit.as_bytes().to_vec();
        let clear: Vec<u8> = inserted.encode_utf16().flat_map(u16::to_le_bytes).collect();
        assert!(!written[bytes.len()..].windows(clear.len()).any(|w| w == clear));
        let (_, stored) = page(&written);
        assert!(stored.objects == after.objects);
        bytes = written;
    }
});
