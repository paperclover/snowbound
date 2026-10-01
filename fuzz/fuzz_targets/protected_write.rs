#![no_main]
//! Chains text edits on a protected section kept open under its key, now and then writing
//! it anew under another password: every sealed image opens with the key, reads back as the
//! model predicts and stores none of the new text in clear. Text typed at the end of a text
//! whose empty final run keeps an insertion style takes that style, which the page model
//! cannot show, so only its characters are predicted.
use libfuzzer_sys::fuzz_target;
use onestore::{
    Arena, ExGuid, Section,
    op::{Edit, Op, PageOp, predict},
    page::{Page, PageObject},
    protected::{Key, rekey},
};
use std::sync::LazyLock;

const SOURCE: &[u8] =
    include_bytes!("../../corpus/native-encrypted/encrypted-01/notebook/synthetic.one");
static KEYS: LazyLock<[Key; 2]> = LazyLock::new(|| {
    [
        Key::open(SOURCE, "fictitious-only").unwrap(),
        Key::new("another fictitious password").unwrap(),
    ]
});

/// The first page.
fn page(bytes: &[u8], key: &Key) -> (ExGuid, Page) {
    let arena = Arena::default();
    let mut section = Section::unlock(&arena, bytes.to_vec(), key).unwrap();
    let (space, ..) = section.pages().unwrap()[0];
    (space, section.page(space).unwrap())
}

fn texts(page: &Page) -> Vec<String> {
    page.objects
        .iter()
        .filter_map(|object| match object {
            PageObject::Outline(outline) => Some(&outline.paragraphs),
            _ => None,
        })
        .flatten()
        .filter_map(|paragraph| Some(paragraph.text()?.text.text().to_owned()))
        .collect()
}

fuzz_target!(|data: &[u8]| {
    let mut bytes = SOURCE.to_vec();
    let mut key = &KEYS[0];
    for (at, step) in (0..).zip(data.chunks(12).take(4)) {
        if step.len() < 4 {
            return;
        }
        if step[3] == 0xff {
            let other = &KEYS[usize::from(std::ptr::eq(key, &KEYS[0]))];
            let before = page(&bytes, key).1;
            bytes = rekey(&bytes, Some(key), Some(other)).unwrap();
            key = other;
            assert_eq!(texts(&page(&bytes, key).1), texts(&before));
            continue;
        }
        let (space, mut expected) = page(&bytes, key);
        let candidates: Vec<_> = expected
            .objects
            .iter()
            .filter_map(|object| match object {
                PageObject::Outline(outline) => Some(&outline.paragraphs),
                _ => None,
            })
            .flatten()
            .filter_map(|paragraph| paragraph.text())
            .collect();
        let text = candidates[usize::from(step[0]) % candidates.len()];
        let end = text.text.utf16_offset(text.text.text().len()).unwrap();
        let start = u32::from(step[1]) % (end + 1);
        let stop = (start + u32::from(step[2]) % 8).min(end);
        // Marked so its absence from the stored bytes is checkable.
        let inserted = format!(
            "\u{1f512}sealed\u{1f512}{}",
            String::from_utf8_lossy(&step[3..]).replace(['\0', '\n', '\u{fffc}'], "")
        );
        let op = PageOp::Text {
            text: text.id,
            range: start..stop,
            with: inserted.clone(),
        };
        if predict(&mut expected, &op).is_err() {
            return;
        }
        let edit = Edit {
            at: 133_000_000_000_000_000 + at * 10_000_000,
            ops: vec![Op::Page { space, op }],
        };
        let arena = Arena::default();
        let mut section = Section::unlock(&arena, bytes.clone(), key).unwrap();
        section.apply("Fuzz", &edit).unwrap();
        let transaction = section.seal().unwrap().unwrap();
        let mut written = bytes.clone();
        transaction.apply(&mut written).unwrap();
        let clear: Vec<u8> = inserted.encode_utf16().flat_map(u16::to_le_bytes).collect();
        assert!(
            !written[bytes.len()..]
                .windows(clear.len())
                .any(|w| w == clear)
        );
        assert_eq!(texts(&page(&written, key).1), texts(&expected));
        bytes = written;
    }
});
