#![no_main]
use libfuzzer_sys::fuzz_target;
use onestore::{
    CommitState, ExGuid, Insertion, PreparedEdit, RevisionIndex, Store, TextAttribute as A,
    document::{Document, Kind},
};
use std::sync::LazyLock;

#[path = "../../crates/onestore/tests/support/current.rs"]
mod current;
#[path = "../../crates/onestore/tests/support/disk.rs"]
mod disk;

static SOURCE: LazyLock<(Vec<u8>, ExGuid, ExGuid, ExGuid)> = LazyLock::new(|| {
    let bytes = include_bytes!(
        "../../corpus/native/20260905-05/snapshots/03-format-unicode/notebook/synthetic.one"
    )
    .to_vec();
    let store = Store::parse(&bytes).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let (space, page) = document.pages().unwrap()[0];
    let s = &document.spaces[&space];
    let view = &s.revisions[&s.contexts[&ExGuid::default()]];
    let outline = *view.nodes[&page]
        .children
        .iter()
        .find(|id| matches!(view.nodes[id].kind, Kind::Outline { .. }))
        .unwrap();
    (bytes, space, page, outline)
});

fuzz_target!(|input: &[u8]| {
    let (source, space, page, outline) = &*SOURCE;
    if let Ok(intent) = serde_json::from_slice::<Insertion>(input)
        && let Ok(prepared) = PreparedEdit::insert(source, *space, &intent)
    {
        current::current(prepared.as_bytes());
    }
    let mut persisted = source.clone();
    let mut caches = std::array::from_fn::<_, 12, _>(|_| source.clone());
    for step in input.chunks_exact(8).take(12) {
        let actor = usize::from(step[0]) % caches.len();
        if step[1] % 4 == 0 {
            caches[actor].clone_from(&persisted);
        }
        let source = &caches[actor];
        let text =
            ["", "ab", "🦀e\u{301}東京\rEnd", "same style same style"][usize::from(step[2]) % 4];
        let offsets: Vec<u32> = std::iter::once(0)
            .chain(text.chars().scan(0, |n, c| {
                *n += c.len_utf16() as u32;
                Some(*n)
            }))
            .collect();
        let first = usize::from(step[3]) % offsets.len();
        let second = usize::from(step[4]) % offsets.len();
        let (start, end) = (first.min(second), first.max(second));
        let plain = if step[1] & 1 == 0 {
            Insertion::paragraph(*outline, None, text, "Insertion fuzz").unwrap()
        } else {
            Insertion::outline(*page, 72.0, 144.0, text, "Insertion fuzz").unwrap()
        };
        let enabled = step[5] & 1 != 0;
        let intent = if start != end || text.is_empty() {
            plain
                .with_formatting(
                    offsets[start]..offsets[end],
                    &[A::Bold(enabled), A::FontSize(18.0)],
                )
                .unwrap()
        } else {
            plain
        };
        let encoded = serde_json::to_vec(&intent).unwrap();
        let intent: Insertion = serde_json::from_slice(&encoded).unwrap();
        let edit = PreparedEdit::insert(source, *space, &intent).unwrap();
        let store = Store::parse(edit.as_bytes()).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let document = Document::parse(&index).unwrap();
        let s = &document.spaces[space];
        let view = &s.revisions[&s.contexts[&ExGuid::default()]];
        let actual: Vec<_> = view
            .text_runs(intent.text_object())
            .unwrap()
            .into_iter()
            .flat_map(|run| {
                run.text.chars().map(move |c| {
                    (
                        c,
                        run.format.bold.unwrap_or(false),
                        run.format.font_size.unwrap(),
                    )
                })
            })
            .collect();
        let expected: Vec<_> = text
            .chars()
            .enumerate()
            .map(|(i, c)| {
                if (start..end).contains(&i) {
                    (c, enabled, 18.0)
                } else {
                    (c, false, 11.0)
                }
            })
            .collect();
        assert_eq!(actual, expected);
        let before = current::current(&persisted);
        let after = current::current(edit.as_bytes());
        let mut disk = disk::Disk {
            visible: persisted.clone(),
            durable: persisted.clone(),
            operation: 0,
            fail_at: (step[6] != 0).then_some(usize::from(step[6])),
            write_limit: if step[7] & 1 == 0 { 17 } else { 4096 },
            random: u64::from(step[7]) + 1,
        };
        let result = edit.commit(&mut disk);
        let observed = current::current(&disk.durable);
        match result {
            Ok(()) => assert_eq!(observed, after),
            Err(error) => {
                assert!(observed == before || observed == after);
                if error.state == CommitState::NotCommitted {
                    assert_eq!(observed, before);
                }
                if error.state == CommitState::Committed {
                    assert_eq!(observed, after);
                }
            }
        }
        persisted = disk.durable;
    }
});
