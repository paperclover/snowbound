//! Page creation reconciliation: dependent body saves, uncertain publication, anchor review,
//! and deterministic multi-actor schedules.

use notebook::{ConflictKind, EditStatus, Operation, Recovery, Replica};
use onestore::{
    ExGuid, PageCreation, RevisionIndex, Store,
    document::{Document, Kind},
};
use std::collections::BTreeMap;

#[path = "support/server.rs"]
mod server;
use server::*;
#[path = "../../onestore/tests/support/disk.rs"]
mod disk;
#[path = "support/model_ops.rs"]
mod model_ops;
#[path = "support/page_schedule.rs"]
mod page_schedule;
use page_schedule::body_outline;

#[test]
fn twelve_replica_page_schedules_retain_acknowledged_pages_through_interruptions() {
    for seed in 0..24_u64 {
        let mut random = seed + 1956;
        let mut input = Vec::new();
        for step in 0..48 {
            random ^= random << 13;
            random ^= random >> 7;
            random ^= random << 17;
            let mut action = random.to_le_bytes();
            if step < 12 {
                action[0] = step;
                action[1] = 1;
            }
            input.extend_from_slice(&action);
        }
        page_schedule::run(&input);
    }
}

#[test]
fn offline_page_creation_rebases_with_dependent_edits_and_duplicate_titles() {
    let source = onestore::create_section("pages.one", "Original", "Author").unwrap();
    let directory = tempfile::tempdir().unwrap();
    let mut server = Server::new(&source);
    let mut expected = Vec::new();
    for actor in 0..12 {
        let path = directory.path().join(format!("{actor}.sqlite"));
        let cache = Replica::create(&path, &source).unwrap();
        let page = PageCreation::new(None, Some("Same 🦋 é"), "Offline author").unwrap();
        let id = cache.create_page(&source, &page).unwrap().unwrap();
        let mut created = model_ops::page_of(&cache.snapshot().unwrap(), page.space());
        let body = body_outline(&mut created, "Body");
        let save = cache
            .save(
                &cache.snapshot().unwrap(),
                page.space(),
                &created,
                model_ops::AUTHOR,
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            model_ops::save(&cache, body, |page| model_ops::replace_text(
                page,
                body,
                4..4,
                &format!(" {actor}")
            ))
            .unwrap(),
            Some(save),
            "a dependent text edit coalesces into the unattempted body save"
        );
        let original = cache.pending().unwrap();
        assert_eq!(original.len(), 2);
        assert!(
            matches!(&original[0].operation, Operation::CreatePage(retained) if retained == &page)
        );
        let local = cache.snapshot().unwrap();
        drop(cache);
        let cache = Replica::open(&path).unwrap();
        assert_eq!(cache.pending().unwrap(), original);
        assert_eq!(cache.snapshot().unwrap(), local);
        for edit in original {
            assert!(matches!(cache.sync_once(&mut server).unwrap(),
                Some((published, EditStatus::Published { .. })) if published == edit.id));
        }
        assert!(matches!(
            cache.status(id).unwrap(),
            Some(EditStatus::Published { .. })
        ));
        expected.push((page.space(), page.object(), body, format!("Body {actor}")));
    }
    let store = Store::parse(&server.durable).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let pages = document.pages().unwrap();
    assert_eq!(pages.len(), 13);
    for (actual, (sid, page, text, expected)) in pages[1..].iter().zip(&expected) {
        assert_eq!(*actual, (*sid, *page));
        let space = &document.spaces[sid];
        let view = &space.revisions[&space.contexts[&ExGuid::default()]];
        assert!(matches!(&view.nodes[text].kind, Kind::RichText { text, .. } if text == expected));
    }
    assert_eq!(server.publications, 24);
    if let Some(output) = std::env::var_os("ONESTORE_OFFLINE_PAGE_OUTPUT") {
        std::fs::create_dir(&output).unwrap();
        std::fs::write(
            std::path::Path::new(&output).join("pages.one"),
            &server.durable,
        )
        .unwrap();
    }
}

#[test]
fn uncertain_page_publication_retains_both_revisions_and_never_replays() {
    for fault in [
        Fault::UnknownBefore,
        Fault::UnknownAfter,
        Fault::PanicBefore,
        Fault::PanicAfter,
    ] {
        let source = onestore::create_section("pages.one", "Original", "Author").unwrap();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("pages.sqlite");
        let cache = Replica::create(&path, &source).unwrap();
        let page = PageCreation::new(None, Some("Created"), "Author").unwrap();
        let id = cache.create_page(&source, &page).unwrap().unwrap();
        let local = cache.snapshot().unwrap();
        let mut server = Server::new(&source);
        server.fault = fault;
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            cache.sync_once(&mut server)
        }));
        let attempted = cache.status(id).unwrap().unwrap();
        assert!(matches!(attempted, EditStatus::AwaitingConfirmation { .. }));
        let archive = directory.path().join("recovery.sqlite");
        cache.export_recovery(&archive).unwrap();
        assert_eq!(
            Recovery::open(&archive).unwrap().status(id).unwrap(),
            Some(attempted.clone())
        );
        drop(cache);
        let db = rusqlite::Connection::open(&path).unwrap();
        let encoded: String = db
            .query_row("SELECT revisions FROM attempt", [], |row| row.get(0))
            .unwrap();
        let revisions: BTreeMap<ExGuid, ExGuid> = serde_json::from_str(&encoded).unwrap();
        assert_eq!(revisions.len(), 2);
        assert!(revisions.contains_key(&page.space()));
        drop(db);
        let cache = Replica::open(&path).unwrap();
        let observed = server.visible.clone();
        let result = cache.sync_once(&mut server).unwrap().unwrap();
        let visible = matches!(fault, Fault::UnknownAfter | Fault::PanicAfter);
        assert_eq!(server.publications, 1);
        assert_eq!(server.confirmations, usize::from(visible));
        if visible {
            assert!(matches!(result.1, EditStatus::Published { .. }));
            assert!(cache.snapshot().unwrap() == observed);
            assert!(observed[..212] == server.durable[..212]);
            assert!(observed[252..] == server.durable[252..]);
        } else {
            assert_eq!(result, (id, attempted.clone()));
            assert_eq!(cache.snapshot().unwrap(), local);
        }
    }
}

#[test]
fn surviving_page_revision_alone_does_not_confirm_section_publication() {
    let source = onestore::create_section("pages.one", "Original", "Author").unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("pages.sqlite");
    let cache = Replica::create(&path, &source).unwrap();
    let page = PageCreation::new(None, None, "Author").unwrap();
    let id = cache.create_page(&source, &page).unwrap().unwrap();
    let mut server = Server::new(&source);
    server.fault = Fault::UnknownAfter;
    assert!(cache.sync_once(&mut server).is_err());
    let attempted = cache.status(id).unwrap().unwrap();
    drop(cache);
    let db = rusqlite::Connection::open(&path).unwrap();
    let encoded: String = db
        .query_row("SELECT revisions FROM attempt", [], |row| row.get(0))
        .unwrap();
    let revisions: BTreeMap<ExGuid, ExGuid> = serde_json::from_str(&encoded).unwrap();
    drop(db);
    let complete = server.visible.clone();
    let (&section, &retired) = revisions
        .iter()
        .find(|(sid, _)| **sid != page.space())
        .unwrap();
    let store = Store::parse(&complete).unwrap();
    let mut positions = Vec::new();
    for list in store.lists.values() {
        for node in &list.nodes {
            if node.id == 0x1e
                && node.payload[..16] == retired.guid
                && node.payload[16..20] == retired.n.to_le_bytes()
            {
                positions.push(node.offset);
            }
        }
    }
    assert_eq!(positions.len(), 1);
    // Replacing only the revision identity models maintenance retiring the section proof.
    let at = positions[0] + 4;
    server.visible[at] ^= 0x40;
    let store = Store::parse(&server.visible).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    index.validate_current().unwrap();
    assert!(!index.spaces[&section].revisions.contains_key(&retired));
    assert!(
        index.spaces[&page.space()]
            .revisions
            .contains_key(&revisions[&page.space()])
    );
    let cache = Replica::open(&path).unwrap();
    assert_eq!(
        cache.sync_once(&mut server).unwrap(),
        Some((id, attempted.clone()))
    );
    assert_eq!(server.confirmations, 0);
    assert_eq!(server.publications, 1);
    server.visible = complete;
    assert!(matches!(
        cache.sync_once(&mut server).unwrap(),
        Some((_, EditStatus::Published { .. }))
    ));
    assert_eq!(server.publications, 1);
}

#[test]
fn changed_page_anchor_requires_review_without_regenerating_dependent_identities() {
    let source = include_bytes!("../../../corpus/page-lifecycle/03-renamed/notebook/Lifecycle.one");
    let remote = include_bytes!("../../../corpus/page-lifecycle/04-nested/notebook/Lifecycle.one");
    let store = Store::parse(source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let pages = Document::parse(&index).unwrap().pages().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("pages.sqlite");
    let cache = Replica::create(&path, source).unwrap();
    let page = PageCreation::new(Some(pages[4].0), Some("Created"), "Author").unwrap();
    let id = cache.create_page(source, &page).unwrap().unwrap();
    let mut created = model_ops::page_of(&cache.snapshot().unwrap(), page.space());
    let body = body_outline(&mut created, "Retained body");
    cache
        .save(&cache.snapshot().unwrap(), page.space(), &created, "Author")
        .unwrap()
        .unwrap();
    let local = cache.snapshot().unwrap();
    let pending = cache.pending().unwrap();
    let mut server = Server::new(remote);
    assert_eq!(
        cache.sync_once(&mut server).unwrap(),
        Some((id, EditStatus::Conflict(ConflictKind::StructureChanged)))
    );
    assert!(
        cache
            .rebase_page_creation_conflict(id, source, remote, None)
            .is_err()
    );
    assert!(
        cache
            .rebase_page_creation_conflict(id, &local, source, None)
            .is_err()
    );
    cache
        .rebase_page_creation_conflict(id, &local, remote, None)
        .unwrap();
    assert_eq!(cache.snapshot().unwrap(), local);
    assert_eq!(cache.pending().unwrap()[1], pending[1]);
    let Operation::CreatePage(reviewed) = &cache.pending().unwrap()[0].operation else {
        panic!()
    };
    assert_eq!(*reviewed, page.reposition(None).unwrap());
    drop(cache);
    let cache = Replica::open(&path).unwrap();
    for _ in 0..2 {
        assert!(matches!(
            cache.sync_once(&mut server).unwrap(),
            Some((_, EditStatus::Published { .. }))
        ));
    }
    assert_eq!(server.publications, 2);
    let store = Store::parse(&server.durable).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let space = &document.spaces[&page.space()];
    let view = &space.revisions[&space.contexts[&ExGuid::default()]];
    assert!(matches!(&view.nodes[&body].kind,
        Kind::RichText { text, .. } if text == "Retained body"));
}
