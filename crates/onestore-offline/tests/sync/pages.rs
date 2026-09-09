use super::*;
use onestore::{PageEdit, PagePosition};
use onestore_offline::{Operation, Recovery};
use std::collections::BTreeMap;

const SOURCE: &[u8] =
    include_bytes!("../../../../corpus/page-lifecycle/04-nested/notebook/Lifecycle.one");

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
    let id = cache.pages(SOURCE, &edits).unwrap().unwrap();
    assert!(cache.pages(SOURCE, &edits).is_err());
    let mut expected_text = texts(SOURCE);
    let (&(sid, oid), original) = expected_text
        .iter()
        .find(|(_, text)| text.starts_with("Body parent"))
        .unwrap();
    let changed = format!("Offline {original}");
    expected_text.insert((sid, oid), changed);
    cache
        .edit_text(&cache.snapshot().unwrap(), sid, oid, 0..0, "Offline ")
        .unwrap()
        .unwrap();
    let queue = cache.pending().unwrap();
    assert!(matches!(&queue[0].operation, Operation::Pages(batch) if batch.edits == edits));
    let local = cache.snapshot().unwrap();
    drop(cache);
    let cache = Replica::open(&path).unwrap();
    assert_eq!(cache.pending().unwrap(), queue);
    assert_eq!(cache.snapshot().unwrap(), local);
    let mut server = Server::new(SOURCE);
    for edit in queue {
        assert!(matches!(cache.sync_once(&mut server).unwrap(),
            Some((published, EditStatus::Published { .. })) if published == edit.id));
    }
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
    assert_eq!(server.publications, 2);
}

#[test]
fn indentation_only_edits_preserve_remote_page_movement() {
    let directory = tempfile::tempdir().unwrap();
    let cache = Replica::create(directory.path().join("pages.sqlite"), SOURCE).unwrap();
    let before = order(SOURCE);
    let sid = before[4].0;
    let id = cache
        .pages(SOURCE, &[PageEdit::set_level(sid, 3).unwrap()])
        .unwrap()
        .unwrap();
    let mut server = Server::new(SOURCE);
    PreparedEdit::pages(
        SOURCE,
        &[PageEdit::move_to(sid, Some(before[7].0), 2).unwrap()],
    )
    .unwrap()
    .commit(&mut server)
    .unwrap();
    let mut expected = order(&server.durable);
    expected.iter_mut().find(|p| p.0 == sid).unwrap().1 = 3;
    assert!(matches!(cache.sync_once(&mut server).unwrap(),
        Some((published, EditStatus::Published { .. })) if published == id));
    assert_eq!(order(&server.durable), expected);
}

#[test]
fn competing_page_moves_require_current_review_and_retain_intent_identities() {
    let directory = tempfile::tempdir().unwrap();
    let cache = Replica::create(directory.path().join("pages.sqlite"), SOURCE).unwrap();
    let pages = order(SOURCE);
    let sid = pages[6].0;
    let original = PageEdit::move_to(sid, None, 1).unwrap();
    let id = cache
        .pages(SOURCE, std::slice::from_ref(&original))
        .unwrap()
        .unwrap();
    let mut server = Server::new(SOURCE);
    PreparedEdit::pages(
        SOURCE,
        &[PageEdit::move_to(sid, Some(pages[0].0), 1).unwrap()],
    )
    .unwrap()
    .commit(&mut server)
    .unwrap();
    let remote = server.visible.clone();
    let local = cache.snapshot().unwrap();
    assert_eq!(
        cache.sync_once(&mut server).unwrap(),
        Some((id, EditStatus::Conflict(ConflictKind::StructureChanged)))
    );
    assert_eq!(server.publications, 0);
    assert_eq!(cache.snapshot().unwrap(), local);
    let revised = original
        .reposition(PagePosition::Before(Some(pages[2].0)), 1)
        .unwrap();
    let fresh = PageEdit::move_to(sid, Some(pages[2].0), 1).unwrap();
    assert!(
        cache
            .rebase_pages_conflict(id, &local, &remote, &[fresh])
            .is_err()
    );
    assert!(
        cache
            .rebase_pages_conflict(id, SOURCE, &remote, std::slice::from_ref(&revised))
            .is_err()
    );
    assert!(
        cache
            .rebase_pages_conflict(id, &local, SOURCE, std::slice::from_ref(&revised))
            .is_err()
    );
    cache
        .rebase_pages_conflict(id, &local, &remote, std::slice::from_ref(&revised))
        .unwrap();
    assert!(matches!(&cache.pending().unwrap()[0].operation,
        Operation::Pages(batch) if batch.edits == [revised.clone()]));
    let expected = PreparedEdit::pages(&remote, &[revised]).unwrap();
    assert!(matches!(cache.sync_once(&mut server).unwrap(),
        Some((published, EditStatus::Published { .. })) if published == id));
    assert_eq!(order(&server.durable), order(expected.as_bytes()));
}

#[test]
fn one_competing_level_prevents_publication_of_the_whole_batch() {
    let directory = tempfile::tempdir().unwrap();
    let cache = Replica::create(directory.path().join("pages.sqlite"), SOURCE).unwrap();
    let pages = order(SOURCE);
    let id = cache
        .pages(
            SOURCE,
            &[
                PageEdit::set_level(pages[4].0, 3).unwrap(),
                PageEdit::set_level(pages[5].0, 2).unwrap(),
            ],
        )
        .unwrap()
        .unwrap();
    let local = cache.snapshot().unwrap();
    let mut server = Server::new(SOURCE);
    PreparedEdit::pages(SOURCE, &[PageEdit::set_level(pages[5].0, 1).unwrap()])
        .unwrap()
        .commit(&mut server)
        .unwrap();
    let remote = server.durable.clone();
    assert_eq!(
        cache.sync_once(&mut server).unwrap(),
        Some((id, EditStatus::Conflict(ConflictKind::StructureChanged)))
    );
    assert_eq!(server.publications, 0);
    assert_eq!(server.durable, remote);
    assert_eq!(cache.snapshot().unwrap(), local);
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
        let id = cache.pages(SOURCE, &edits).unwrap().unwrap();
        let local = cache.snapshot().unwrap();
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
        assert_eq!(recovery.status(id).unwrap(), Some(attempted));
        assert_eq!(recovery.snapshot().unwrap(), local);
        assert!(
            cache
                .rebase_pages_conflict(id, &local, SOURCE, &edits)
                .is_err()
        );
        let result = cache.sync_once(&mut server).unwrap().unwrap();
        assert_eq!(server.publications, 1);
        if matches!(fault, Fault::UnknownAfter | Fault::PanicAfter) {
            assert!(matches!(result.1, EditStatus::Published { .. }));
            assert_eq!(server.confirmations, 1);
        } else {
            assert_eq!(result, (id, attempted));
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
    let source =
        onestore::replace_property_bytes(SOURCE, root, copy, 0x14001dff, &3_u32.to_le_bytes())
            .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("pages.sqlite");
    let cache = Replica::create(&path, &source).unwrap();
    let id = cache
        .pages(&source, &[PageEdit::set_level(sid, 3).unwrap()])
        .unwrap()
        .unwrap();
    let mut server = Server::new(&source);
    server.fault = Fault::UnknownAfter;
    assert!(cache.sync_once(&mut server).is_err());
    let attempted = cache.status(id).unwrap().unwrap();
    drop(cache);
    let db = rusqlite::Connection::open(&path).unwrap();
    let encoded: String = db
        .query_row("SELECT revisions FROM attempt", [], |r| r.get(0))
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
    assert_eq!(cache.sync_once(&mut server).unwrap(), Some((id, attempted)));
    assert_eq!(server.confirmations, 0);
    server.visible = complete;
    assert!(matches!(
        cache.sync_once(&mut server).unwrap(),
        Some((_, EditStatus::Published { .. }))
    ));
    assert_eq!(server.publications, 1);
    assert_eq!(server.confirmations, 1);
}

#[test]
fn an_explicit_anchor_is_retained_even_when_originally_in_place() {
    let directory = tempfile::tempdir().unwrap();
    let cache = Replica::create(directory.path().join("pages.sqlite"), SOURCE).unwrap();
    let pages = order(SOURCE);
    let id = cache
        .pages(
            SOURCE,
            &[
                PageEdit::move_to(pages[6].0, Some(pages[7].0), 1).unwrap(),
                PageEdit::set_level(pages[4].0, 3).unwrap(),
            ],
        )
        .unwrap()
        .unwrap();
    let mut server = Server::new(SOURCE);
    PreparedEdit::pages(
        SOURCE,
        &[PageEdit::move_to(pages[6].0, Some(pages[0].0), 1).unwrap()],
    )
    .unwrap()
    .commit(&mut server)
    .unwrap();
    assert_eq!(
        cache.sync_once(&mut server).unwrap(),
        Some((id, EditStatus::Conflict(ConflictKind::StructureChanged)))
    );
    assert_eq!(server.publications, 0);
}

#[test]
fn convergent_page_indentation_requires_confirmation_without_republishing() {
    let directory = tempfile::tempdir().unwrap();
    let cache = Replica::create(directory.path().join("pages.sqlite"), SOURCE).unwrap();
    let sid = order(SOURCE)[4].0;
    let id = cache
        .pages(SOURCE, &[PageEdit::set_level(sid, 1).unwrap()])
        .unwrap()
        .unwrap();
    let mut server = Server::new(SOURCE);
    PreparedEdit::pages(SOURCE, &[PageEdit::set_level(sid, 1).unwrap()])
        .unwrap()
        .commit(&mut server)
        .unwrap();
    server.fault = Fault::Confirm;
    assert!(cache.sync_once(&mut server).is_err());
    assert_eq!(cache.status(id).unwrap(), Some(EditStatus::Pending));
    assert_eq!(server.publications, 0);
    assert!(matches!(cache.sync_once(&mut server).unwrap(),
        Some((published, EditStatus::Published { .. })) if published == id));
    assert_eq!(server.publications, 0);
    assert_eq!(server.confirmations, 2);
}

#[test]
fn schema_ten_migration_preserves_multi_space_uncertainty() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("pages.sqlite");
    let cache = Replica::create(&path, SOURCE).unwrap();
    let page = onestore::PageCreation::new(None, Some("Migration"), "Author").unwrap();
    let id = cache.create_page(SOURCE, &page).unwrap().unwrap();
    let mut server = Server::new(SOURCE);
    server.fault = Fault::UnknownBefore;
    assert!(cache.sync_once(&mut server).is_err());
    let local = cache.snapshot().unwrap();
    let queue = cache.pending().unwrap();
    let status = cache.status(id).unwrap();
    drop(cache);
    let db = rusqlite::Connection::open(&path).unwrap();
    db.pragma_update(None, "user_version", 10).unwrap();
    let proofs: String = db
        .query_row("SELECT revisions FROM attempt", [], |r| r.get(0))
        .unwrap();
    drop(db);
    let cache = Replica::open(&path).unwrap();
    assert_eq!(cache.snapshot().unwrap(), local);
    assert_eq!(cache.pending().unwrap(), queue);
    assert_eq!(cache.status(id).unwrap(), status);
    assert_eq!(
        cache.sync_once(&mut server).unwrap(),
        Some((id, status.unwrap()))
    );
    assert_eq!(server.publications, 1);
    drop(cache);
    let db = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(
        db.query_row("SELECT revisions FROM attempt", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        proofs
    );
    assert_eq!(
        db.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
            .unwrap(),
        11
    );
}

#[test]
fn native_page_changes_reconcile_with_atomic_offline_batches_and_review() {
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
            let id = cache.pages(SOURCE, &edits).unwrap().unwrap();
            let text_id = cache
                .edit_text(
                    &cache.snapshot().unwrap(),
                    text_space,
                    text_object,
                    0..0,
                    "Offline ",
                )
                .unwrap()
                .unwrap();
            let native = std::fs::read(root.join(phase).join("notebook/Lifecycle.one")).unwrap();
            let mut server = Server::new(&native);
            let local = cache.snapshot().unwrap();
            let expected_conflict = if mode == "indent" {
                matches!(
                    phase.as_str(),
                    "04-subpage-move" | "07-promoted-root" | "08-collapsed-group-move"
                )
            } else {
                !matches!(
                    phase.as_str(),
                    "02-promoted-parent" | "03-selected-group-move" | "06-promoted-level-two"
                )
            };
            let first = cache.sync_once(&mut server).unwrap().unwrap();
            assert_eq!(first.0, id);
            let mut reviewed = edits.clone();
            if expected_conflict {
                assert_eq!(
                    first.1,
                    EditStatus::Conflict(ConflictKind::StructureChanged),
                    "{mode}:{phase}"
                );
                assert_eq!(server.publications, 0);
                assert_eq!(cache.snapshot().unwrap(), local);
                if mode == "indent" && phase == "08-collapsed-group-move" {
                    reviewed[0] = reviewed[0].reposition(PagePosition::Keep, 1).unwrap();
                }
                cache
                    .rebase_pages_conflict(id, &local, &native, &reviewed)
                    .unwrap();
                assert!(matches!(cache.sync_once(&mut server).unwrap(),
                    Some((published, EditStatus::Published { .. })) if published == id));
            } else {
                assert!(
                    matches!(first.1, EditStatus::Published { .. }),
                    "{mode}:{phase}"
                );
            }
            counts[usize::from(expected_conflict)] += 1;
            drop(cache);
            let cache = Replica::open(&path).unwrap();
            assert!(matches!(cache.sync_once(&mut server).unwrap(),
                Some((published, EditStatus::Published { .. })) if published == text_id));
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
            assert_eq!(
                server.publications,
                1 + usize::from(expected != order(&native))
            );
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
                        "reviewed": reviewed, "conflict": expected_conflict,
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
    assert_eq!(counts, [9, 9]);
}

#[test]
fn twelve_disjoint_page_batches_merge_with_dependent_bodies_without_review() {
    let mut source = onestore::create_section("pages.one", "Sentinel", "Author").unwrap();
    let mut created = Vec::new();
    for _ in 0..36 {
        let page = onestore::PageCreation::new(None, Some("Same title"), "Author").unwrap();
        source = PreparedEdit::create_page(&source, &page)
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
        cache.pages(&source, &edits).unwrap().unwrap();
        let body = format!("Actor {actor} 🦀 é");
        let insertion =
            onestore::Insertion::outline(group[1].object(), 36.0, 36.0, &body, "Author").unwrap();
        cache
            .insert(&cache.snapshot().unwrap(), group[1].space(), &insertion)
            .unwrap()
            .unwrap();
        expected.extend([
            (group[1].space(), 1),
            (group[0].space(), 1),
            (group[2].space(), 2),
        ]);
        expected_text.insert((group[1].space(), insertion.text_object()), body);
        queues.push((path, cache.pending().unwrap()));
    }
    let mut server = Server::new(&source);
    for (path, queue) in &queues {
        let cache = Replica::open(path).unwrap();
        assert_eq!(cache.pending().unwrap(), *queue);
        for edit in queue {
            assert!(matches!(cache.sync_once(&mut server).unwrap(),
                Some((published, EditStatus::Published { .. })) if published == edit.id));
        }
        assert!(cache.pending().unwrap().is_empty());
    }
    assert_eq!(server.publications, 24);
    assert_eq!(order(&server.durable), expected);
    assert_eq!(texts(&server.durable), expected_text);
    for (path, queue) in &queues {
        let cache = Replica::open(path).unwrap();
        assert_eq!(cache.sync_once(&mut server).unwrap(), None);
        assert_eq!(order(&cache.snapshot().unwrap()), expected);
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
                    .map(|edit| {
                        serde_json::json!({"id": edit.id, "space": edit.space,
                "operation": edit.operation})
                    })
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
