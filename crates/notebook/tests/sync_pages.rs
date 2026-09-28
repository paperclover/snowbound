//! Page batch reconciliation: atomic batches with dependent body saves, competing moves and
//! indentation (a page the remote moved keeps the remote's placement), uncertain batches, and
//! native page movement fixtures.

#[path = "../../onestore/tests/support/ops.rs"]
mod ops;

use notebook::{EditStatus, Recovery, Replica};
use onestore::op::{Op, SectionOp};
use onestore::{
    ExGuid, PageEdit, PagePosition, RevisionIndex, Store,
    document::{Document, Format, Kind, Layout},
    page::{
        Outline, Page, PageObject, PageParagraph, Paragraph, ParagraphContent, TextObject,
        text::new_id,
    },
};
use std::collections::BTreeMap;

#[path = "support/server.rs"]
mod server;
use server::*;
#[path = "support/model_ops.rs"]
mod model_ops;

const SOURCE: &[u8] =
    include_bytes!("../../../corpus/page-lifecycle/04-nested/notebook/Lifecycle.one");

fn order(source: &[u8]) -> Vec<(ExGuid, u32)> {
    let store = Store::parse(source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    index.validate_current().unwrap();
    let document = Document::parse(&index).unwrap();
    document
        .pages()
        .unwrap()
        .iter()
        .map(|(sid, _)| {
            let space = &document.spaces[sid];
            let view = &space.revisions[&space.contexts[&ExGuid::default()]];
            let Kind::Metadata { level, .. } = view.nodes[&view.roots[&2]].kind else {
                panic!()
            };
            (*sid, level.unwrap_or(1))
        })
        .collect()
}

fn texts(source: &[u8]) -> BTreeMap<(ExGuid, ExGuid), String> {
    let store = Store::parse(source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let mut texts = BTreeMap::new();
    for (sid, space) in &document.spaces {
        let view = &space.revisions[&space.contexts[&ExGuid::default()]];
        for (oid, node) in &view.nodes {
            if let Kind::RichText { text, .. } = &node.kind {
                texts.insert((*sid, *oid), text.clone());
            }
        }
    }
    texts
}

/// Adds a body outline holding one plain paragraph, returning its text identity. A page the
/// section just created has no body text for `model_ops::insert_outline` to copy formatting from.
fn body_outline(page: &mut Page, text: &str) -> ExGuid {
    // The writer creates outline text with the store's default style; the model must state it.
    let format = Format {
        font: Some("Calibri".to_owned()),
        font_size: Some(11.0),
        language: Some(0x409),
        ..Default::default()
    };
    let content = TextObject {
        id: new_id().unwrap(),
        date_field: None,
        text: Paragraph::new(text.into(), format),
        tags: Vec::new(),
    };
    let id = content.id;
    let outline = Outline {
        id: new_id().unwrap(),
        title: false,
        min_width: None,
        layout: Layout {
            x: Some(36.0),
            y: Some(36.0),
            ..Default::default()
        },
        indents: Vec::new(),
        paragraphs: vec![PageParagraph {
            id: new_id().unwrap(),
            parent: None,
            level: 1,
            style: None,
            format: Default::default(),
            content: ParagraphContent::Text(content),
            lists: Vec::new(),
            tags: Vec::new(),
            media: Default::default(),
            collapsed: false,
        }],
        unsupported: Vec::new(),
    };
    let at = page
        .objects
        .iter()
        .position(|object| matches!(object, PageObject::Title(_)))
        .unwrap_or(page.objects.len());
    page.objects.insert(at, PageObject::Outline(outline));
    id
}

#[test]
fn atomic_page_batches_survive_reopen_with_dependent_text() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("pages.sqlite");
    let cache = Replica::create(&path, SOURCE).unwrap();
    let before = order(SOURCE);
    let edits: Vec<_> = before[3..6]
        .iter()
        .map(|(sid, level)| PageEdit::move_to(*sid, None, *level).unwrap())
        .collect();
    let id = section_op(&cache, SectionOp::Pages(edits.to_vec()));
    let mut expected_text = texts(SOURCE);
    let (&(sid, oid), original) = expected_text
        .iter()
        .find(|(_, text)| text.starts_with("Body parent"))
        .unwrap();
    let changed = format!("Offline {original}");
    expected_text.insert((sid, oid), changed);
    model_ops::save(&cache, oid, |page| {
        model_ops::replace_text(page, oid, 0..0, "Offline ")
    })
    .unwrap()
    .unwrap();
    let queue = cache.pending().unwrap();
    assert!(
        matches!(&queue[0].edit.ops[..], [Op::Section(SectionOp::Pages(batch))] if *batch == edits)
    );
    let local = snapshot(&cache);
    drop(cache);
    let cache = Replica::open(&path).unwrap();
    assert_eq!(cache.pending().unwrap(), queue);
    assert_eq!(server::pages(&snapshot(&cache)), server::pages(&local));
    let mut server = Server::new(SOURCE);
    assert!(matches!(
        cache.sync_once(&mut server).unwrap().edit,
        Some((_, EditStatus::Published { .. }))
    ));
    let expected = [
        before[..3].to_vec(),
        before[6..].to_vec(),
        before[3..6].to_vec(),
    ]
    .concat();
    assert_eq!(order(&server.durable), expected);
    assert_eq!(texts(&server.durable), expected_text);
    assert!(matches!(
        cache.status(id).unwrap(),
        Some(EditStatus::Published { .. })
    ));
    assert_eq!(server.publications, 1);
}

#[test]
fn indenting_a_page_the_remote_moved_keeps_the_remote_placement() {
    let directory = tempfile::tempdir().unwrap();
    let cache = Replica::create(directory.path().join("pages.sqlite"), SOURCE).unwrap();
    let before = order(SOURCE);
    let sid = before[4].0;
    let id = section_op(
        &cache,
        SectionOp::Pages([PageEdit::set_level(sid, 3).unwrap()].to_vec()),
    );
    let mut server = Server::new(SOURCE);
    ops::section_op(
        SOURCE,
        SectionOp::Pages([PageEdit::move_to(sid, Some(before[7].0), 2).unwrap()].to_vec()),
    )
    .unwrap()
    .commit(&mut server)
    .unwrap();
    let expected = order(&server.durable);
    assert!(matches!(cache.sync_once(&mut server).unwrap().edit,
        Some((published, EditStatus::Published { .. })) if published == id));
    assert_eq!(order(&server.durable), expected);
}

/// A page both sides moved stays where the remote put it, as OneNote 2010 keeps it
/// (corpus/conflict-page/native-pages).
#[test]
fn competing_page_moves_keep_the_remote_placement() {
    let directory = tempfile::tempdir().unwrap();
    let cache = Replica::create(directory.path().join("pages.sqlite"), SOURCE).unwrap();
    let pages = order(SOURCE);
    let sid = pages[6].0;
    let id = section_op(
        &cache,
        SectionOp::Pages(vec![PageEdit::move_to(sid, None, 1).unwrap()]),
    );
    let mut server = Server::new(SOURCE);
    ops::section_op(
        SOURCE,
        SectionOp::Pages([PageEdit::move_to(sid, Some(pages[0].0), 1).unwrap()].to_vec()),
    )
    .unwrap()
    .commit(&mut server)
    .unwrap();
    let expected = order(&server.visible);
    assert!(matches!(cache.sync_once(&mut server).unwrap().edit,
        Some((published, EditStatus::Published { .. })) if published == id));
    assert_eq!(order(&server.durable), expected);
}

#[test]
fn a_competing_level_keeps_the_remote_level_and_the_rest_of_the_batch() {
    let directory = tempfile::tempdir().unwrap();
    let cache = Replica::create(directory.path().join("pages.sqlite"), SOURCE).unwrap();
    let pages = order(SOURCE);
    let edits = vec![
        PageEdit::set_level(pages[4].0, 3).unwrap(),
        PageEdit::set_level(pages[5].0, 2).unwrap(),
    ];
    let id = section_op(&cache, SectionOp::Pages(edits.clone()));
    let mut server = Server::new(SOURCE);
    ops::section_op(
        SOURCE,
        SectionOp::Pages([PageEdit::set_level(pages[5].0, 1).unwrap()].to_vec()),
    )
    .unwrap()
    .commit(&mut server)
    .unwrap();
    let expected = ops::section_op(
        &server.durable.clone(),
        SectionOp::Pages(edits[..1].to_vec()),
    )
    .unwrap();
    assert!(matches!(cache.sync_once(&mut server).unwrap().edit,
        Some((published, EditStatus::Published { .. })) if published == id));
    assert_eq!(order(&server.durable), order(expected.as_bytes()));
}

#[test]
fn uncertain_page_batches_survive_recovery_and_do_not_replay() {
    for fault in [
        Fault::UnknownBefore,
        Fault::UnknownAfter,
        Fault::PanicBefore,
        Fault::PanicAfter,
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("pages.sqlite");
        let mut cache = Replica::create(&path, SOURCE).unwrap();
        let pages = order(SOURCE);
        let edits = [
            PageEdit::set_level(pages[4].0, 3).unwrap(),
            PageEdit::set_level(pages[5].0, 2).unwrap(),
        ];
        let id = section_op(&cache, SectionOp::Pages(edits.to_vec()));
        let local = snapshot(&cache);
        let queue = cache.pending().unwrap();
        let mut server = Server::new(SOURCE);
        server.fault = fault;
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            cache.sync_once(&mut server)
        }));
        drop(cache);
        cache = Replica::open(&path).unwrap();
        let attempted = cache.status(id).unwrap().unwrap();
        assert!(matches!(attempted, EditStatus::AwaitingConfirmation { .. }));
        let archive = directory.path().join("recovery.sqlite");
        cache.export_recovery(&archive).unwrap();
        let recovery = Recovery::open(&archive).unwrap();
        assert_eq!(recovery.pending().unwrap(), queue);
        assert_eq!(recovery.status(id).unwrap(), Some(attempted.clone()));
        assert_eq!(
            server::pages(&recovery.snapshot().unwrap()),
            server::pages(&local)
        );
        let result = cache.sync_once(&mut server).unwrap().edit.unwrap();
        assert_eq!(server.publications, 1);
        if matches!(fault, Fault::UnknownAfter | Fault::PanicAfter) {
            assert!(matches!(result.1, EditStatus::Published { .. }));
            assert_eq!(server.confirmations, 1);
        } else {
            assert_eq!(result, (id, attempted.clone()));
            assert_eq!(server.confirmations, 0);
        }
    }
}

#[test]
fn an_unchanged_section_revision_cannot_confirm_a_changed_page() {
    let before = order(SOURCE);
    let sid = before[4].0;
    let store = Store::parse(SOURCE).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let root = index.root;
    let doc = Document::parse(&index).unwrap();
    let space = &doc.spaces[&root];
    let view = &space.revisions[&space.contexts[&ExGuid::default()]];
    let copy = *view.nodes.iter().find(|(_, node)|
        matches!(&node.kind, Kind::Metadata { title: Some(title), .. } if title == "child"))
        .unwrap().0;
    // The section's copy of that page's metadata says level 3, stored where it lies.
    let revision = index.resolve_active(root).unwrap();
    let onestore::ObjectData::Properties(data) = revision.objects[&copy].data else {
        panic!()
    };
    let properties = onestore::PropertySets::parse(data).unwrap();
    let onestore::Value::Bytes(level) = properties.sets[0]
        .iter()
        .find(|property| property.id == 0x14001dff)
        .unwrap()
        .value
    else {
        panic!()
    };
    let at = level.as_ptr().addr() - SOURCE.as_ptr().addr();
    let mut source = SOURCE.to_vec();
    source[at..at + 4].copy_from_slice(&3_u32.to_le_bytes());
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("pages.sqlite");
    let cache = Replica::create(&path, &source).unwrap();
    let id = section_op(
        &cache,
        SectionOp::Pages([PageEdit::set_level(sid, 3).unwrap()].to_vec()),
    );
    let mut server = Server::new(&source);
    server.fault = Fault::UnknownAfter;
    assert!(cache.sync_once(&mut server).is_err());
    let attempted = cache.status(id).unwrap().unwrap();
    drop(cache);
    let db = rusqlite::Connection::open(&path).unwrap();
    let encoded: String = db
        .query_row("SELECT revisions FROM batches WHERE attempted=1", [], |r| {
            r.get(0)
        })
        .unwrap();
    let proofs: BTreeMap<ExGuid, ExGuid> = serde_json::from_str(&encoded).unwrap();
    assert_eq!(proofs.len(), 2);
    let store = Store::parse(&source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    assert_eq!(
        proofs[&root],
        index.spaces[&root].labels[&(ExGuid::default(), 1)]
    );
    assert!(!index.spaces[&sid].revisions.contains_key(&proofs[&sid]));
    drop(db);
    let cache = Replica::open(&path).unwrap();
    let complete = server.visible.clone();
    server.visible = source;
    assert_eq!(
        cache.sync_once(&mut server).unwrap().edit,
        Some((id, attempted.clone()))
    );
    assert_eq!(server.confirmations, 0);
    server.visible = complete;
    assert!(matches!(
        cache.sync_once(&mut server).unwrap().edit,
        Some((_, EditStatus::Published { .. }))
    ));
    assert_eq!(server.publications, 1);
    assert_eq!(server.confirmations, 1);
}

#[test]
fn an_explicit_anchor_yields_to_a_remote_move() {
    let directory = tempfile::tempdir().unwrap();
    let cache = Replica::create(directory.path().join("pages.sqlite"), SOURCE).unwrap();
    let pages = order(SOURCE);
    let id = section_op(
        &cache,
        SectionOp::Pages(vec![
            PageEdit::move_to(pages[6].0, Some(pages[7].0), 1).unwrap(),
            PageEdit::set_level(pages[4].0, 3).unwrap(),
        ]),
    );
    let mut server = Server::new(SOURCE);
    ops::section_op(
        SOURCE,
        SectionOp::Pages([PageEdit::move_to(pages[6].0, Some(pages[0].0), 1).unwrap()].to_vec()),
    )
    .unwrap()
    .commit(&mut server)
    .unwrap();
    let expected = ops::section_op(
        &server.durable.clone(),
        SectionOp::Pages(vec![PageEdit::set_level(pages[4].0, 3).unwrap()]),
    )
    .unwrap();
    assert!(matches!(cache.sync_once(&mut server).unwrap().edit,
        Some((published, EditStatus::Published { .. })) if published == id));
    assert_eq!(order(&server.durable), order(expected.as_bytes()));
}

#[test]
fn convergent_page_indentation_requires_confirmation_without_republishing() {
    let directory = tempfile::tempdir().unwrap();
    let cache = Replica::create(directory.path().join("pages.sqlite"), SOURCE).unwrap();
    let sid = order(SOURCE)[4].0;
    let id = section_op(
        &cache,
        SectionOp::Pages([PageEdit::set_level(sid, 1).unwrap()].to_vec()),
    );
    let mut server = Server::new(SOURCE);
    ops::section_op(
        SOURCE,
        SectionOp::Pages([PageEdit::set_level(sid, 1).unwrap()].to_vec()),
    )
    .unwrap()
    .commit(&mut server)
    .unwrap();
    server.fault = Fault::Confirm;
    assert!(cache.sync_once(&mut server).is_err());
    assert_eq!(cache.status(id).unwrap(), Some(EditStatus::Pending));
    assert_eq!(server.publications, 0);
    assert!(matches!(cache.sync_once(&mut server).unwrap().edit,
        Some((published, EditStatus::Published { .. })) if published == id));
    assert_eq!(server.publications, 0);
    assert_eq!(server.confirmations, 2);
}
#[test]
fn native_page_changes_reconcile_with_atomic_offline_batches() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus/page-lifecycle/movement");
    let provenance: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("provenance.json")).unwrap()).unwrap();
    let phases = provenance["cold"].as_object().unwrap();
    assert_eq!(phases.len(), 9);
    let original = order(SOURCE);
    let original_text = texts(SOURCE);
    let (&(text_space, text_object), body) = original_text
        .iter()
        .find(|(_, text)| text.starts_with("Body parent"))
        .unwrap();
    let mut expected_text = original_text.clone();
    expected_text.insert((text_space, text_object), format!("Offline {body}"));
    let mut counts = [0; 2];
    for mode in ["indent", "group"] {
        for phase in phases.keys() {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("pages.sqlite");
            let cache = Replica::create(&path, SOURCE).unwrap();
            let edits = if mode == "indent" {
                vec![PageEdit::set_level(original[4].0, 3).unwrap()]
            } else {
                original[3..6]
                    .iter()
                    .map(|(sid, level)| PageEdit::move_to(*sid, None, *level).unwrap())
                    .collect()
            };
            let id = section_op(&cache, SectionOp::Pages(edits.to_vec()));
            let text_id = model_ops::save(&cache, text_object, |page| {
                model_ops::replace_text(page, text_object, 0..0, "Offline ")
            })
            .unwrap()
            .unwrap();
            let native = std::fs::read(root.join(phase).join("notebook/Lifecycle.one")).unwrap();
            let mut server = Server::new(&native);
            // The page list takes the edits of the pages OneNote left in their series, at
            // their level and after the same page.
            let both: Vec<ExGuid> = order(&native)
                .into_iter()
                .map(|(sid, _)| sid)
                .filter(|sid| original.iter().any(|(s, _)| s == sid))
                .collect();
            let placement = |image: &[u8]| -> BTreeMap<ExGuid, (ExGuid, Option<ExGuid>, u32)> {
                let arena = onestore::Arena::default();
                let series = onestore::Section::open(&arena, image.to_vec())
                    .unwrap()
                    .series()
                    .unwrap();
                let shared: Vec<_> = order(image)
                    .into_iter()
                    .filter(|(sid, _)| both.contains(sid))
                    .collect();
                (0..shared.len())
                    .map(|at| {
                        let (sid, level) = shared[at];
                        (
                            sid,
                            (series[&sid], at.checked_sub(1).map(|b| shared[b].0), level),
                        )
                    })
                    .collect()
            };
            let (before, after) = (placement(SOURCE), placement(&native));
            let reviewed: Vec<PageEdit> = edits
                .iter()
                .filter(|edit| after.get(&edit.space()) == before.get(&edit.space()))
                .cloned()
                .collect();
            let trimmed = reviewed != edits;
            assert!(
                matches!(cache.sync_once(&mut server).unwrap().edit, Some((published, EditStatus::Published { .. })) if published == text_id),
                "{mode}:{phase}"
            );
            counts[usize::from(trimmed)] += 1;
            drop(cache);
            let cache = Replica::open(&path).unwrap();
            assert!(cache.pending().unwrap().is_empty());
            let mut expected = order(&native);
            for edit in &reviewed {
                let at = expected.iter().position(|p| p.0 == edit.space()).unwrap();
                expected[at].1 = edit.level();
                if let PagePosition::Before(before) = edit.position() {
                    let page = expected.remove(at);
                    let at = before.map_or(expected.len(), |sid| {
                        expected.iter().position(|p| p.0 == sid).unwrap()
                    });
                    expected.insert(at, page);
                }
            }
            assert_eq!(order(&server.durable), expected, "{mode}:{phase}");
            assert_eq!(texts(&server.durable), expected_text, "{mode}:{phase}");
            assert_eq!(server.publications, 1);
            if let Some(output) = std::env::var_os("ONESTORE_OFFLINE_PAGE_EDIT_OUTPUT") {
                let output = std::path::Path::new(&output);
                assert!(output.is_absolute());
                let path = output.join(format!("{mode}-{phase}"));
                std::fs::create_dir_all(&path).unwrap();
                std::fs::write(path.join("Lifecycle.one"), &server.durable).unwrap();
                std::fs::write(
                    path.join("manifest.json"),
                    serde_json::to_vec_pretty(
                        &serde_json::json!({"mode": mode, "phase": phase, "original": edits,
                        "reviewed": reviewed, "conflict": trimmed,
                        "publications": server.publications, "confirmations": server.confirmations,
                        "page_receipt": format!("{:?}", cache.status(id).unwrap()),
                        "text_receipt": format!("{:?}", cache.status(text_id).unwrap())}),
                    )
                    .unwrap(),
                )
                .unwrap();
            }
        }
    }
    assert_eq!(counts.iter().sum::<usize>(), 18);
}

#[test]
fn twelve_disjoint_page_batches_merge_with_dependent_bodies_without_review() {
    let mut source = onestore::create_section("pages.one", "Sentinel", "Author").unwrap();
    let mut created = Vec::new();
    for _ in 0..36 {
        let page = onestore::PageCreation::new(None, Some("Same title"), "Author").unwrap();
        source = ops::section_op(&source, SectionOp::Create(page.clone()))
            .unwrap()
            .as_bytes()
            .to_vec();
        created.push(page);
    }
    let mut expected = vec![order(&source)[0]];
    let mut expected_text = texts(&source);
    let directory = tempfile::tempdir().unwrap();
    let mut queues = Vec::new();
    for (actor, group) in created.chunks_exact(3).enumerate() {
        let path = directory.path().join(format!("{actor}.sqlite"));
        let cache = Replica::create(&path, &source).unwrap();
        let edits = [
            PageEdit::move_to(group[1].space(), Some(group[0].space()), 1).unwrap(),
            PageEdit::set_level(group[2].space(), 2).unwrap(),
        ];
        section_op(&cache, SectionOp::Pages(edits.to_vec()));
        let body = format!("Actor {actor} 🦀 e\u{301}");
        let local = snapshot(&cache);
        let mut model = model_ops::page_of(&local, group[1].space());
        let text = body_outline(&mut model, &body);
        model_ops::save_as(&cache, group[1].space(), &model, "Author")
            .unwrap()
            .unwrap();
        expected.extend([
            (group[1].space(), 1),
            (group[0].space(), 1),
            (group[2].space(), 2),
        ]);
        expected_text.insert((group[1].space(), text), body);
        queues.push((path, cache.pending().unwrap()));
    }
    let mut server = Server::new(&source);
    for (path, queue) in &queues {
        let cache = Replica::open(path).unwrap();
        assert_eq!(cache.pending().unwrap(), *queue);
        assert!(matches!(
            cache.sync_once(&mut server).unwrap().edit,
            Some((_, EditStatus::Published { .. }))
        ));
        assert!(cache.pending().unwrap().is_empty());
    }
    assert_eq!(server.publications, 12);
    assert_eq!(order(&server.durable), expected);
    assert_eq!(texts(&server.durable), expected_text);
    for (path, queue) in &queues {
        let cache = Replica::open(path).unwrap();
        assert_eq!(cache.sync_once(&mut server).unwrap().edit, None);
        assert_eq!(order(&snapshot(&cache)), expected);
        for edit in queue {
            assert!(matches!(
                cache.status(edit.id).unwrap(),
                Some(EditStatus::Published { .. })
            ));
        }
    }
    if let Some(output) = std::env::var_os("ONESTORE_OFFLINE_PAGE_CLIENT_OUTPUT") {
        let output = std::path::Path::new(&output);
        assert!(output.is_absolute());
        std::fs::create_dir_all(output).unwrap();
        std::fs::write(output.join("pages.one"), &server.durable).unwrap();
        let operations: Vec<_> = queues
            .iter()
            .map(|(_, queue)| {
                queue
                    .iter()
                    .map(|edit| serde_json::json!({"id": edit.id, "edit": edit.edit}))
                    .collect::<Vec<_>>()
            })
            .collect();
        std::fs::write(
            output.join("manifest.json"),
            serde_json::to_vec_pretty(
                &serde_json::json!({"operations": operations, "publications": server.publications,
                "expected_order": expected, "pages": 37}),
            )
            .unwrap(),
        )
        .unwrap();
    }
}

/// Two OneNote 2010 clients' page merges (`corpus/conflict-page/native-*`), replayed: client
/// A, offline, `moves` pages (each last, or before another) and edits `Target`'s body while
/// client B publishes `b-published`. The pages the merge lists after the first, and
/// `Target`'s space and texts.
fn native_page_merge(
    capture: &str,
    moves: &[(&str, Option<&str>)],
) -> (Vec<String>, ExGuid, Vec<String>) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus/conflict-page")
        .join(capture);
    let read =
        |phase: &str| std::fs::read(root.join(phase).join("notebook/synthetic.one")).unwrap();
    let pages = |image: &[u8]| {
        let arena = onestore::Arena::default();
        onestore::Section::open(&arena, image.to_vec())
            .unwrap()
            .pages()
            .unwrap()
    };
    let initial = read("initial");
    let listed = pages(&initial);
    let space = |title: &str| listed.iter().find(|page| page.1 == title).unwrap().0;
    let directory = tempfile::tempdir().unwrap();
    let cache = Replica::create(directory.path().join("pages.sqlite"), &initial).unwrap();
    for (page, before) in moves {
        section_op(
            &cache,
            SectionOp::Pages(vec![
                PageEdit::move_to(space(page), before.map(space), 1).unwrap(),
            ]),
        );
    }
    let body = texts(&initial)
        .into_iter()
        .find(|(_, text)| text == "Body Target.")
        .unwrap()
        .0
        .1;
    model_ops::save(&cache, body, |page| {
        model_ops::replace_text(page, body, 0..12, "Body Target edited offline.")
    })
    .unwrap()
    .unwrap();
    let mut server = Server::new(&read("b-published"));
    for _ in 0..moves.len() + 2 {
        cache.sync_once(&mut server).unwrap();
    }
    assert!(cache.pending().unwrap().is_empty(), "{capture}");
    let merged = pages(&server.durable);
    let target = merged.iter().find(|page| page.1 == "Target").unwrap().0;
    let texts = texts(&server.durable)
        .into_iter()
        .filter(|((page, _), _)| *page == target)
        .map(|(_, text)| text)
        .collect();
    (
        merged[1..].iter().map(|page| page.1.clone()).collect(),
        target,
        texts,
    )
}

/// Each side keeps the pages it gave a series of their own, the remote's where both did;
/// a page the remote deleted comes back as a new page where the queue had it. OneNote's
/// COM moves re-series the pages a page jumps over, so its moves replay as moves of those.
#[test]
fn page_moves_and_a_removed_page_merge_as_onenote_merges_them() {
    for (capture, moves) in [
        (
            "native-pages",
            &[
                ("One", Some("Target")),
                ("Three", Some("Target")),
                ("Two", None),
            ][..],
        ),
        (
            "native-restore",
            &[
                ("One", None),
                ("Two", None),
                ("Target", None),
                ("Three", None),
            ][..],
        ),
        ("native-restore", &[("Four", Some("One"))][..]),
    ] {
        let root =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/conflict-page");
        let native: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join(capture).join("merged.json")).unwrap())
                .unwrap();
        let (order, target, texts) = native_page_merge(capture, moves);
        let native: Vec<String> = serde_json::from_value(native["order"].clone()).unwrap();
        assert_eq!(order, native[1..], "{capture}");
        assert!(
            texts.contains(&"Body Target edited offline.".to_owned()),
            "{capture}"
        );
        let arena = onestore::Arena::default();
        let initial =
            std::fs::read(root.join(capture).join("initial/notebook/synthetic.one")).unwrap();
        let listed = onestore::Section::open(&arena, initial)
            .unwrap()
            .pages()
            .unwrap();
        assert!(
            listed.iter().all(|page| page.0 != target),
            "{capture}: a new page"
        );
    }
}
