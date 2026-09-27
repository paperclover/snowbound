#![no_main]
//! Chains text edits on a protected section: every written image unlocks with the same
//! password, reads back as the model predicts and stores none of the new text in clear.
//! Text typed at the end of a text whose empty final run keeps an insertion style takes
//! that style, which the page model cannot show, so only its characters are predicted.
use libfuzzer_sys::fuzz_target;
use onestore::{
    ExGuid, RevisionIndex, Store,
    document::Kind,
    op::{Edit, Op, PageOp, predict},
    page::{Page, PageObject},
    protected::{Limits, UnlockedSection},
};

const SOURCE: &[u8] =
    include_bytes!("../../corpus/native-encrypted/encrypted-01/notebook/synthetic.one");
const PASSWORD: &str = "fictitious-only";

/// The first page, and the texts whose empty final run keeps an insertion style.
fn page(bytes: &[u8]) -> (ExGuid, Page, Vec<ExGuid>) {
    let store = Store::parse(bytes).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let unlocked = UnlockedSection::open(&index, PASSWORD, Limits::default()).unwrap();
    let document = unlocked.document().unwrap();
    let (space, _) = document.pages().unwrap()[0];
    let view = &document.spaces[&space];
    let hidden = view.revisions[&view.contexts[&ExGuid::default()]]
        .nodes
        .iter()
        .filter(|(_, node)| {
            matches!(&node.kind, Kind::RichText { text, runs, .. }
                if !text.is_empty() && runs.len() > 1 && runs.last().is_some_and(|r| r.start == r.end))
        })
        .map(|(id, _)| *id)
        .collect();
    (space, Page::from_space(&document, space).unwrap(), hidden)
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
    for (at, step) in (0..).zip(data.chunks(12).take(4)) {
        if step.len() < 4 {
            return;
        }
        let (space, mut expected, hidden) = page(&bytes);
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
        let styled = hidden.contains(&text.id) && stop == end;
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
        let store = Store::parse(&bytes).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let unlocked = UnlockedSection::open(&index, PASSWORD, Limits::default()).unwrap();
        let transaction = unlocked.apply("Fuzz", &edit).unwrap();
        let mut written = bytes.clone();
        transaction.apply(&mut written).unwrap();
        let clear: Vec<u8> = inserted.encode_utf16().flat_map(u16::to_le_bytes).collect();
        assert!(
            !written[bytes.len()..]
                .windows(clear.len())
                .any(|w| w == clear)
        );
        let (_, stored, _) = page(&written);
        if styled {
            assert_eq!(texts(&stored), texts(&expected));
        } else {
            assert!(stored.objects == expected.objects);
        }
        bytes = written;
    }
});
