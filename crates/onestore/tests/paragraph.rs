use onestore::{
    ExGuid, RevisionIndex, Store,
    document::{Document, Kind, Revision},
    op::{Op, OpError, PageOp},
    page::text::new_id,
};
use serde_json::Value;

#[path = "support/checkpoint.rs"]
mod checkpoint;
#[path = "support/current.rs"]
mod current;
#[path = "support/disk.rs"]
mod disk;
#[path = "support/ops.rs"]
mod ops;

/// Enter at `at` in `text`, with fresh identities for the new paragraph, its text and a
/// copy of each list node of the paragraph: the op, the new paragraph and its text.
fn split(source: &[u8], space: ExGuid, text: ExGuid, at: u32) -> (Op, ExGuid, ExGuid) {
    let store = Store::parse(source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let lists = document.spaces[&space]
        .active()
        .unwrap()
        .nodes
        .values()
        .find_map(|node| match &node.kind {
            Kind::Paragraph { lists, .. } if node.content == [text] => Some(lists.len()),
            _ => None,
        })
        .unwrap_or(0);
    let (paragraph, right) = (new_id().unwrap(), new_id().unwrap());
    let op = PageOp::Split {
        text,
        at,
        paragraph,
        right,
        lists: (0..lists).map(|_| new_id().unwrap()).collect(),
    };
    (Op::Page { space, op }, paragraph, right)
}

/// `source` after joining the paragraph of text `right` to the one of text `left`.
fn join(source: &[u8], space: ExGuid, left: ExGuid, right: ExGuid) -> Result<Vec<u8>, OpError> {
    ops::edited(source, vec![Op::Page { space, op: PageOp::Join { left, right } }])
}

const SOURCE: &[u8] =
    include_bytes!("../../../corpus/paragraph-edit/before/notebook/synthetic.one");

fn characters(view: &Revision<'_>, id: ExGuid) -> Vec<(char, Value)> {
    view.text_runs(id)
        .unwrap()
        .into_iter()
        .flat_map(|run| {
            let format = serde_json::to_value(run.format).unwrap();
            run.text.chars().map(move |c| (c, format.clone()))
        })
        .collect()
}

const JOIN_FIXTURES: [(&[u8], &[u8], bool); 3] = [
    (
        include_bytes!("../../../corpus/paragraph-edit/split/notebook/synthetic.one").as_slice(),
        include_bytes!("../../../corpus/paragraph-edit/joined/notebook/synthetic.one").as_slice(),
        true,
    ),
    (
        include_bytes!("../../../corpus/paragraph-edit/join-edges/before/notebook/synthetic.one")
            .as_slice(),
        include_bytes!("../../../corpus/paragraph-edit/join-edges/joined/notebook/synthetic.one")
            .as_slice(),
        false,
    ),
    (
        include_bytes!("../../../corpus/paragraph-edit/join-tags/before/notebook/synthetic.one")
            .as_slice(),
        include_bytes!("../../../corpus/paragraph-edit/join-tags/joined/notebook/synthetic.one")
            .as_slice(),
        false,
    ),
];

fn join_targets(document: &Document<'_>, split_cases: bool) -> Vec<(String, ExGuid, ExGuid)> {
    let mut cases = Vec::new();
    if split_cases {
        let manifest: Value =
            serde_json::from_str(include_str!("../../../corpus/paragraph-edit/manifest.json"))
                .unwrap();
        for case in manifest["cases"].as_array().unwrap() {
            cases.push((
                case["case"].as_str().unwrap().to_owned(),
                serde_json::from_value::<ExGuid>(case["original_text"].clone()).unwrap(),
                serde_json::from_value::<ExGuid>(case["new_text"].clone()).unwrap(),
            ));
        }
    } else {
        for (sid, page) in document.pages().unwrap() {
            let s = &document.spaces[&sid];
            let view = &s.revisions[&s.contexts[&ExGuid::default()]];
            let Kind::Metadata {
                title: Some(name), ..
            } = &view.nodes[&view.roots[&2]].kind
            else {
                panic!()
            };
            if !name.starts_with("Join ") {
                continue;
            }
            let outline = view.nodes[&page]
                .children
                .iter()
                .find(|id| matches!(view.nodes[id].kind, Kind::Outline { .. }))
                .unwrap();
            let mut left = view.nodes[outline].children[0];
            while let Some(child) = view.nodes[&left].children.last() {
                left = *child;
            }
            let right = view.nodes[outline].children[1];
            cases.push((
                name.clone(),
                view.nodes[&left].content[0],
                view.nodes[&right].content[0],
            ));
        }
    }
    assert_eq!(cases.len(), document.pages().unwrap().len() - 1);
    cases
}

#[test]
fn joins_match_native_graphs_tags_and_inherited_character_styles() {
    for (source, native, split_cases) in JOIN_FIXTURES {
        let store = Store::parse(source).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let document = Document::parse(&index).unwrap();
        let native_store = Store::parse(native).unwrap();
        let native_index = RevisionIndex::parse(&native_store).unwrap();
        let native_document = Document::parse(&native_index).unwrap();
        let cases = join_targets(&document, split_cases);
        for (name, left, right) in cases {
            let (sid, space) = document
                .spaces
                .iter()
                .find(|(_, space)| {
                    space.revisions[&space.contexts[&ExGuid::default()]]
                        .nodes
                        .contains_key(&left)
                })
                .unwrap();
            let edited =
                join(source, *sid, left, right).unwrap_or_else(|error| panic!("{name}: {error}"));
            let current_store = Store::parse(&edited).unwrap();
            assert_eq!(
                current_store.header.transaction_count,
                store.header.transaction_count + 1
            );
            let current_index = RevisionIndex::parse(&current_store).unwrap();
            current_index.validate_current().unwrap();
            let current_document = Document::parse(&current_index).unwrap();
            let current = &current_document.spaces[sid];
            let current = &current.revisions[&current.contexts[&ExGuid::default()]];
            let expected = &native_document.spaces[sid];
            let expected = &expected.revisions[&expected.contexts[&ExGuid::default()]];
            let (_, page) = document
                .pages()
                .unwrap()
                .into_iter()
                .find(|(id, _)| id == sid)
                .unwrap();
            let before = &space.revisions[&space.contexts[&ExGuid::default()]];
            let left_paragraph = *before
                .nodes
                .iter()
                .find(|(_, node)| node.content == [left])
                .unwrap()
                .0;
            let right_paragraph = *before
                .nodes
                .iter()
                .find(|(_, node)| node.content == [right])
                .unwrap()
                .0;
            let right_parent = *before
                .nodes
                .iter()
                .find(|(_, node)| node.children.contains(&right_paragraph))
                .unwrap()
                .0;
            let survivor = if characters(before, left).is_empty() {
                right
            } else {
                left
            };
            let mut changed = std::collections::BTreeSet::from([survivor, before.roots[&2]]);
            let mut ancestors = vec![left_paragraph, right_parent];
            while let Some(id) = ancestors.pop() {
                if !changed.insert(id) || id == page {
                    continue;
                }
                ancestors.extend(before.nodes.iter().filter_map(|(parent, node)| {
                    node.children
                        .iter()
                        .chain(&node.content)
                        .chain(&node.structure)
                        .any(|child| *child == id)
                        .then_some(*parent)
                }));
            }
            let old_raw = index
                .resolve(*sid, space.contexts[&ExGuid::default()])
                .unwrap();
            let current_rid = current_document.spaces[sid].contexts[&ExGuid::default()];
            let new_raw = current_index.resolve(*sid, current_rid).unwrap();
            for (id, object) in old_raw.objects {
                if !changed.contains(&id) {
                    assert_eq!(
                        object.data, new_raw.objects[&id].data,
                        "{name} untouched {id}"
                    );
                }
            }
            let mut pending: Vec<_> = expected.nodes[&page]
                .children
                .iter()
                .filter(|id| matches!(expected.nodes[id].kind, Kind::Outline { .. }))
                .copied()
                .collect();
            while let Some(id) = pending.pop() {
                let node = &expected.nodes[&id];
                assert_eq!(
                    current.nodes[&id].children, node.children,
                    "{name} children {id}"
                );
                assert_eq!(
                    current.nodes[&id].content, node.content,
                    "{name} content {id}"
                );
                assert_eq!(
                    current.nodes[&id].child_level, node.child_level,
                    "{name} indentation {id}"
                );
                pending.extend(
                    node.children
                        .iter()
                        .chain(&node.content)
                        .chain(&node.structure)
                        .copied(),
                );
                if matches!(node.kind, Kind::RichText { .. }) {
                    assert_eq!(
                        serde_json::to_value(&current.nodes[&id].tags).unwrap(),
                        serde_json::to_value(&node.tags).unwrap(),
                        "{name} tags {id}"
                    );
                    let normalize = |view, id| {
                        characters(view, id)
                            .into_iter()
                            .map(|(c, mut style)| {
                                let fields = style.as_object_mut().unwrap();
                                for key in
                                    ["alignment", "space_before", "space_after", "line_spacing"]
                                {
                                    fields.remove(key);
                                }
                                for key in [
                                    "bold",
                                    "italic",
                                    "underline",
                                    "strike",
                                    "superscript",
                                    "subscript",
                                    "hidden",
                                    "hyperlink",
                                    "hyperlink_label",
                                    "math",
                                    "embedded_object",
                                    "rtl",
                                ] {
                                    if fields[key].is_null() {
                                        fields.insert(key.into(), false.into());
                                    }
                                }
                                for key in ["color", "highlight"] {
                                    if fields[key].is_null() {
                                        fields.insert(key.into(), 0xff000000_u32.into());
                                    }
                                }
                                (c, style)
                            })
                            .collect::<Vec<_>>()
                    };
                    assert_eq!(
                        normalize(current, id),
                        normalize(expected, id),
                        "{name} styles {id}"
                    );
                }
            }
            for (sid, old) in &index.spaces {
                for revision in old.revisions.keys() {
                    let old = index.resolve(*sid, *revision).unwrap();
                    let retained = current_index.resolve(*sid, *revision).unwrap();
                    assert_eq!(old.roots, retained.roots);
                    for (id, object) in old.objects {
                        assert_eq!(object.data, retained.objects[&id].data);
                    }
                }
            }
            assert!(join(&edited, *sid, left, right).is_err());
            assert_eq!(
                space.contexts.len(),
                current_document.spaces[sid].contexts.len()
            );
        }
    }
}

#[test]
fn splits_partition_native_paragraphs_at_every_scalar_boundary() {
    let manifest: Value =
        serde_json::from_str(include_str!("../../../corpus/paragraph-edit/manifest.json")).unwrap();
    let store = Store::parse(SOURCE).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let native_store = Store::parse(JOIN_FIXTURES[0].0).unwrap();
    let native_index = RevisionIndex::parse(&native_store).unwrap();
    let native_document = Document::parse(&native_index).unwrap();
    for case in manifest["cases"].as_array().unwrap() {
        let text: ExGuid = serde_json::from_value(case["original_text"].clone()).unwrap();
        let paragraph: ExGuid = serde_json::from_value(case["original_paragraph"].clone()).unwrap();
        let (sid, space) = document
            .spaces
            .iter()
            .find(|(_, space)| {
                space.revisions[&space.contexts[&ExGuid::default()]]
                    .nodes
                    .contains_key(&text)
            })
            .unwrap();
        let before = &space.revisions[&space.contexts[&ExGuid::default()]];
        let (parent, parent_node) = before
            .nodes
            .iter()
            .find(|(_, node)| node.children.contains(&paragraph))
            .unwrap();
        let expected = characters(before, text);
        let offsets: Vec<u32> = std::iter::once(0)
            .chain(expected.iter().scan(0, |offset, (c, _)| {
                *offset += u32::try_from(c.len_utf16()).unwrap();
                Some(*offset)
            }))
            .collect();
        for (position, offset) in offsets.iter().enumerate() {
            let (op, new_paragraph, new_text) = split(SOURCE, *sid, text, *offset);
            let edited = ops::edited(SOURCE, vec![op.clone()]);
            // OneNote 2010 splits before or after a hyperlink and ignores Enter inside one.
            let link = |at: usize| {
                expected
                    .get(at)
                    .is_some_and(|(_, style)| style["hyperlink"] == true)
            };
            if position > 0
                && link(position - 1)
                && link(position)
                && expected[position].0 != '\u{fddf}'
            {
                assert!(edited.is_err(), "{} at {offset}", case["case"]);
                continue;
            }
            let edited =
                edited.unwrap_or_else(|error| panic!("{} at {offset}: {error}", case["case"]));
            let current_store = Store::parse(&edited).unwrap();
            assert_eq!(
                current_store.header.transaction_count,
                store.header.transaction_count + 1
            );
            let current_index = RevisionIndex::parse(&current_store).unwrap();
            current_index.validate_current().unwrap();
            let current_document = Document::parse(&current_index).unwrap();
            let current_space = &current_document.spaces[sid];
            let after = &current_space.revisions[&current_space.contexts[&ExGuid::default()]];
            let mut changed = std::collections::BTreeSet::from([text, before.roots[&2]]);
            let mut pending = vec![paragraph];
            while let Some(id) = pending.pop() {
                if !changed.insert(id) {
                    continue;
                }
                pending.extend(before.nodes.iter().filter_map(|(parent, node)| {
                    node.children
                        .iter()
                        .chain(&node.content)
                        .chain(&node.structure)
                        .any(|child| *child == id)
                        .then_some(*parent)
                }));
            }
            let old_raw = index
                .resolve(*sid, space.contexts[&ExGuid::default()])
                .unwrap();
            let new_raw = current_index
                .resolve(*sid, current_space.contexts[&ExGuid::default()])
                .unwrap();
            for (id, object) in old_raw.objects {
                if !changed.contains(&id) {
                    assert_eq!(object.data, new_raw.objects[&id].data, "untouched {id}");
                }
            }
            assert_eq!(
                characters(after, text),
                expected[..position],
                "{} prefix at {offset}",
                case["case"]
            );
            assert_eq!(
                characters(after, new_text),
                expected[position..],
                "{} suffix at {offset}",
                case["case"]
            );
            if case["offset_utf16"] == *offset {
                let native = &native_document.spaces[sid];
                let native = &native.revisions[&native.contexts[&ExGuid::default()]];
                let native_text = serde_json::from_value(case["new_text"].clone()).unwrap();
                assert_eq!(
                    characters(after, text),
                    characters(native, text),
                    "{}",
                    case["case"]
                );
                assert_eq!(
                    characters(after, new_text),
                    characters(native, native_text),
                    "{}",
                    case["case"]
                );
            }
            let mut children = parent_node.children.clone();
            children.insert(
                children.iter().position(|id| *id == paragraph).unwrap() + 1,
                new_paragraph,
            );
            assert_eq!(after.nodes[parent].children, children);
            assert_eq!(after.nodes[&paragraph].content, [text]);
            assert!(after.nodes[&paragraph].children.is_empty());
            assert_eq!(
                after.nodes[&new_paragraph].content,
                [new_text]
            );
            assert_eq!(
                after.nodes[&new_paragraph].children,
                before.nodes[&paragraph].children
            );
            assert_eq!(
                serde_json::to_value(&after.nodes[&text].tags).unwrap(),
                serde_json::to_value(&before.nodes[&text].tags).unwrap()
            );
            assert!(after.nodes[&new_text].tags.is_empty());
            let Kind::Paragraph {
                lists: old_lists, ..
            } = &before.nodes[&paragraph].kind
            else {
                panic!()
            };
            let Kind::Paragraph { lists, .. } = &after.nodes[&new_paragraph].kind else {
                panic!()
            };
            assert_eq!(old_lists.len(), lists.len());
            for (old, new) in old_lists.iter().zip(lists) {
                assert_ne!(old, new);
                let mut old = serde_json::to_value(&before.nodes[old]).unwrap();
                old["kind"]["restart"] = Value::Null;
                assert_eq!(serde_json::to_value(&after.nodes[new]).unwrap(), old);
            }
            for (space_id, old_space) in &index.spaces {
                for revision in old_space.revisions.keys() {
                    let old = index.resolve(*space_id, *revision).unwrap();
                    let retained = current_index.resolve(*space_id, *revision).unwrap();
                    assert_eq!(old.roots, retained.roots);
                    for (id, object) in old.objects {
                        assert_eq!(object.data, retained.objects[&id].data);
                    }
                }
            }
            assert!(ops::edited(&edited, vec![op]).is_err());
        }
        for offset in 0..=*offsets.last().unwrap() + 1 {
            if offsets.contains(&offset) {
                continue;
            }
            assert!(ops::edited(SOURCE, vec![split(SOURCE, *sid, text, offset).0]).is_err());
        }
    }
}

#[test]
fn invalid_split_identities_and_title_targets_are_rejected() {
    let manifest: Value =
        serde_json::from_str(include_str!("../../../corpus/paragraph-edit/manifest.json")).unwrap();
    let text: ExGuid =
        serde_json::from_value(manifest["cases"][0]["original_text"].clone()).unwrap();
    let store = Store::parse(SOURCE).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let sid = *document
        .spaces
        .iter()
        .find(|(_, space)| {
            space.revisions[&space.contexts[&ExGuid::default()]]
                .nodes
                .contains_key(&text)
        })
        .unwrap()
        .0;
    let (op, ..) = split(SOURCE, sid, text, 1);
    assert!(ops::transaction(SOURCE, "a\0b", vec![op.clone()]).is_err());
    let Op::Page { op: PageOp::Split { paragraph, right, lists, .. }, .. } = op else {
        unreachable!()
    };
    for (text, at, paragraph) in [
        (ExGuid::default(), 1, paragraph),
        (text, u32::MAX, paragraph),
        (text, 1, ExGuid::default()),
        (text, 1, text),
    ] {
        let op = PageOp::Split { text, at, paragraph, right, lists: lists.clone() };
        assert!(ops::page_edited(SOURCE, sid, vec![op]).is_err());
    }
    let mut titles = 0;
    for (sid, _) in document.pages().unwrap() {
        let space = &document.spaces[&sid];
        let view = &space.revisions[&space.contexts[&ExGuid::default()]];
        let mut pending: Vec<_> = view
            .nodes
            .iter()
            .filter_map(|(id, n)| matches!(n.kind, Kind::Title).then_some(*id))
            .collect();
        let mut seen = std::collections::BTreeSet::new();
        while let Some(id) = pending.pop() {
            if !seen.insert(id) {
                continue;
            }
            let node = &view.nodes[&id];
            pending.extend(node.children.iter().chain(&node.content).copied());
            if matches!(node.kind, Kind::RichText { .. }) {
                assert!(ops::edited(SOURCE, vec![split(SOURCE, sid, id, 0).0]).is_err());
                titles += 1;
            }
        }
    }
    assert!(titles >= 14);
}

#[test]
#[ignore = "exports paragraph splits for independent native validation"]
fn export_native_paragraph_splits() {
    use std::{fs, path::PathBuf};
    let output = PathBuf::from(std::env::var_os("ONESTORE_PARAGRAPH_OUTPUT").unwrap());
    assert!(output.is_absolute());
    fs::create_dir(&output).unwrap();
    let manifest: Value =
        serde_json::from_str(include_str!("../../../corpus/paragraph-edit/manifest.json")).unwrap();
    let mut source = SOURCE.to_vec();
    let mut written = Vec::new();
    for case in manifest["cases"].as_array().unwrap() {
        let text: ExGuid = serde_json::from_value(case["original_text"].clone()).unwrap();
        let store = Store::parse(&source).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let document = Document::parse(&index).unwrap();
        let sid = *document
            .spaces
            .iter()
            .find(|(_, space)| {
                space.revisions[&space.contexts[&ExGuid::default()]]
                    .nodes
                    .contains_key(&text)
            })
            .unwrap()
            .0;
        let at = serde_json::from_value(case["offset_utf16"].clone()).unwrap();
        let (op, new_paragraph, new_text) = split(&source, sid, text, at);
        let transaction = ops::transaction(&source, "Rust split author", vec![op.clone()])
            .unwrap()
            .unwrap();
        written.push(serde_json::json!({"case": case["case"], "op": op,
            "new_paragraph": new_paragraph, "new_text": new_text}));
        transaction.apply(&mut source).unwrap();
    }
    let candidate = output.join("candidate");
    fs::create_dir(&candidate).unwrap();
    fs::write(candidate.join("synthetic.one"), source).unwrap();
    fs::write(
        output.join("manifest.json"),
        serde_json::to_vec_pretty(&serde_json::json!({"cases": written})).unwrap(),
    )
    .unwrap();
}

#[test]
fn interrupted_splits_publish_a_complete_graph_or_retain_the_original() {
    let original = onestore::create_section("split.one", "Original", "Author").unwrap();
    let store = Store::parse(&original).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let (sid, page) = document.pages().unwrap()[0];
    let space = &document.spaces[&sid];
    let view = &space.revisions[&space.contexts[&ExGuid::default()]];
    let outline = view.nodes[&page]
        .children
        .iter()
        .find(|id| matches!(view.nodes[id].kind, Kind::Outline { .. }))
        .unwrap();
    let paragraph = view.nodes[outline].children[0];
    let mut child = ops::paragraph("a🦀b");
    child.level = 2;
    let text = child.text().unwrap().id;
    let inserted = ops::page_edited(
        &original,
        sid,
        vec![
            PageOp::Insert {
                container: paragraph,
                before: None,
                paragraphs: vec![child],
            },
            PageOp::Format {
                text,
                range: 1..3,
                set: vec![onestore::TextAttribute::Bold(true)],
                clear: Vec::new(),
            },
        ],
    )
    .unwrap();
    let source = inserted.as_slice();
    let checkpoint = checkpoint::pending(source, sid, text);
    for source in [source, &checkpoint] {
        let (op, ..) = split(source, sid, text, 1);
        let edit = ops::transaction(source, "Author", vec![op]).unwrap().unwrap();
        let mut written = source.to_vec();
        edit.apply(&mut written).unwrap();
        let before = current::current(source);
        let after = current::current(&written);
        for write_limit in [17, 4096] {
            let disk = |fail_at| disk::Disk {
                visible: source.to_vec(),
                durable: source.to_vec(),
                operation: 0,
                fail_at,
                write_limit,
                random: 347,
            };
            let mut successful = disk(None);
            edit.commit(&mut successful).unwrap();
            assert_eq!(successful.durable, written);
            for at in 1..=successful.operation {
                let mut interrupted = disk(Some(at));
                let error = edit.commit(&mut interrupted).unwrap_err();
                let recovered = current::current(&interrupted.durable);
                assert!(
                    recovered == before || recovered == after,
                    "interruption {at}"
                );
                match error.state {
                    onestore::CommitState::NotCommitted => assert_eq!(recovered, before),
                    onestore::CommitState::Committed => assert_eq!(recovered, after),
                    onestore::CommitState::Unknown => {}
                }
            }
        }
    }
}

#[test]
#[ignore = "exports paragraph joins for independent native validation"]
fn export_native_paragraph_joins() {
    use std::{fs, path::PathBuf};
    let output = PathBuf::from(std::env::var_os("ONESTORE_PARAGRAPH_JOIN_OUTPUT").unwrap());
    assert!(output.is_absolute());
    fs::create_dir(&output).unwrap();
    for (number, (source, _, split_cases)) in JOIN_FIXTURES.into_iter().enumerate() {
        let store = Store::parse(source).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let document = Document::parse(&index).unwrap();
        let cases = join_targets(&document, split_cases);
        let mut source = source.to_vec();
        let mut manifest = Vec::new();
        for (name, left, right) in cases {
            let sid = *document
                .spaces
                .iter()
                .find(|(_, space)| {
                    space.revisions[&space.contexts[&ExGuid::default()]]
                        .nodes
                        .contains_key(&left)
                })
                .unwrap()
                .0;
            let op = Op::Page { space: sid, op: PageOp::Join { left, right } };
            let transaction = ops::transaction(&source, "Rust join author", vec![op.clone()])
                .unwrap()
                .unwrap();
            transaction.apply(&mut source).unwrap();
            manifest.push(serde_json::json!({"case":name,"space":sid,"op":op}));
        }
        let folder = output.join(["split", "inheritance", "tags"][number]);
        fs::create_dir_all(folder.join("candidate")).unwrap();
        fs::write(folder.join("candidate/synthetic.one"), source).unwrap();
        fs::write(
            folder.join("manifest.json"),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
    }
}

#[test]
fn interrupted_joins_preserve_complete_graphs_and_empty_text_adoption() {
    let source = onestore::create_section("join.one", "Left", "Author").unwrap();
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
    let left = view.nodes[&view.nodes[&outline].children[0]].content[0];
    let right = ops::paragraph("Right 🦀");
    let right_text = right.text().unwrap().id;
    let mut child = ops::paragraph("Retained child");
    child.parent = Some(right.id);
    child.level = 2;
    let original = ops::page_edited(
        &source,
        sid,
        vec![
            PageOp::Insert {
                container: outline,
                before: None,
                paragraphs: vec![right, child],
            },
            PageOp::Format {
                text: right_text,
                range: 0..5,
                set: vec![onestore::TextAttribute::Bold(true)],
                clear: Vec::new(),
            },
        ],
    )
    .unwrap();
    let original = original.as_slice();
    let empty = ops::page_edited(
        original,
        sid,
        vec![PageOp::Text {
            text: left,
            range: 0..4,
            with: String::new(),
        }],
    )
    .unwrap();
    let checkpoint = checkpoint::pending(original, sid, left);
    for source in [original, &empty, &checkpoint] {
        let op = Op::Page { space: sid, op: PageOp::Join { left, right: right_text } };
        let edit = ops::transaction(source, "Join author", vec![op]).unwrap().unwrap();
        let mut written = source.to_vec();
        edit.apply(&mut written).unwrap();
        let before = current::current(source);
        let after = current::current(&written);
        for write_limit in [17, 4096] {
            let disk = |fail_at| disk::Disk {
                visible: source.to_vec(),
                durable: source.to_vec(),
                operation: 0,
                fail_at,
                write_limit,
                random: 917,
            };
            let mut successful = disk(None);
            edit.commit(&mut successful).unwrap();
            assert_eq!(successful.durable, written);
            for at in 1..=successful.operation {
                let mut interrupted = disk(Some(at));
                let error = edit.commit(&mut interrupted).unwrap_err();
                let recovered = current::current(&interrupted.durable);
                assert!(
                    recovered == before || recovered == after,
                    "interruption {at}"
                );
                match error.state {
                    onestore::CommitState::NotCommitted => assert_eq!(recovered, before),
                    onestore::CommitState::Committed => assert_eq!(recovered, after),
                    onestore::CommitState::Unknown => {}
                }
            }
        }
    }
}

#[test]
fn joins_reject_invalid_identities_wrong_order_and_unrelated_pages() {
    let (source, _, _) = JOIN_FIXTURES[0];
    let store = Store::parse(source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let cases = join_targets(&document, true);
    let (_, left, right) = &cases[0];
    let (sid, space) = document
        .spaces
        .iter()
        .find(|(_, space)| {
            space.revisions[&space.contexts[&ExGuid::default()]]
                .nodes
                .contains_key(left)
        })
        .unwrap();
    for (a, b) in [
        (*left, *left),
        (ExGuid::default(), *right),
        (*right, *left),
        (*left, cases[1].2),
    ] {
        assert!(join(source, *sid, a, b).is_err());
    }
    let op = Op::Page { space: *sid, op: PageOp::Join { left: *left, right: *right } };
    assert!(ops::transaction(source, "a\0b", vec![op]).is_err());
    let view = &space.revisions[&space.contexts[&ExGuid::default()]];
    let paragraph = *view
        .nodes
        .iter()
        .find(|(_, node)| node.content == [*left])
        .unwrap()
        .0;
    let parent = view
        .nodes
        .values()
        .find(|node| node.children.contains(&paragraph))
        .unwrap();
    let last = view.nodes[parent.children.last().unwrap()].content[0];
    assert!(join(source, *sid, *left, last).is_err());
    let title = view
        .nodes
        .iter()
        .find_map(|(_, node)| matches!(node.kind, Kind::Title).then_some(node))
        .unwrap();
    let mut pending = title.children.clone();
    let mut rejected = 0;
    while let Some(id) = pending.pop() {
        let node = &view.nodes[&id];
        pending.extend(node.children.iter().chain(&node.content).copied());
        if matches!(node.kind, Kind::RichText { .. }) {
            assert!(join(source, *sid, id, *right).is_err());
            rejected += 1;
        }
    }
    assert!(rejected > 0);
}
