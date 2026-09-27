#![no_main]
use libfuzzer_sys::fuzz_target;
use onestore::{
    CommitState, ExGuid, RevisionIndex, Store, TextAttribute as A,
    document::{Document, Kind},
    op::{Edit, Op, PageOp},
};
use std::sync::LazyLock;

#[path = "../../crates/onestore/tests/support/current.rs"]
mod current;
#[path = "../../crates/onestore/tests/support/disk.rs"]
mod disk;
#[path = "../../crates/onestore/tests/support/ops.rs"]
mod ops;

static SOURCE: LazyLock<(Vec<u8>, ExGuid, ExGuid)> = LazyLock::new(|| {
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
    (bytes, space, outline)
});

fuzz_target!(|input: &[u8]| {
    let (source, space, outline) = &*SOURCE;
    if let Ok(edit) = serde_json::from_slice::<Edit>(input)
        && let Ok(edited) = ops::apply(source, "Insertion fuzz", edit.ops)
    {
        current::current(edited.as_bytes());
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
        let (insert, text_object) = if step[1] & 1 == 0 {
            let paragraph = ops::paragraph(text);
            let id = paragraph.text().unwrap().id;
            let insert = PageOp::Insert {
                container: *outline,
                before: None,
                paragraphs: vec![paragraph],
            };
            (insert, id)
        } else {
            let (add, _, id) = ops::new_outline(72.0, 144.0, text);
            (add, id)
        };
        let enabled = step[5] & 1 != 0;
        let mut page_ops = vec![insert];
        if start != end || text.is_empty() {
            page_ops.push(PageOp::Format {
                text: text_object,
                range: offsets[start]..offsets[end],
                set: vec![A::Bold(enabled), A::FontSize(18.0)],
                clear: Vec::new(),
            });
        }
        let edit = Edit {
            at: ops::now(),
            ops: page_ops
                .into_iter()
                .map(|op| Op::Page { space: *space, op })
                .collect(),
        };
        let encoded = serde_json::to_vec(&edit).unwrap();
        let edit: Edit = serde_json::from_slice(&encoded).unwrap();
        let edit = ops::apply(source, "Insertion fuzz", edit.ops).unwrap();
        let store = Store::parse(edit.as_bytes()).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let document = Document::parse(&index).unwrap();
        let s = &document.spaces[space];
        let view = &s.revisions[&s.contexts[&ExGuid::default()]];
        let actual: Vec<_> = view
            .text_runs(text_object)
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
