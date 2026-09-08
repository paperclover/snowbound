use onestore::{
    CommitState, ExGuid, Insertion, PreparedEdit, RevisionIndex, Store, TreeEdit,
    document::{Document, Kind, Revision},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::LazyLock,
};

use crate::{current, disk};

static SOURCE: LazyLock<Vec<u8>> = LazyLock::new(|| {
    let mut source = onestore::create_section("tree.one", "Original", "Author").unwrap();
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
    let other = Insertion::outline(page, 360.0, 72.0, "Other", "Author").unwrap();
    source = PreparedEdit::insert(&source, sid, &other)
        .unwrap()
        .as_bytes()
        .to_vec();
    for parent in [outline, other.object()] {
        for text in ["First 🦀 é", "Second 東京", "Third"] {
            let insertion = Insertion::paragraph(parent, None, text, "Author").unwrap();
            source = PreparedEdit::insert(&source, sid, &insertion)
                .unwrap()
                .as_bytes()
                .to_vec();
            let child = Insertion::paragraph(insertion.object(), None, "Child", "Author").unwrap();
            source = PreparedEdit::insert(&source, sid, &child)
                .unwrap()
                .as_bytes()
                .to_vec();
        }
    }
    source
});

fn forest(view: &Revision<'_>, root: ExGuid) -> BTreeMap<ExGuid, (ExGuid, u32, usize)> {
    let mut positions = BTreeMap::new();
    let mut pending = vec![(root, root, 0)];
    while let Some((id, mut scope, mut level)) = pending.pop() {
        let node = &view.nodes[&id];
        if matches!(node.kind, Kind::Outline { .. } | Kind::Cell { .. }) {
            scope = id;
            level = 0;
        }
        assert!(
            positions
                .insert(id, (scope, level, positions.len()))
                .is_none()
        );
        pending.extend(
            node.children
                .iter()
                .rev()
                .map(|id| (*id, scope, level + u32::from(node.child_level.unwrap_or(1)))),
        );
        pending.extend(node.content.iter().rev().map(|id| (*id, scope, level)));
    }
    positions
}

pub fn run(input: &[u8]) {
    let Some((&selector, input)) = input.split_first() else {
        return;
    };
    let source = if selector & 1 == 0 {
        SOURCE.as_slice()
    } else {
        include_bytes!("../../../../corpus/outline-edit/tree/before/notebook/synthetic.one")
    };
    let mut persisted = source.to_vec();
    let mut clients = vec![persisted.clone(); 12];
    for step in input.chunks_exact(10).take(36) {
        let client = usize::from(step[0]) % clients.len();
        if step[1] & 1 != 0 {
            clients[client] = persisted.clone();
        }
        let source = &clients[client];
        let store = Store::parse(source).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let document = Document::parse(&index).unwrap();
        let pages = document.pages().unwrap();
        let (sid, page) = pages[usize::from(step[2]) % pages.len()];
        let space = &document.spaces[&sid];
        let view = &space.revisions[&space.contexts[&ExGuid::default()]];
        let before = forest(view, page);
        let mut targets: Vec<_> = before
            .keys()
            .copied()
            .filter(|id| {
                matches!(
                    view.nodes[id].kind,
                    Kind::Paragraph { .. } | Kind::Outline { .. }
                )
            })
            .collect();
        targets.sort_by_key(|id| before[id].2);
        if targets.is_empty() {
            continue;
        }
        let target = targets[usize::from(step[3]) % targets.len()];
        let subtree = forest(view, target);
        let parents: BTreeMap<_, _> = before
            .keys()
            .flat_map(|id| view.nodes[id].children.iter().map(|child| (*child, *id)))
            .collect();
        let parent = parents[&target];
        let deleting = step[1] & 6 == 0;
        let mut expected: BTreeMap<_, _> = before
            .iter()
            .filter_map(|(id, pos)| {
                matches!(view.nodes[id].kind, Kind::Paragraph { .. })
                    .then_some((*id, (pos.0, pos.1)))
            })
            .collect();
        let mut ordered: Vec<_> = expected.keys().copied().collect();
        ordered.sort_by_key(|id| before[id].2);
        let mut expected_order = BTreeMap::<_, Vec<_>>::new();
        for id in ordered {
            expected_order.entry(before[&id].0).or_default().push(id);
        }
        let intent;
        let mut reversal = None;
        let edit = if deleting {
            expected.retain(|id, _| !subtree.contains_key(id));
            for ids in expected_order.values_mut() {
                ids.retain(|id| !subtree.contains_key(id));
            }
            intent = TreeEdit::delete(target, "Tree schedule").unwrap();
            PreparedEdit::tree(source, sid, &intent)
        } else {
            let mut destinations: Vec<_> = before
                .keys()
                .copied()
                .filter(|id| {
                    if matches!(view.nodes[&target].kind, Kind::Outline { .. }) {
                        *id == page
                    } else {
                        matches!(
                            view.nodes[id].kind,
                            Kind::Outline { .. }
                                | Kind::OutlineGroup
                                | Kind::Paragraph { .. }
                                | Kind::Cell { .. }
                        )
                    }
                })
                .collect();
            destinations.sort_by_key(|id| before[id].2);
            if destinations.is_empty() {
                continue;
            }
            let destination = destinations[usize::from(step[4]) % destinations.len()];
            let children = &view.nodes[&destination].children;
            let anchor = children
                .get(usize::from(step[5]) % (children.len() + 1))
                .copied();
            intent = TreeEdit::move_to(target, destination, anchor, "Tree schedule").unwrap();
            let edit = PreparedEdit::tree(source, sid, &intent);
            if subtree.contains_key(&destination) {
                assert!(edit.is_err());
                continue;
            }
            if destination == parent && step[9] & 3 == 0 {
                let original = &view.nodes[&parent].children;
                let next = original.iter().position(|id| *id == target).unwrap() + 1;
                reversal = Some(
                    TreeEdit::move_to(target, parent, original.get(next).copied(), "Tree schedule")
                        .unwrap(),
                );
            }
            if matches!(view.nodes[&target].kind, Kind::Paragraph { .. }) {
                let (scope, base, _) = before[&destination];
                let level = base + u32::from(view.nodes[&destination].child_level.unwrap_or(1));
                let (old_scope, old_level, _) = before[&target];
                if anchor != Some(target) {
                    let moved: Vec<_> = expected_order[&old_scope]
                        .iter()
                        .copied()
                        .filter(|id| subtree.contains_key(id))
                        .collect();
                    expected_order
                        .get_mut(&old_scope)
                        .unwrap()
                        .retain(|id| !subtree.contains_key(id));
                    let boundary = anchor.map(|id| before[&id].2).unwrap_or_else(|| {
                        forest(view, destination)
                            .keys()
                            .map(|id| before[id].2)
                            .max()
                            .unwrap()
                            + 1
                    });
                    let sequence = expected_order.entry(scope).or_default();
                    let position = sequence
                        .iter()
                        .position(|id| before[id].2 >= boundary)
                        .unwrap_or(sequence.len());
                    sequence.splice(position..position, moved);
                }
                for (id, pos) in &mut expected {
                    if subtree.contains_key(id) && pos.0 == old_scope {
                        *pos = (scope, level + pos.1 - old_level);
                    }
                }
            }
            edit
        };
        let restored = serde_json::from_value(serde_json::to_value(&intent).unwrap()).unwrap();
        assert_eq!(intent, restored);
        let edit = match edit {
            Ok(edit) => edit,
            Err(error)
                if error.message
                    == "Removing this group would exceed 31 child indentation levels" =>
            {
                continue;
            }
            Err(error) => panic!("{intent:?}: {error:?}"),
        };
        let after_store = Store::parse(edit.as_bytes()).unwrap();
        let after_index = RevisionIndex::parse(&after_store).unwrap();
        after_index.validate_current().unwrap();
        let after_document = Document::parse(&after_index).unwrap();
        let space = &after_document.spaces[&sid];
        let after = &space.revisions[&space.contexts[&ExGuid::default()]];
        let actual = forest(after, page);
        let mut added = BTreeSet::new();
        for (id, pos) in &actual {
            let node = &after.nodes[id];
            if let Kind::Paragraph { .. } = node.kind {
                if let Some(wanted) = expected.remove(id) {
                    assert_eq!((pos.0, pos.1), wanted, "{intent:?}: {id}");
                } else {
                    assert!(!before.contains_key(id));
                    assert!(matches!(view.nodes[&parent].kind, Kind::Cell { .. }));
                    assert_eq!(view.nodes[&parent].children, [target]);
                    assert_eq!(node.children.len(), 0);
                    assert_eq!(node.content.len(), 1);
                    assert!(node.tags.is_empty());
                    assert!(
                        matches!(&after.nodes[&node.content[0]].kind,Kind::RichText{text,..} if text.is_empty())
                    );
                    added.insert(*id);
                }
            }
            if before.contains_key(id) {
                assert_eq!(node.content, view.nodes[id].content);
                let mut expected_kind = serde_json::to_value(&view.nodes[id].kind).unwrap();
                if *id == page && expected_kind["alternate_title"].is_string() {
                    let mut roots = after.nodes[&page].children.clone();
                    roots.sort_by(|a, b| {
                        let a = &after.nodes[a].layout;
                        let b = &after.nodes[b].layout;
                        a.y.unwrap_or(0.0)
                            .total_cmp(&b.y.unwrap_or(0.0))
                            .then_with(|| a.x.unwrap_or(0.0).total_cmp(&b.x.unwrap_or(0.0)))
                    });
                    let mut title = String::new();
                    for root in roots {
                        let mut nodes: Vec<_> = forest(after, root).into_iter().collect();
                        nodes.sort_by_key(|(_, pos)| pos.2);
                        for (id, _) in nodes {
                            if let Kind::RichText { text, .. } = &after.nodes[&id].kind {
                                title = text
                                    .trim_start()
                                    .split('\r')
                                    .next()
                                    .unwrap()
                                    .trim_end()
                                    .to_owned();
                                if !title.is_empty() {
                                    break;
                                }
                            }
                        }
                        if !title.is_empty() {
                            break;
                        }
                    }
                    expected_kind["alternate_title"] = title.into();
                    let Kind::Metadata { title, .. } = &after.nodes[&after.roots[&2]].kind else {
                        panic!()
                    };
                    assert_eq!(title.as_deref(), expected_kind["alternate_title"].as_str());
                }
                assert_eq!(serde_json::to_value(&node.kind).unwrap(), expected_kind);
                assert_eq!(
                    serde_json::to_value(&node.tags).unwrap(),
                    serde_json::to_value(&view.nodes[id].tags).unwrap()
                );
            }
            if matches!(
                node.kind,
                Kind::Outline { .. } | Kind::OutlineGroup | Kind::Cell { .. }
            ) {
                assert!(
                    node.children
                        .iter()
                        .any(|id| matches!(after.nodes[id].kind, Kind::Paragraph { .. }))
                );
            }
        }
        assert!(expected.is_empty());
        assert!(added.len() <= 1);
        let mut ordered: Vec<_> = actual
            .iter()
            .filter(|(id, _)| {
                matches!(after.nodes[id].kind, Kind::Paragraph { .. }) && !added.contains(id)
            })
            .collect();
        ordered.sort_by_key(|(_, pos)| pos.2);
        let mut actual_order = BTreeMap::<_, Vec<_>>::new();
        for (id, pos) in ordered {
            actual_order.entry(pos.0).or_default().push(*id);
        }
        expected_order.retain(|_, ids| !ids.is_empty());
        assert_eq!(actual_order, expected_order, "{intent:?}");
        if let Some(reversal) = reversal {
            let reversed = PreparedEdit::tree(edit.as_bytes(), sid, &reversal).unwrap();
            let replayed = PreparedEdit::tree(reversed.as_bytes(), sid, &intent).unwrap();
            let store = Store::parse(replayed.as_bytes()).unwrap();
            let index = RevisionIndex::parse(&store).unwrap();
            let document = Document::parse(&index).unwrap();
            let space = &document.spaces[&sid];
            let replayed = &space.revisions[&space.contexts[&ExGuid::default()]];
            assert_eq!(forest(replayed, page), actual);
        }
        let before = current::current(&persisted);
        let after = current::current(edit.as_bytes());
        let mut disk = disk::Disk {
            visible: persisted.clone(),
            durable: persisted.clone(),
            operation: 0,
            fail_at: (step[6] != 0).then_some(usize::from(u16::from_le_bytes([step[6], step[7]]))),
            write_limit: if step[8] & 1 == 0 { 17 } else { 4096 },
            random: u64::from(step[9]) + 1,
        };
        let result = edit.commit(&mut disk);
        let actual = current::current(&disk.durable);
        match result {
            Ok(()) => assert_eq!(actual, after),
            Err(error) => {
                assert!(actual == before || actual == after);
                match error.state {
                    CommitState::NotCommitted => assert_eq!(actual, before),
                    CommitState::Committed => assert_eq!(actual, after),
                    CommitState::Unknown => {}
                }
            }
        }
        persisted = disk.durable;
    }
}
