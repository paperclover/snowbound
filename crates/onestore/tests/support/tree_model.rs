use onestore::{
    CommitState, ExGuid, RevisionIndex, Store,
    document::{Document, Kind, Revision},
    op::{Op, PageOp},
    page::Page,
};
use std::{
    collections::BTreeMap,
    sync::LazyLock,
};

use crate::{current, disk, ops};

static SOURCE: LazyLock<Vec<u8>> = LazyLock::new(|| {
    let source = onestore::create_section("tree.one", "Original", "Author").unwrap();
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
    let (add, other, _) = ops::new_outline(360.0, 72.0, "Other");
    let mut edits = vec![add];
    for container in [outline, other] {
        let mut paragraphs = Vec::new();
        for text in ["First 🦀 é", "Second 東京", "Third"] {
            let paragraph = ops::paragraph(text);
            let mut child = ops::paragraph("Child");
            child.parent = Some(paragraph.id);
            child.level = 2;
            paragraphs.extend([paragraph, child]);
        }
        edits.push(PageOp::Insert {
            container,
            before: None,
            paragraphs,
        });
    }
    ops::page_edited(&source, sid, edits).unwrap()
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

/// Random subtree moves and deletions by twelve clients, each on its own possibly stale
/// image, committed under injected storage faults: an edit stores the page the model
/// predicts, and the file always holds a complete old or new graph.
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
        let outline = matches!(view.nodes[&target].kind, Kind::Outline { .. });
        let deleting = step[1] & 6 == 0;
        let (op, cycle) = if deleting {
            (PageOp::Delete { object: target }, false)
        } else {
            let mut destinations: Vec<_> = before
                .keys()
                .copied()
                .filter(|id| {
                    if outline {
                        *id == page
                    } else {
                        matches!(
                            view.nodes[id].kind,
                            Kind::Outline { .. } | Kind::Paragraph { .. } | Kind::Cell { .. }
                        )
                    }
                })
                .collect();
            destinations.sort_by_key(|id| before[id].2);
            let destination = destinations[usize::from(step[4]) % destinations.len()];
            // Outline groups are the writers' business: anchors are the paragraphs in them.
            let mut children = Vec::new();
            let mut pending: Vec<ExGuid> = view.nodes[&destination].children.iter().rev().copied().collect();
            while let Some(id) = pending.pop() {
                match view.nodes[&id].kind {
                    Kind::OutlineGroup => pending.extend(view.nodes[&id].children.iter().rev()),
                    _ if id == target => {}
                    _ => children.push(id),
                }
            }
            let before = children
                .get(usize::from(step[5]) % (children.len() + 1))
                .copied();
            let op = PageOp::Move {
                object: target,
                parent: (!outline).then_some(destination),
                before,
            };
            (op, subtree.contains_key(&destination))
        };
        let model = Page::from_space(&document, sid).unwrap();
        let ops = vec![Op::Page { space: sid, op: op.clone() }];
        let Ok(edit) = ops::apply(source, "Tree schedule", ops) else {
            continue;
        };
        assert!(!cycle, "{op:?}");
        let after_store = Store::parse(edit.as_bytes()).unwrap();
        let after_index = RevisionIndex::parse(&after_store).unwrap();
        after_index.validate_current().unwrap();
        let after_document = Document::parse(&after_index).unwrap();
        let mut predicted = model.clone();
        onestore::op::predict(&mut predicted, &op).unwrap();
        let stored = Page::from_space(&after_document, sid).unwrap();
        if stored != predicted {
            let (a, b) = (format!("{stored:?}"), format!("{predicted:?}"));
            let at = a.bytes().zip(b.bytes()).take_while(|(x, y)| x == y).count();
            panic!("{op:?}\n stored    …{}\n predicted …{}", &a[a.floor_char_boundary(at.saturating_sub(300))..a.floor_char_boundary((at + 200).min(a.len()))], &b[b.floor_char_boundary(at.saturating_sub(300))..b.floor_char_boundary((at + 200).min(b.len()))]);
        }
        let space = &after_document.spaces[&sid];
        let after = &space.revisions[&space.contexts[&ExGuid::default()]];
        for (id, _) in forest(after, page) {
            let node = &after.nodes[&id];
            if before.contains_key(&id) {
                assert_eq!(node.content, view.nodes[&id].content);
                assert_eq!(
                    serde_json::to_value(&node.tags).unwrap(),
                    serde_json::to_value(&view.nodes[&id].tags).unwrap()
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
        let Some(transaction) = &edit.transaction else {
            continue;
        };
        let result = transaction.commit(&mut disk);
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
