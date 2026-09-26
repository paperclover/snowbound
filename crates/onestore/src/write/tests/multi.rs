use crate::{
    ExGuid, RevisionIndex, Store,
    document::{Document, Kind},
    write::{PropertyObject, RevisionEdit, write_revisions},
};
use std::collections::BTreeMap;

mod disk {
    use crate as onestore;
    include!("../../../tests/support/disk.rs");
}

mod current {
    use crate as onestore;
    include!("../../../tests/support/current.rs");
}

const SOURCE: &[u8] =
    include_bytes!("../../../../../corpus/page-lifecycle/03-renamed/notebook/Lifecycle.one");

fn nest(source: &[u8]) -> Vec<u8> {
    write_revisions(source, |index| {
        let document = Document::parse(index)?;
        let pages = document.pages()?;
        assert_eq!(pages.len(), 9);
        let section = index.resolve(
            index.root,
            index.spaces[&index.root].labels[&(ExGuid::default(), 1)],
        )?;
        let view = &document.spaces[&index.root];
        let view = view.active().unwrap();
        let root = section.roots[&1];
        let parent = *view.nodes[&root]
            .children
            .iter()
            .find(|id| view.nodes[id].spaces.contains(&pages[3].0))
            .unwrap();
        let mut series = PropertyObject::from_object(&section.objects[&parent])?;
        let mut spaces = Vec::new();
        let mut metadata = Vec::new();
        let mut replacements = BTreeMap::new();
        let mut changes = BTreeMap::new();
        for (ordinal, (sid, page)) in pages[3..6].iter().enumerate() {
            spaces.extend_from_slice(&series.reference(*sid)?);
            let raw = index.resolve(*sid, index.spaces[sid].labels[&(ExGuid::default(), 1)])?;
            let id = raw.roots[&2];
            let mut object = PropertyObject::from_object(&raw.objects[&id])?;
            object.set(&[(0x14001dff, &((ordinal + 1) as u32).to_le_bytes())])?;
            changes.insert(*sid, BTreeMap::from([(id, object)]));
            let mut copy = PropertyObject::from_object(&raw.objects[&id])?;
            copy.set(&[(0x14001dff, &((ordinal + 1) as u32).to_le_bytes())])?;
            let mut copy_id = ExGuid {
                guid: [
                    0x31, 0xc0, 0xa8, 0x22, 0, 0x36, 0xee, 0x42, 0xb7, 0x14, 0xd7, 0xac, 0xda,
                    0x24, 0x35, 0xe8,
                ],
                n: 1,
            };
            for (value, salt) in copy_id.guid.iter_mut().zip(page.guid) {
                *value ^= salt;
            }
            copy.reference(copy_id)?;
            metadata.extend_from_slice(&series.reference(copy_id)?);
            replacements.insert(copy_id, copy);
        }
        let links = PropertyObject {
            jcid: series.jcid,
            bytes: crate::create::properties(&[(0x2c001d63, spaces)])?,
            global_ids: std::sync::Arc::clone(&series.global_ids),
        };
        series
            .copy_property(&links, 0x2c001d63)
            .expect("copy spaces");
        series
            .set(&[(0x24003442, &metadata)])
            .expect("set metadata links");
        replacements.insert(parent, series);
        let mut object = PropertyObject::from_object(&section.objects[&root])?;
        let mut children = Vec::new();
        for id in &view.nodes[&root].children {
            if *id == parent
                || !view.nodes[id]
                    .spaces
                    .iter()
                    .any(|sid| [pages[4].0, pages[5].0].contains(sid))
            {
                children.extend_from_slice(&object.reference(*id)?);
            }
        }
        object.set(&[(0x24001c20, &children)])?;
        replacements.insert(root, object);
        changes.insert(index.root, replacements);
        Ok(changes
            .into_iter()
            .map(|(sid, objects)| (sid, RevisionEdit::Update(objects)))
            .collect())
    })
    .unwrap()
}

#[test]
fn nesting_publishes_section_order_and_page_levels_in_one_transaction() {
    let written = nest(SOURCE);
    let before = Store::parse(SOURCE).unwrap();
    let after = Store::parse(&written).unwrap();
    assert_eq!(
        after.header.transaction_count,
        before.header.transaction_count + 1
    );
    assert_eq!(after.header.generation, before.header.generation + 1);
    let old = RevisionIndex::parse(&before).unwrap();
    let new = RevisionIndex::parse(&after).unwrap();
    let document = Document::parse(&new).unwrap();
    let pages = document.pages().unwrap();
    assert_eq!(pages, Document::parse(&old).unwrap().pages().unwrap());
    let mut changed_spaces = 0;
    for (sid, space) in &old.spaces {
        changed_spaces += usize::from(space.labels != new.spaces[sid].labels);
        for rid in space.revisions.keys() {
            assert_eq!(
                format!("{:?}", old.resolve(*sid, *rid).unwrap()),
                format!("{:?}", new.resolve(*sid, *rid).unwrap())
            );
        }
    }
    assert_eq!(changed_spaces, 3);
    for (ordinal, (sid, _)) in pages.iter().enumerate() {
        let view = &document.spaces[sid];
        let view = view.active().unwrap();
        let Kind::Metadata { level, .. } = view.nodes[&view.roots[&2]].kind else {
            panic!()
        };
        assert_eq!(
            level,
            Some(match ordinal {
                4 => 2,
                5 => 3,
                _ => 1,
            })
        );
        let old_view = old
            .resolve(*sid, old.spaces[sid].labels[&(ExGuid::default(), 1)])
            .unwrap();
        let new_view = new
            .resolve(*sid, new.spaces[sid].labels[&(ExGuid::default(), 1)])
            .unwrap();
        for (id, object) in &old_view.objects {
            if *id != old_view.roots[&2] {
                assert_eq!(format!("{object:?}"), format!("{:?}", new_view.objects[id]));
            }
        }
    }
    let root = &document.spaces[&document.root];
    let root = root.active().unwrap();
    assert_eq!(
        root.nodes[&root.roots[&1]]
            .children
            .iter()
            .map(|id| root.nodes[id].spaces.len())
            .collect::<Vec<_>>(),
        [1, 1, 1, 3, 1, 1, 1]
    );
    assert_eq!(nest(&written), written);
    if let Some(output) = std::env::var_os("ONESTORE_PAGE_BATCH_OUTPUT") {
        std::fs::create_dir(&output).unwrap();
        std::fs::write(
            std::path::Path::new(&output).join("Lifecycle.one"),
            &written,
        )
        .unwrap();
    }
}

#[test]
fn interrupted_multi_space_publication_never_exposes_a_partial_nesting() {
    let written = nest(SOURCE);
    let old = current::current(SOURCE);
    let new = current::current(&written);
    assert_ne!(old, new);
    for write_limit in [17, 4096] {
        let mut complete = disk::Disk {
            visible: SOURCE.to_vec(),
            durable: SOURCE.to_vec(),
            operation: 0,
            fail_at: None,
            write_limit,
            random: 1951,
        };
        crate::commit::Transaction::between(SOURCE, &written)
            .commit(&mut complete)
            .unwrap();
        assert_eq!(complete.durable, written);
        for fail_at in 1..=complete.operation {
            let mut interrupted = disk::Disk {
                visible: SOURCE.to_vec(),
                durable: SOURCE.to_vec(),
                operation: 0,
                fail_at: Some(fail_at),
                write_limit,
                random: 1951 + fail_at as u64,
            };
            crate::commit::Transaction::between(SOURCE, &written)
                .commit(&mut interrupted)
                .unwrap_err();
            let observed = current::current(&interrupted.durable);
            assert!(
                observed == old || observed == new,
                "{write_limit}:{fail_at}"
            );
        }
    }
}

#[test]
fn empty_batches_and_invalid_spaces_do_not_produce_an_edit() {
    assert_eq!(
        write_revisions(SOURCE, |_| Ok(BTreeMap::new())).unwrap(),
        SOURCE
    );
    assert!(
        write_revisions(SOURCE, |_| Ok(BTreeMap::from([(
            ExGuid::default(),
            RevisionEdit::Update(BTreeMap::new())
        )])))
        .is_err()
    );
}

#[test]
fn repeated_multi_space_edits_cross_counter_carries_and_checkpoint_each_space() {
    let mut source = crate::create_section("batch.one", "Preserved text", "Author").unwrap();
    let initial = source.clone();
    let initial_store = Store::parse(&initial).unwrap();
    let initial_index = RevisionIndex::parse(&initial_store).unwrap();
    let mut checkpoints = BTreeMap::new();
    for step in 1_u32..=514 {
        let written = write_revisions(&source, |index| {
            let mut changes = BTreeMap::new();
            for (sid, space) in &index.spaces {
                let raw = index.resolve(*sid, space.labels[&(ExGuid::default(), 1)])?;
                let (id, property) = if *sid == index.root {
                    (raw.roots[&2], 0x14001cbe)
                } else {
                    (
                        *raw.objects
                            .iter()
                            .find(|(_, object)| object.jcid == 0x6000b)
                            .unwrap()
                            .0,
                        0x14001d7a,
                    )
                };
                let mut object = PropertyObject::from_object(&raw.objects[&id])?;
                object.set(&[(property, &step.to_le_bytes())])?;
                changes.insert(*sid, BTreeMap::from([(id, object)]));
            }
            Ok(changes
                .into_iter()
                .map(|(sid, objects)| (sid, RevisionEdit::Update(objects)))
                .collect())
        })
        .unwrap();
        let store = Store::parse(&written).unwrap();
        assert_eq!(
            store.header.transaction_count,
            initial_store.header.transaction_count + step
        );
        assert!(store.checksum_mismatches.is_empty());
        let index = RevisionIndex::parse(&store).unwrap();
        for (sid, space) in &index.spaces {
            let rid = space.labels[&(ExGuid::default(), 1)];
            if space.revisions[&rid].dependency.is_none() {
                *checkpoints.entry(*sid).or_insert(0) += 1;
            }
            for old_rid in initial_index.spaces[sid].revisions.keys() {
                assert_eq!(
                    format!("{:?}", initial_index.resolve(*sid, *old_rid).unwrap()),
                    format!("{:?}", index.resolve(*sid, *old_rid).unwrap())
                );
            }
        }
        if step == 1 || store.header.transaction_count == 256 || step == 512 {
            let old = current::current(&source);
            let new = current::current(&written);
            let mut complete = disk::Disk {
                visible: source.clone(),
                durable: source.clone(),
                operation: 0,
                fail_at: None,
                write_limit: 4096,
                random: 1952,
            };
            crate::commit::Transaction::between(&source, &written)
                .commit(&mut complete)
                .unwrap();
            for fail_at in 1..=complete.operation {
                let mut interrupted = disk::Disk {
                    visible: source.clone(),
                    durable: source.clone(),
                    operation: 0,
                    fail_at: Some(fail_at),
                    write_limit: 4096,
                    random: 1952 + fail_at as u64,
                };
                crate::commit::Transaction::between(&source, &written)
                    .commit(&mut interrupted)
                    .unwrap_err();
                let observed = current::current(&interrupted.durable);
                assert!(observed == old || observed == new, "{step}:{fail_at}");
            }
        }
        source = written;
    }
    assert_eq!(checkpoints.len(), 2);
    assert!(checkpoints.values().all(|count| *count == 1));
}
