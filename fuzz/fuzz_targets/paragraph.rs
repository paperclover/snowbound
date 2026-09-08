#![no_main]
use libfuzzer_sys::fuzz_target;
use onestore::{
    CommitState, ExGuid, Insertion, OutlineEdit, ParagraphJoin, ParagraphSplit, PreparedEdit,
    RevisionIndex, Store, TextAttribute as A,
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
    if let Ok(operation) = serde_json::from_slice::<OutlineEdit>(input)
        && let Ok(edit) = PreparedEdit::outline(source, *sid, *outline, operation)
    {
        current::current(edit.as_bytes());
    }
    if let Ok(intent) = serde_json::from_slice::<ParagraphSplit>(input)
        && let Ok(edit) = PreparedEdit::split(source, *sid, &intent)
    {
        current::current(edit.as_bytes());
    }
    if let Ok(intent) = serde_json::from_slice::<ParagraphJoin>(input)
        && let Ok(edit) = PreparedEdit::join(source, *sid, &intent)
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
        let characters = |view: &onestore::document::Revision<'_>, id| {
            view.text_runs(id)
                .unwrap()
                .into_iter()
                .flat_map(|run| {
                    let style = serde_json::to_value(run.format).unwrap();
                    run.text.chars().map(move |c| (c, style.clone()))
                })
                .collect::<Vec<_>>()
        };
        let pairs: Vec<_> = view
            .nodes
            .iter()
            .flat_map(|(parent, node)| {
                node.children.windows(2).filter_map(|pair| {
                    (paragraphs.contains(&pair[0])
                        && paragraphs.contains(&pair[1])
                        && view.nodes[&pair[0]].children.is_empty())
                    .then_some((*parent, pair[0], pair[1]))
                })
            })
            .collect();
        let mut expected_text = Vec::new();
        let mut expected_graph = Vec::new();
        let mut expected_layout = None;
        let mut expected_collapse = None;
        let edit = if step[1] & 16 != 0 {
            let mut layout = serde_json::to_value(&view.nodes[outline].layout).unwrap();
            let (object, operation) = match (step[1] >> 5) % 3 {
                0 => {
                    let x = (f32::from(step[2]) - 64.0) * 18.0;
                    let y = f32::from(step[3]) * 18.0;
                    layout["x"] = x.into();
                    layout["y"] = y.into();
                    (*outline, OutlineEdit::Position { x, y })
                }
                1 => {
                    let points = (2.0 + f32::from(step[2])) * 18.0;
                    let user_set = step[3] & 1 != 0;
                    layout["max_width"] = points.into();
                    layout["width_set_by_user"] = user_set.into();
                    (*outline, OutlineEdit::Width { points, user_set })
                }
                _ => {
                    let paragraph = paragraphs[usize::from(step[2]) % paragraphs.len()];
                    let collapsed = step[3] & 1 != 0;
                    expected_collapse = Some((paragraph, collapsed));
                    (paragraph, OutlineEdit::Collapsed(collapsed))
                }
            };
            expected_layout = Some(layout);
            for (id, node) in &view.nodes {
                expected_graph.push((*id, node.children.clone(), node.content.clone()));
                if matches!(node.kind, Kind::RichText { .. }) {
                    expected_text.push((*id, characters(view, *id)));
                }
            }
            PreparedEdit::outline(source, *sid, object, operation).unwrap()
        } else if step[1] & 8 != 0 && !pairs.is_empty() {
            let (parent, left, right) = pairs[usize::from(step[2]) % pairs.len()];
            let a = view.nodes[&left].content[0];
            let b = view.nodes[&right].content[0];
            let mut expected = characters(view, a);
            let survivor = if expected.is_empty() { b } else { a };
            expected.extend(characters(view, b));
            expected_text.push((survivor, expected));
            let children: Vec<_> = view.nodes[&parent]
                .children
                .iter()
                .filter(|id| **id != right)
                .copied()
                .collect();
            expected_graph.push((parent, children, view.nodes[&parent].content.clone()));
            expected_graph.push((left, view.nodes[&right].children.clone(), vec![survivor]));
            let intent = ParagraphJoin::new(a, b, "Paragraph fuzz").unwrap();
            let restored = serde_json::from_value(serde_json::to_value(&intent).unwrap()).unwrap();
            assert_eq!(intent, restored);
            PreparedEdit::join(source, *sid, &restored).unwrap()
        } else {
            let paragraph = paragraphs[usize::from(step[2]) % paragraphs.len()];
            let text = view.nodes[&paragraph].content[0];
            let before = characters(view, text);
            let offsets: Vec<u32> = std::iter::once(0)
                .chain(before.iter().scan(0, |n, (c, _)| {
                    *n += u32::try_from(c.len_utf16()).unwrap();
                    Some(*n)
                }))
                .collect();
            let offset = u32::from(step[3]) % (offsets.last().unwrap() + 2);
            let intent = ParagraphSplit::new(text, offset, "Paragraph fuzz").unwrap();
            let restored = serde_json::from_value(serde_json::to_value(&intent).unwrap()).unwrap();
            assert_eq!(intent, restored);
            let edit = PreparedEdit::split(source, *sid, &restored);
            let Some(position) = offsets.iter().position(|n| *n == offset) else {
                assert!(edit.is_err());
                continue;
            };
            expected_text.push((text, before[..position].to_vec()));
            expected_text.push((intent.text_object(), before[position..].to_vec()));
            expected_graph.push((paragraph, vec![], vec![text]));
            expected_graph.push((
                intent.object(),
                view.nodes[&paragraph].children.clone(),
                vec![intent.text_object()],
            ));
            edit.unwrap()
        };
        let after_store = Store::parse(edit.as_bytes()).unwrap();
        let after_index = RevisionIndex::parse(&after_store).unwrap();
        let after_document = Document::parse(&after_index).unwrap();
        let space = &after_document.spaces[sid];
        let after_view = &space.revisions[&space.contexts[&ExGuid::default()]];
        if let Some(expected) = expected_layout {
            assert_eq!(
                serde_json::to_value(&after_view.nodes[outline].layout).unwrap(),
                expected
            );
        }
        if let Some((id, expected)) = expected_collapse {
            let Kind::Paragraph { collapse_state, .. } = after_view.nodes[&id].kind else {
                panic!()
            };
            assert_eq!(collapse_state, Some(u8::from(expected)));
        }
        for (id, expected) in expected_text {
            assert_eq!(characters(after_view, id), expected);
        }
        for (id, children, content) in expected_graph {
            assert_eq!(after_view.nodes[&id].children, children);
            assert_eq!(after_view.nodes[&id].content, content);
        }
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
