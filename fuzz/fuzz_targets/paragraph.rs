#![no_main]
use libfuzzer_sys::fuzz_target;
use onestore::{
    CommitState, ExGuid, Insertion, ParagraphSplit, PreparedEdit, RevisionIndex, Store,
    TextAttribute as A,
    document::{Document, Kind},
};
use std::sync::LazyLock;

#[path = "../../crates/onestore/tests/support/current.rs"]
mod current;
#[path = "../../crates/onestore/tests/support/disk.rs"]
mod disk;

static SOURCE: LazyLock<(Vec<u8>, ExGuid, ExGuid)> = LazyLock::new(|| {
    let source = onestore::create_section("paragraph.one", "Original", "Author").unwrap();
    let store = Store::parse(&source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let (sid, page) = document.pages().unwrap()[0];
    let space = &document.spaces[&sid];
    let view = &space.revisions[&space.contexts[&ExGuid::default()]];
    let outline = *view.nodes[&page]
        .children
        .iter()
        .find(|id| matches!(view.nodes[id].kind, Kind::Outline { .. }))
        .unwrap();
    let parent = view.nodes[&outline].children[0];
    let intent = Insertion::paragraph(parent, None, "a🦀 e\u{301} 東京\rEnd", "Author")
        .unwrap()
        .with_formatting(0..4, &[A::Bold(true)])
        .unwrap()
        .with_formatting(4..7, &[A::Italic(true), A::Color(Some([12, 34, 56]))])
        .unwrap();
    let edited = PreparedEdit::insert(&source, sid, &intent).unwrap();
    (edited.as_bytes().to_vec(), sid, outline)
});

fuzz_target!(|input: &[u8]| {
    let (source, sid, outline) = &*SOURCE;
    if let Ok(intent) = serde_json::from_slice::<ParagraphSplit>(input)
        && let Ok(edit) = PreparedEdit::split(source, *sid, &intent)
    {
        current::current(edit.as_bytes());
    }
    let mut persisted = source.clone();
    let mut caches = std::array::from_fn::<_, 12, _>(|_| source.clone());
    for step in input.chunks_exact(8).take(20) {
        let actor = usize::from(step[0]) % caches.len();
        if step[1] % 3 == 0 {
            caches[actor].clone_from(&persisted);
        }
        let source = &caches[actor];
        let store = Store::parse(source).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let document = Document::parse(&index).unwrap();
        let space = &document.spaces[sid];
        let view = &space.revisions[&space.contexts[&ExGuid::default()]];
        let mut pending = vec![*outline];
        let mut paragraphs = Vec::new();
        while let Some(id) = pending.pop() {
            let node = &view.nodes[&id];
            pending.extend(node.children.iter().copied());
            if matches!(node.kind, Kind::Paragraph { .. }) {
                paragraphs.push(id);
            }
        }
        let paragraph = paragraphs[usize::from(step[2]) % paragraphs.len()];
        let text = view.nodes[&paragraph].content[0];
        let characters: Vec<_> = view
            .text_runs(text)
            .unwrap()
            .into_iter()
            .flat_map(|run| {
                let style = serde_json::to_value(run.format).unwrap();
                run.text.chars().map(move |c| (c, style.clone()))
            })
            .collect();
        let offsets: Vec<u32> = std::iter::once(0)
            .chain(characters.iter().scan(0, |n, (c, _)| {
                *n += u32::try_from(c.len_utf16()).unwrap();
                Some(*n)
            }))
            .collect();
        let offset = u32::from(step[3]) % (offsets.last().unwrap() + 2);
        let intent = ParagraphSplit::new(text, offset, "Paragraph fuzz").unwrap();
        let intent = serde_json::from_value(serde_json::to_value(intent).unwrap()).unwrap();
        let edit = PreparedEdit::split(source, *sid, &intent);
        let Some(position) = offsets.iter().position(|n| *n == offset) else {
            assert!(edit.is_err());
            continue;
        };
        let edit = edit.unwrap();
        let after_store = Store::parse(edit.as_bytes()).unwrap();
        let after_index = RevisionIndex::parse(&after_store).unwrap();
        let after_document = Document::parse(&after_index).unwrap();
        let space = &after_document.spaces[sid];
        let after_view = &space.revisions[&space.contexts[&ExGuid::default()]];
        for (id, expected) in [
            (text, &characters[..position]),
            (intent.text_object(), &characters[position..]),
        ] {
            let actual: Vec<_> = after_view
                .text_runs(id)
                .unwrap()
                .into_iter()
                .flat_map(|run| {
                    let style = serde_json::to_value(run.format).unwrap();
                    run.text.chars().map(move |c| (c, style.clone()))
                })
                .collect();
            assert_eq!(actual, expected);
        }
        assert!(after_view.nodes[&paragraph].children.is_empty());
        assert_eq!(
            after_view.nodes[&intent.object()].children,
            view.nodes[&paragraph].children
        );
        let before = current::current(&persisted);
        let after = current::current(edit.as_bytes());
        let mut disk = disk::Disk {
            visible: persisted.clone(),
            durable: persisted.clone(),
            operation: 0,
            fail_at: (step[4] != 0).then_some(usize::from(u16::from_le_bytes([step[4], step[5]]))),
            write_limit: if step[6] & 1 == 0 { 17 } else { 4096 },
            random: u64::from(step[7]) + 1,
        };
        let result = edit.commit(&mut disk);
        let observed = current::current(&disk.durable);
        match result {
            Ok(()) => assert_eq!(observed, after),
            Err(error) => {
                assert!(observed == before || observed == after);
                match error.state {
                    CommitState::NotCommitted => assert_eq!(observed, before),
                    CommitState::Committed => assert_eq!(observed, after),
                    CommitState::Unknown => {}
                }
            }
        }
        persisted = disk.durable;
    }
});
