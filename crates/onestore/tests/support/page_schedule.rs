use onestore::{
    CommitState, ExGuid, PageCreation, PageEdit, RevisionIndex, Store,
    document::{Document, FieldValue, Kind},
    op::{PageOp, SectionOp},
};
use std::{collections::BTreeMap, sync::LazyLock};

#[path = "current.rs"]
pub(crate) mod current;
#[path = "disk.rs"]
pub(crate) mod disk;
#[path = "ops.rs"]
pub(crate) mod ops;

static SOURCE: LazyLock<Vec<u8>> = LazyLock::new(|| {
    onestore::create_section("pages.one", "Original 🦀 é 東京", "Author").unwrap()
});

pub fn run(input: &[u8]) {
    let source = match input.first().copied().unwrap_or(0) % 5 {
        1 => include_bytes!("../../../../corpus/page-lifecycle/page-edits/optional-cache/source/Lifecycle.one")
            .as_slice(),
        2 => include_bytes!("../../../../corpus/page-lifecycle/page-edits/optional-cache/source-cold/notebook/Lifecycle.one")
            .as_slice(),
        3 => include_bytes!("../../../../corpus/page-lifecycle/removal/features/native/before/notebook/Lifecycle.one").as_slice(),
        4 => include_bytes!("../../../../corpus/page-lifecycle/removal/ink/native/before/notebook/Lifecycle.one").as_slice(),
        _ => SOURCE.as_slice(),
    };
    if let Ok(intent) = serde_json::from_slice::<PageCreation>(input)
        && let Ok(prepared) = ops::section_op(source, SectionOp::Create(intent.clone()))
    {
        current::current(prepared.as_bytes());
    }
    if let Ok(edits) = serde_json::from_slice::<Vec<PageEdit>>(input)
        && let Ok(prepared) = ops::section_op(source, SectionOp::Pages(edits.to_vec()))
    {
        current::current(prepared.as_bytes());
    }
    if let Ok(pages) = serde_json::from_slice::<Vec<ExGuid>>(input)
        && let Ok(prepared) = ops::section_op(source, SectionOp::Delete(pages.to_vec()))
    {
        current::current(prepared.as_bytes());
    }
    let mut persisted = source.to_vec();
    let mut caches = std::array::from_fn::<_, 12, _>(|_| source.to_vec());
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
        let mut levels: BTreeMap<_, _> = pages
            .iter()
            .map(|(sid, _)| {
                let space = &document.spaces[sid];
                let view = &space.revisions[&space.contexts[&ExGuid::default()]];
                let Kind::Metadata { level, .. } = view.nodes[&view.roots[&2]].kind else {
                    panic!()
                };
                (*sid, level.unwrap_or(1))
            })
            .collect();
        let selected = pages
            .get(usize::from(step[2]) % pages.len().max(1))
            .copied();
        let text = ["", "Same title", "é 🦋 東京", "  spaces  "][usize::from(step[3]) % 4];
        let existing = if let Some((sid, _)) = selected {
            let space = &document.spaces[&sid];
            let view = &space.revisions[&space.contexts[&ExGuid::default()]];
            let titles: Vec<_> = view
                .nodes
                .iter()
                .filter_map(|(id, node)| {
                    (matches!(
                        node.kind,
                        Kind::RichText {
                            boilerplate: false,
                            ..
                        }
                    ) && node.extra[0].iter().any(|field| field.id == 0x88001cb4))
                    .then_some(*id)
                })
                .collect();
            if step[1] & 64 != 0 {
                let count = if step[3] & 128 != 0 {
                    pages.len()
                } else {
                    1 + usize::from(step[3] & 64 != 0 && pages.len() > 1)
                };
                let removed: Vec<_> = (0..count)
                    .map(|i| pages[(usize::from(step[2]) + i) % pages.len()].0)
                    .collect();
                let prepared =
                    ops::section_op(source, SectionOp::Delete(removed.to_vec())).unwrap();
                pages.retain(|page| !removed.contains(&page.0));
                for sid in removed {
                    levels.remove(&sid);
                }
                if let Some((first, _)) = pages.first() {
                    levels.insert(*first, 1);
                }
                Some((prepared, None))
            } else if step[1] & 32 != 0 {
                let count = 1 + usize::from(step[3] & 128 != 0 && pages.len() > 1);
                let selected: Vec<_> = (0..count)
                    .map(|i| pages[(usize::from(step[2]) + i) % pages.len()].0)
                    .collect();
                let before = (step[3] & 4 != 0)
                    .then_some(pages[usize::from(step[7]) % pages.len()].0)
                    .filter(|id| !selected.contains(id));
                let mut edits = Vec::new();
                for (ordinal, sid) in selected.iter().enumerate() {
                    let level = u32::from(if ordinal == 0 { step[3] } else { step[6] }) % 3 + 1;
                    levels.insert(*sid, level);
                    let edit = if step[3] & 8 == 0 {
                        PageEdit::set_level(*sid, level).unwrap()
                    } else {
                        let at = pages.iter().position(|p| p.0 == *sid).unwrap();
                        let page = pages.remove(at);
                        let at = before.map_or(pages.len(), |id| {
                            pages.iter().position(|p| p.0 == id).unwrap()
                        });
                        pages.insert(at, page);
                        PageEdit::move_to(*sid, before, level).unwrap()
                    };
                    edits.push(edit);
                }
                let restored: Vec<PageEdit> =
                    serde_json::from_value(serde_json::to_value(&edits).unwrap()).unwrap();
                assert_eq!(edits, restored);
                let prepared = ops::section_op(source, SectionOp::Pages(restored.to_vec()));
                if levels[&pages[0].0] != 1 {
                    assert!(prepared.is_err());
                    continue;
                }
                Some((prepared.unwrap(), None))
            } else if step[1] & 8 != 0 && !titles.is_empty() {
                let object = titles[0];
                let Kind::RichText { text: original, .. } = &view.nodes[&object].kind else {
                    unreachable!()
                };
                Some((
                    ops::page_op(
                        source,
                        sid,
                        PageOp::Text {
                            text: object,
                            range: 0..u32::try_from(original.encode_utf16().count()).unwrap(),
                            with: text.into(),
                        },
                    )
                    .unwrap(),
                    Some((sid, object, text)),
                ))
            } else if step[1] & 16 != 0 {
                let (add, _, inserted) = ops::new_outline(36.0, 36.0, text);
                Some((
                    ops::page_op(source, sid, add).unwrap(),
                    Some((sid, inserted, text)),
                ))
            } else {
                None
            }
        } else {
            None
        };
        let (edit, title_update) = existing.unwrap_or_else(|| {
            let before = if step[2] & 1 != 0 {
                selected.map(|(sid, _)| {
                    let section = &document.spaces[&document.root];
                    let section = &section.revisions[&section.contexts[&ExGuid::default()]];
                    section.nodes[&section.roots[&1]]
                        .children
                        .iter()
                        .map(|id| &section.nodes[id])
                        .find(|series| series.spaces.contains(&sid))
                        .unwrap()
                        .spaces[0]
                })
            } else {
                None
            };
            let title = (step[3] & 4 == 0).then_some(text);
            let intent = PageCreation::new(before, title, "Page fuzz").unwrap();
            let restored = serde_json::from_value(serde_json::to_value(&intent).unwrap()).unwrap();
            assert_eq!(intent, restored);
            let edit = ops::section_op(source, SectionOp::Create(restored.clone())).unwrap();
            let position = before.map_or(pages.len(), |sid| {
                pages.iter().position(|p| p.0 == sid).unwrap()
            });
            pages.insert(position, (intent.space(), intent.object()));
            levels.insert(intent.space(), 1);
            (
                edit,
                intent
                    .title_object()
                    .map(|object| (intent.space(), object, text)),
            )
        });
        let after_store = Store::parse(edit.as_bytes()).unwrap();
        let after_index = RevisionIndex::parse(&after_store).unwrap();
        let after_document = Document::parse(&after_index).unwrap();
        assert_eq!(after_document.pages().unwrap(), pages);
        let mut metadata_levels = BTreeMap::new();
        for (sid, _) in &pages {
            let space = &after_document.spaces[sid];
            let view = &space.revisions[&space.contexts[&ExGuid::default()]];
            let Kind::Metadata { level, .. } = view.nodes[&view.roots[&2]].kind else {
                panic!()
            };
            assert_eq!(level.unwrap_or(1), levels[sid]);
            let metadata = &view.nodes[&view.roots[&2]];
            let FieldValue::Bytes(guid) = metadata.extra[0]
                .iter()
                .find(|field| field.id == 0x1c001c30)
                .unwrap()
                .value
            else {
                panic!()
            };
            assert!(metadata_levels.insert(guid, levels[sid]).is_none());
        }
        let section = &after_document.spaces[&after_document.root];
        let view = &section.revisions[&section.contexts[&ExGuid::default()]];
        for metadata in view.nodes.values() {
            if let Kind::Metadata { level, .. } = metadata.kind {
                let FieldValue::Bytes(guid) = metadata.extra[0]
                    .iter()
                    .find(|field| field.id == 0x1c001c30)
                    .unwrap()
                    .value
                else {
                    panic!()
                };
                assert_eq!(metadata_levels.get(guid), Some(&level.unwrap_or(1)));
            }
        }
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
        let Some(transaction) = &edit.transaction else {
            continue;
        };
        let result = transaction.commit(&mut disk);
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
}
