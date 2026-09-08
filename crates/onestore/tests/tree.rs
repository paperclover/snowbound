use onestore::{
    ExGuid, PreparedEdit, RevisionIndex, Store, TreeEdit,
    document::{Document, Kind, Revision},
};
use std::collections::{BTreeMap, BTreeSet};

#[path = "support/current.rs"]
mod current;
#[path = "support/disk.rs"]
mod disk;
#[path = "support/tree_model.rs"]
mod tree_model;

const FIXTURES: [(&[u8], &[u8]); 2] = [
    (
        include_bytes!("../../../corpus/outline-edit/before/notebook/synthetic.one"),
        include_bytes!("../../../corpus/outline-edit/after/notebook/synthetic.one"),
    ),
    (
        include_bytes!("../../../corpus/outline-edit/tree/before/notebook/synthetic.one"),
        include_bytes!("../../../corpus/outline-edit/tree/after/notebook/synthetic.one"),
    ),
];

#[test]
fn twelve_client_tree_schedules_preserve_content_and_indentation() {
    let mut random = 1932_u64;
    for _ in 0..80 {
        let input: Vec<_> = (0..241)
            .map(|_| {
                random ^= random << 13;
                random ^= random >> 7;
                random ^= random << 17;
                random as u8
            })
            .collect();
        tree_model::run(&input);
    }
}

fn active(view: &Revision<'_>, page: ExGuid) -> BTreeSet<ExGuid> {
    let mut pending = vec![page];
    let mut ids = BTreeSet::new();
    while let Some(id) = pending.pop() {
        assert!(ids.insert(id), "Repeated active identity: {id}");
        let node = &view.nodes[&id];
        pending.extend(
            node.children
                .iter()
                .chain(&node.content)
                .chain(&node.structure)
                .copied(),
        );
    }
    ids
}

fn operation(view: &Revision<'_>, page: ExGuid) -> Option<(String, ExGuid, TreeEdit)> {
    let Kind::Metadata {
        title: Some(name), ..
    } = &view.nodes[&view.roots[&2]].kind
    else {
        panic!()
    };
    let ids = active(view, page);
    let paragraphs: BTreeMap<_, _> = ids
        .iter()
        .filter_map(|id| {
            let node = &view.nodes[id];
            let text = node.content.first()?;
            match &view.nodes[text].kind {
                Kind::RichText { text, .. } if matches!(node.kind, Kind::Paragraph { .. }) => {
                    Some((text.as_str(), *id))
                }
                _ => None,
            }
        })
        .collect();
    let target = *paragraphs
        .iter()
        .find(|(text, _)| text.starts_with("Target "))?
        .1;
    let outline = *view.nodes[&page]
        .children
        .iter()
        .find(|id| matches!(view.nodes[id].kind, Kind::Outline { .. }))
        .unwrap();
    let parent = *ids
        .iter()
        .find(|id| view.nodes[id].children.contains(&target))
        .unwrap();
    let edit = match name.as_str() {
        "Delete outline" => TreeEdit::delete(outline, "Tree author"),
        name if name.starts_with("Delete") => TreeEdit::delete(target, "Tree author"),
        "Move leaf down"
        | "Move subtree down"
        | "Move numbered subtree down"
        | "Move cell subtree down" => TreeEdit::move_to(target, parent, None, "Tree author"),
        "Move subtree up" => {
            TreeEdit::move_to(target, parent, Some(paragraphs["Anchor"]), "Tree author")
        }
        "Indent subtree" | "Indent bullet subtree" => {
            TreeEdit::move_to(target, paragraphs["Anchor"], None, "Tree author")
        }
        "Outdent subtree" | "Outdent first group" => TreeEdit::move_to(
            target,
            outline,
            Some(paragraphs["Trailing sibling"]),
            "Tree author",
        ),
        _ => return None,
    }
    .unwrap();
    Some((
        name.clone(),
        if name == "Delete outline" {
            outline
        } else {
            target
        },
        edit,
    ))
}

#[test]
fn native_subtree_controls_match_with_preserved_fields_and_history() {
    let mut count = 0;
    for (source, native) in FIXTURES {
        let store = Store::parse(source).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let document = Document::parse(&index).unwrap();
        let native_store = Store::parse(native).unwrap();
        let native_index = RevisionIndex::parse(&native_store).unwrap();
        let native_document = Document::parse(&native_index).unwrap();
        let mut candidate = source.to_vec();
        for (sid, page) in document.pages().unwrap() {
            let space = &document.spaces[&sid];
            let before = &space.revisions[&space.contexts[&ExGuid::default()]];
            let Some((name, object, edit)) = operation(before, page) else {
                continue;
            };
            let restored = serde_json::from_value(serde_json::to_value(&edit).unwrap()).unwrap();
            assert_eq!(edit, restored);
            let prepared = PreparedEdit::tree(source, sid, &restored).unwrap();
            let after_store = Store::parse(prepared.as_bytes()).unwrap();
            assert!(after_store.checksum_mismatches.is_empty());
            let after_index = RevisionIndex::parse(&after_store).unwrap();
            after_index.validate_current().unwrap();
            let after_document = Document::parse(&after_index).unwrap();
            let space = &after_document.spaces[&sid];
            let after = &space.revisions[&space.contexts[&ExGuid::default()]];
            let space = &native_document.spaces[&sid];
            let reference = &space.revisions[&space.contexts[&ExGuid::default()]];
            let old_ids = active(before, page);
            let actual = active(after, page);
            let native_ids = active(reference, page);
            assert_eq!(
                actual.intersection(&old_ids).collect::<Vec<_>>(),
                native_ids.intersection(&old_ids).collect::<Vec<_>>(),
                "{name}"
            );
            assert_eq!(actual.len(), native_ids.len(), "{name}");
            let mut mutable = BTreeSet::new();
            for view in [before, after] {
                let ids = active(view, page);
                if !ids.contains(&object) {
                    continue;
                }
                let mut pending = vec![object];
                while let Some(id) = pending.pop() {
                    mutable.insert(id);
                    pending.extend(
                        ids.iter()
                            .filter(|parent| {
                                view.nodes[parent]
                                    .children
                                    .iter()
                                    .chain(&view.nodes[parent].content)
                                    .chain(&view.nodes[parent].structure)
                                    .any(|child| *child == id)
                            })
                            .copied(),
                    );
                }
            }
            let mut remap = BTreeMap::new();
            if name == "Delete sole cell paragraph" {
                for kind in ["Paragraph", "RichText"] {
                    let selected = |view: &Revision<'_>, ids: &BTreeSet<ExGuid>| {
                        *ids.difference(&old_ids)
                            .find(|id| {
                                serde_json::to_value(&view.nodes[id].kind).unwrap()["type"] == kind
                            })
                            .unwrap()
                    };
                    remap.insert(selected(reference, &native_ids), selected(after, &actual));
                }
            }
            for id in &actual {
                let reference_id = remap
                    .iter()
                    .find_map(|(a, b)| (b == id).then_some(*a))
                    .unwrap_or(*id);
                let wanted = &reference.nodes[&reference_id];
                let node = &after.nodes[id];
                for (a, b) in [
                    (&node.children, &wanted.children),
                    (&node.content, &wanted.content),
                    (&node.structure, &wanted.structure),
                ] {
                    assert_eq!(
                        *a,
                        b.iter()
                            .map(|id| remap.get(id).copied().unwrap_or(*id))
                            .collect::<Vec<_>>(),
                        "{name}: {id}"
                    );
                }
                if !node.children.is_empty() {
                    assert_eq!(node.child_level, wanted.child_level, "{name}: {id}");
                }
                if old_ids.contains(id) {
                    let old = &before.nodes[id];
                    let mut expected = serde_json::to_value(old).unwrap();
                    let mut actual = serde_json::to_value(node).unwrap();
                    expected["children"] = actual["children"].clone();
                    if !node.children.is_empty() {
                        expected["child_level"] = actual["child_level"].clone();
                    }
                    if mutable.contains(id)
                        || node.child_level != old.child_level
                        || node.children != old.children
                    {
                        assert!(node.modified >= old.modified);
                        expected["modified"] = actual["modified"].clone();
                    }
                    if *id == object
                        && !name.starts_with("Delete")
                        && matches!(node.kind, Kind::Paragraph { .. })
                    {
                        expected["latest_author"] = actual["latest_author"].clone();
                        assert!(
                            matches!(&after.nodes[&node.latest_author.unwrap()].kind,Kind::Author{name:Some(name)} if name=="Tree author")
                        );
                    }
                    for value in [&mut expected, &mut actual] {
                        value["extra"][0]
                            .as_array_mut()
                            .unwrap()
                            .sort_by_key(|field| field["id"].as_u64().unwrap());
                    }
                    assert_eq!(actual, expected, "{name}: preserved {id}");
                    if matches!(old.kind, Kind::RichText { .. }) {
                        assert_eq!(
                            serde_json::to_value(node).unwrap(),
                            serde_json::to_value(old).unwrap(),
                            "{name}: text {id}"
                        );
                    }
                    assert_eq!(
                        serde_json::to_value(&node.kind).unwrap(),
                        serde_json::to_value(&old.kind).unwrap(),
                        "{name}: kind {id}"
                    );
                    assert_eq!(
                        serde_json::to_value(&node.tags).unwrap(),
                        serde_json::to_value(&old.tags).unwrap(),
                        "{name}: tags {id}"
                    );
                } else if matches!(node.kind, Kind::RichText { .. }) {
                    assert_eq!(
                        serde_json::to_value(&node.kind).unwrap(),
                        serde_json::to_value(&wanted.kind).unwrap()
                    );
                }
            }
            for (sid, space) in &index.spaces {
                for rid in space.revisions.keys() {
                    let old = index.resolve(*sid, *rid).unwrap();
                    let retained = after_index.resolve(*sid, *rid).unwrap();
                    assert_eq!(old.roots, retained.roots);
                    for (id, object) in old.objects {
                        assert_eq!(object.data, retained.objects[&id].data);
                    }
                }
            }
            let another = PreparedEdit::tree(source, sid, &edit).unwrap();
            let second_store = Store::parse(another.as_bytes()).unwrap();
            let second_index = RevisionIndex::parse(&second_store).unwrap();
            let second_doc = Document::parse(&second_index).unwrap();
            let space = &second_doc.spaces[&sid];
            assert_eq!(
                actual,
                active(&space.revisions[&space.contexts[&ExGuid::default()]], page)
            );
            candidate = PreparedEdit::tree(&candidate, sid, &edit)
                .unwrap()
                .as_bytes()
                .to_vec();
            count += 1;
        }
        if let Some(output) = std::env::var_os("ONESTORE_TREE_OUTPUT") {
            let output = std::path::PathBuf::from(output).join(if source == FIXTURES[0].0 {
                "ordinary"
            } else {
                "groups-cells"
            });
            assert!(output.is_absolute());
            std::fs::create_dir_all(&output).unwrap();
            std::fs::write(output.join("synthetic.one"), candidate).unwrap();
        }
    }
    assert_eq!(count, 18);
}

#[test]
fn moves_reject_cycles_wrong_parents_and_stale_siblings() {
    let source = onestore::create_section("tree.one", "Text", "Author").unwrap();
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
    let paragraph = view.nodes[&outline].children[0];
    let text = view.nodes[&paragraph].content[0];
    for intent in [
        TreeEdit::delete(page, "Author"),
        TreeEdit::delete(text, "Author"),
        TreeEdit::move_to(outline, paragraph, None, "Author"),
        TreeEdit::move_to(paragraph, paragraph, None, "Author"),
        TreeEdit::move_to(paragraph, text, None, "Author"),
        TreeEdit::move_to(paragraph, outline, Some(page), "Author"),
    ] {
        assert!(PreparedEdit::tree(&source, sid, &intent.unwrap()).is_err());
    }
    for before in [None, Some(paragraph)] {
        let edit = TreeEdit::move_to(paragraph, outline, before, "Author").unwrap();
        assert_eq!(
            PreparedEdit::tree(&source, sid, &edit).unwrap().as_bytes(),
            source
        );
    }
}

#[test]
fn publication_interruptions_expose_complete_old_or_new_trees() {
    let source = FIXTURES[1].0;
    let store = Store::parse(source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let before = current::current(source);
    let mut count = 0;
    for (sid, page) in document.pages().unwrap() {
        let space = &document.spaces[&sid];
        let view = &space.revisions[&space.contexts[&ExGuid::default()]];
        let Some((name, _, intent)) = operation(view, page) else {
            continue;
        };
        if !matches!(
            name.as_str(),
            "Delete sole cell paragraph"
                | "Delete unindented sibling after group"
                | "Move cell subtree down"
        ) {
            continue;
        }
        let edit = PreparedEdit::tree(source, sid, &intent).unwrap();
        let after = current::current(edit.as_bytes());
        for write_limit in [17, 4096] {
            let disk = |fail_at| disk::Disk {
                visible: source.to_vec(),
                durable: source.to_vec(),
                operation: 0,
                fail_at,
                write_limit,
                random: 1931,
            };
            let mut success = disk(None);
            edit.commit(&mut success).unwrap();
            assert_eq!(success.durable, edit.as_bytes());
            for at in 1..=success.operation {
                let mut interrupted = disk(Some(at));
                let failure = edit.commit(&mut interrupted).unwrap_err();
                let actual = current::current(&interrupted.durable);
                assert!(actual == before || actual == after, "{name}: {at}");
                match failure.state {
                    onestore::CommitState::NotCommitted => assert_eq!(actual, before),
                    onestore::CommitState::Committed => assert_eq!(actual, after),
                    onestore::CommitState::Unknown => {}
                }
            }
        }
        count += 1;
    }
    assert_eq!(count, 3);
}

#[test]
fn cross_container_moves_keep_tables_and_replace_emptied_cells() {
    let source = FIXTURES[1].0;
    let store = Store::parse(source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let mut candidate = source.to_vec();
    let mut count = 0;
    for (sid, page) in document.pages().unwrap() {
        let space = &document.spaces[&sid];
        let view = &space.revisions[&space.contexts[&ExGuid::default()]];
        let Kind::Metadata {
            title: Some(name), ..
        } = &view.nodes[&view.roots[&2]].kind
        else {
            panic!()
        };
        if !matches!(
            name.as_str(),
            "Outdent first group"
                | "Delete sole cell paragraph"
                | "Move cell subtree down"
                | "Delete cell subtree"
        ) {
            continue;
        }
        let ids = active(view, page);
        let parents: BTreeMap<_, _> = ids
            .iter()
            .flat_map(|id| view.nodes[id].children.iter().map(|child| (*child, *id)))
            .collect();
        let target=*ids.iter().find(|id| view.nodes[id].content.first().is_some_and(|text| matches!(&view.nodes[text].kind,Kind::RichText{text,..} if text.starts_with("Target ")))).unwrap();
        let outlines: Vec<_> = view.nodes[&page]
            .children
            .iter()
            .copied()
            .filter(|id| matches!(view.nodes[id].kind, Kind::Outline { .. }))
            .collect();
        let cells: Vec<_> = ids
            .iter()
            .copied()
            .filter(|id| matches!(view.nodes[id].kind, Kind::Cell { .. }))
            .collect();
        let (object, destination) = match name.as_str() {
            "Outdent first group" => (*view.nodes[&outlines[0]].children.last().unwrap(), target),
            "Delete sole cell paragraph" => (
                target,
                *cells.iter().find(|id| **id != parents[&target]).unwrap(),
            ),
            "Move cell subtree down" => (target, outlines[1]),
            _ => (view.nodes[&outlines[0]].children[0], outlines[1]),
        };
        let intent = TreeEdit::move_to(object, destination, None, "Tree author").unwrap();
        let edit = PreparedEdit::tree(source, sid, &intent).unwrap();
        let store = Store::parse(edit.as_bytes()).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let document = Document::parse(&index).unwrap();
        let space = &document.spaces[&sid];
        let after = &space.revisions[&space.contexts[&ExGuid::default()]];
        assert_eq!(after.nodes[&destination].children.last(), Some(&object));
        let actual = active(after, page);
        for id in ids.intersection(&actual) {
            let old = &view.nodes[id];
            let node = &after.nodes[id];
            assert_eq!(node.content, old.content, "{name}: {id}");
            assert_eq!(
                serde_json::to_value(&node.kind).unwrap(),
                serde_json::to_value(&old.kind).unwrap()
            );
            if matches!(
                node.kind,
                Kind::RichText { .. } | Kind::Table { .. } | Kind::Row | Kind::Cell { .. }
            ) {
                assert_eq!(
                    serde_json::to_value(&node.format).unwrap(),
                    serde_json::to_value(&old.format).unwrap()
                );
                assert_eq!(
                    serde_json::to_value(&node.layout).unwrap(),
                    serde_json::to_value(&old.layout).unwrap()
                );
            }
        }
        match name.as_str() {
            "Outdent first group" => {
                assert_eq!(after.nodes[&outlines[0]].children, [target]);
                assert_eq!(after.nodes[&outlines[0]].child_level, Some(2));
                assert!(!actual.contains(&parents[&target]));
            }
            "Delete sole cell paragraph" => {
                let old_cell = parents[&target];
                let replacement = after.nodes[&old_cell].children[0];
                assert!(!ids.contains(&replacement));
                assert_eq!(after.nodes[&old_cell].children.len(), 1);
                let text = after.nodes[&replacement].content[0];
                assert!(
                    matches!(&after.nodes[&text].kind,Kind::RichText{text,..} if text.is_empty())
                );
            }
            "Delete cell subtree" => assert!(!actual.contains(&outlines[0])),
            _ => assert_eq!(after.nodes[&parents[&target]].children.len(), 1),
        }
        candidate = PreparedEdit::tree(&candidate, sid, &intent)
            .unwrap()
            .as_bytes()
            .to_vec();
        count += 1;
    }
    assert_eq!(count, 4);
    if let Some(output) = std::env::var_os("ONESTORE_TREE_OUTPUT") {
        let output = std::path::PathBuf::from(output).join("cross-container");
        assert!(output.is_absolute());
        std::fs::create_dir_all(output.parent().unwrap()).unwrap();
        std::fs::create_dir(&output).unwrap();
        std::fs::write(output.join("synthetic.one"), candidate).unwrap();
    }
}
