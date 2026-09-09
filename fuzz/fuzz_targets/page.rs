#![no_main]
use libfuzzer_sys::fuzz_target;
use onestore::{
    CommitState, ExGuid, Insertion, PageCreation, PreparedEdit, RevisionIndex, Store,
    document::{Document, Kind},
};
use std::sync::LazyLock;

#[path = "../../crates/onestore/tests/support/current.rs"]
mod current;
#[path = "../../crates/onestore/tests/support/disk.rs"]
mod disk;

static SOURCE: LazyLock<Vec<u8>> = LazyLock::new(|| {
    onestore::create_section("pages.one", "Original 🦀 é 東京", "Author").unwrap()
});

fuzz_target!(|input: &[u8]| {
    if let Ok(intent) = serde_json::from_slice::<PageCreation>(input)
        && let Ok(prepared) = PreparedEdit::create_page(&SOURCE, &intent)
    {
        current::current(prepared.as_bytes());
    }
    let mut persisted = SOURCE.clone();
    let mut caches = std::array::from_fn::<_, 12, _>(|_| SOURCE.clone());
    for step in input.chunks_exact(8).take(24) {
        let actor = usize::from(step[0]) % caches.len();
        if step[1] % 3 == 0 {
            caches[actor].clone_from(&persisted);
        }
        let source = &caches[actor];
        let store = Store::parse(source).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let document = Document::parse(&index).unwrap();
        let mut pages = document.pages().unwrap();
        let (sid, page) = pages[usize::from(step[2]) % pages.len()];
        let space = &document.spaces[&sid];
        let view = &space.revisions[&space.contexts[&ExGuid::default()]];
        let text = ["", "Same title", "é 🦋 東京", "  spaces  "][usize::from(step[3]) % 4];
        let titles: Vec<_> = view
            .nodes
            .iter()
            .filter_map(|(id, node)| {
                (node.extra[0].iter().any(|field| field.id == 0x88001cb4)
                    && matches!(
                        node.kind,
                        Kind::RichText {
                            boilerplate: false,
                            ..
                        }
                    ))
                .then_some(*id)
            })
            .collect();
        let (edit, title_update) = if step[1] & 8 != 0 && !titles.is_empty() {
            let object = titles[0];
            let Kind::RichText { text: original, .. } = &view.nodes[&object].kind else {
                unreachable!()
            };
            (
                PreparedEdit::text(
                    source,
                    sid,
                    object,
                    0..original.encode_utf16().count() as u32,
                    text,
                )
                .unwrap(),
                Some((sid, object, text)),
            )
        } else if step[1] & 16 != 0 {
            let intent = Insertion::outline(page, 36.0, 36.0, text, "Page fuzz").unwrap();
            (
                PreparedEdit::insert(source, sid, &intent).unwrap(),
                Some((sid, intent.text_object(), text)),
            )
        } else {
            let before = (step[2] & 1 != 0).then_some(sid);
            let title = (step[3] & 4 == 0).then_some(text);
            let intent = PageCreation::new(before, title, "Page fuzz").unwrap();
            let restored = serde_json::from_value(serde_json::to_value(&intent).unwrap()).unwrap();
            assert_eq!(intent, restored);
            let edit = PreparedEdit::create_page(source, &restored).unwrap();
            let position = before.map_or(pages.len(), |sid| {
                pages.iter().position(|p| p.0 == sid).unwrap()
            });
            pages.insert(position, (intent.space(), intent.object()));
            (
                edit,
                intent
                    .title_object()
                    .map(|object| (intent.space(), object, text)),
            )
        };
        let after_store = Store::parse(edit.as_bytes()).unwrap();
        let after_index = RevisionIndex::parse(&after_store).unwrap();
        let after_document = Document::parse(&after_index).unwrap();
        assert_eq!(after_document.pages().unwrap(), pages);
        if let Some((sid, object, text)) = title_update {
            let space = &after_document.spaces[&sid];
            let view = &space.revisions[&space.contexts[&ExGuid::default()]];
            assert!(
                matches!(&view.nodes[&object].kind, Kind::RichText { text: actual, .. } if actual == text)
            );
        }
        for (sid, space) in &index.spaces {
            let rid = space.labels[&(ExGuid::default(), 1)];
            assert_eq!(
                format!("{:?}", index.resolve(*sid, rid).unwrap()),
                format!("{:?}", after_index.resolve(*sid, rid).unwrap())
            );
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
