//! Page creation reconciliation: dependent body saves, uncertain publication, lost anchors,
//! and deterministic multi-actor schedules.

#[path = "../../onestore/tests/support/ops.rs"]
mod ops;

use notebook::{EditStatus, Recovery, Replica};
use onestore::op::SectionOp;
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
#[path = "../../onestore/tests/support/sweep.rs"]
mod sweep;

#[test]
fn twelve_replica_page_schedules_retain_acknowledged_pages_through_interruptions() {
    for seed in sweep::seeds(0..24, 1) {
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
        let id = section_op(&cache, SectionOp::Create(page.clone()));
        let mut created = cache.page(page.space()).unwrap();
        let body = body_outline(&mut created, "Body");
        let save = model_ops::save_as(&cache, page.space(), &created, model_ops::AUTHOR)
            .unwrap()
            .unwrap();
        let dependent = model_ops::save(&cache, body, |page| {
            model_ops::replace_text(page, body, 4..4, &format!(" {actor}"))
        })
        .unwrap()
        .unwrap();
        assert!(id < save && save < dependent);
        let original = cache.pending().unwrap();
        assert_eq!(original.len(), 3);
        assert!(
            matches!(&original[0].edit.ops[..], [onestore::op::Op::Section(SectionOp::Create(retained))] if retained == &page)
        );
        let local = snapshot(&cache);
        drop(cache);
        let cache = Replica::open(&path).unwrap();
        assert_eq!(cache.pending().unwrap(), original);
        assert_eq!(pages(&snapshot(&cache)), pages(&local));
        assert!(matches!(
            cache.sync_once(&mut server).unwrap().edit,
            Some((published, EditStatus::Published { .. })) if published == dependent
        ));
        for edit in original {
            assert!(matches!(
                cache.status(edit.id).unwrap(),
                Some(EditStatus::Published { .. })
            ));
        }
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
    assert_eq!(server.publications, 12);
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
        let id = section_op(&cache, SectionOp::Create(page.clone()));
        let local = snapshot(&cache);
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
            .query_row(
                "SELECT revisions FROM batches WHERE attempted=1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let revisions: BTreeMap<ExGuid, ExGuid> = serde_json::from_str(&encoded).unwrap();
        assert_eq!(revisions.len(), 2);
        assert!(revisions.contains_key(&page.space()));
        drop(db);
        let cache = Replica::open(&path).unwrap();
        let observed = server.visible.clone();
        let result = cache.sync_once(&mut server).unwrap().edit.unwrap();
        let visible = matches!(fault, Fault::UnknownAfter | Fault::PanicAfter);
        assert_eq!(server.publications, 1);
        assert_eq!(server.confirmations, usize::from(visible));
        if visible {
            assert!(matches!(result.1, EditStatus::Published { .. }));
            assert!(snapshot(&cache) == observed);
            assert!(observed[..212] == server.durable[..212]);
            assert!(observed[252..] == server.durable[252..]);
        } else {
            assert_eq!(result, (id, attempted.clone()));
            assert_eq!(pages(&snapshot(&cache)), pages(&local));
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
    let id = section_op(&cache, SectionOp::Create(page.clone()));
    let mut server = Server::new(&source);
    server.fault = Fault::UnknownAfter;
    assert!(cache.sync_once(&mut server).is_err());
    let attempted = cache.status(id).unwrap().unwrap();
    drop(cache);
    let db = rusqlite::Connection::open(&path).unwrap();
    let encoded: String = db
        .query_row(
            "SELECT revisions FROM batches WHERE attempted=1",
            [],
            |row| row.get(0),
        )
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
    // A rewrite publishes a new file version (MS-ONESTORE 2.3.1).
    server.visible[212] ^= 1;
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
        cache.sync_once(&mut server).unwrap().edit,
        Some((id, attempted.clone()))
    );
    assert_eq!(server.confirmations, 0);
    assert_eq!(server.publications, 1);
    server.visible = complete;
    assert!(matches!(
        cache.sync_once(&mut server).unwrap().edit,
        Some((_, EditStatus::Published { .. }))
    ));
    assert_eq!(server.publications, 1);
}

/// A page created before a page the remote moved goes before the next page the remote left
/// in place, its content under the identities it was made with.
#[test]
fn a_moved_page_anchor_places_the_page_without_regenerating_dependent_identities() {
    let source = include_bytes!("../../../corpus/page-lifecycle/03-renamed/notebook/Lifecycle.one");
    let remote = include_bytes!("../../../corpus/page-lifecycle/04-nested/notebook/Lifecycle.one");
    let store = Store::parse(source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let pages = Document::parse(&index).unwrap().pages().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("pages.sqlite");
    let cache = Replica::create(&path, source).unwrap();
    let page = PageCreation::new(Some(pages[4].0), Some("Created"), "Author").unwrap();
    let id = section_op(&cache, SectionOp::Create(page.clone()));
    let mut created = cache.page(page.space()).unwrap();
    let body = body_outline(&mut created, "Retained body");
    let save = model_ops::save_as(&cache, page.space(), &created, "Author")
        .unwrap()
        .unwrap();
    let local = snapshot(&cache);
    let mut server = Server::new(remote);
    assert!(matches!(
        cache.sync_once(&mut server).unwrap().edit,
        Some((published, EditStatus::Published { .. })) if published == save
    ));
    assert!(matches!(
        cache.status(id).unwrap(),
        Some(EditStatus::Published { .. })
    ));
    assert_eq!(server.publications, 1);
    assert_eq!(
        model_ops::page_of(&server.durable, page.space()),
        model_ops::page_of(&local, page.space())
    );
    let arena = onestore::Arena::default();
    let listed = onestore::Section::open(&arena, server.durable.clone())
        .unwrap()
        .pages()
        .unwrap();
    // The remote made `child` and `grandchild` subpages; `Renamed` stayed.
    let at = listed.iter().position(|listed| listed.0 == pages[6].0).unwrap();
    assert_eq!(listed[at - 1].0, page.space());
    let store = Store::parse(&server.durable).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let space = &document.spaces[&page.space()];
    let view = &space.revisions[&space.contexts[&ExGuid::default()]];
    assert!(matches!(&view.nodes[&body].kind,
        Kind::RichText { text, .. } if text == "Retained body"));
}

/// Edits of a page the remote removed put the local version back where it stood.
#[test]
fn a_page_the_remote_removed_comes_back_where_it_stood() {
    let source = include_bytes!("../../../corpus/page-lifecycle/04-nested/notebook/Lifecycle.one");
    let order = |image: &[u8]| -> Vec<(ExGuid, String, u32)> {
        let arena = onestore::Arena::default();
        onestore::Section::open(&arena, image.to_vec())
            .unwrap()
            .pages()
            .unwrap()
    };
    let pages = order(source);
    // A subpage without subpages of its own and with a page after it, so it has a position
    // and a level to return to.
    let at = (0..pages.len() - 1)
        .find(|at| {
            pages[*at].2 > 1
                && pages[*at + 1].2 <= pages[*at].2
                && model_ops::page_of(source, pages[*at].0)
                    .objects
                    .iter()
                    .any(|object| matches!(object, onestore::page::PageObject::Outline(_)))
        })
        .expect("a subpage with body text");
    let (space, _, level) = pages[at].clone();
    let text = model_ops::page_of(source, space)
        .objects
        .iter()
        .find_map(|object| match object {
            onestore::page::PageObject::Outline(outline) => {
                outline.paragraphs[0].text().map(|t| t.id)
            }
            _ => None,
        })
        .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let cache = Replica::create(directory.path().join("pages.sqlite"), source).unwrap();
    model_ops::save(&cache, text, |page| {
        model_ops::replace_text(page, text, 0..0, "Kept ")
    })
    .unwrap()
    .unwrap();
    let local = cache.page(space).unwrap();
    let removed = ops::section_op(source, SectionOp::Delete([space].to_vec()))
        .unwrap()
        .as_bytes()
        .to_vec();
    let mut server = Server::new(&removed);
    assert!(matches!(
        cache.sync_once(&mut server).unwrap().edit,
        Some((_, EditStatus::Published { .. }))
    ));
    assert_eq!(
        server.publications, 1,
        "a removed page is not edited in place"
    );
    let published = order(&server.durable);
    assert!(published.iter().all(|(page, ..)| *page != space));
    assert_eq!(published.len(), pages.len());
    let (restored, title, restored_level) = published[at].clone();
    assert_eq!(
        (title.as_str(), restored_level),
        (pages[at].1.as_str(), level)
    );
    let page = model_ops::page_of(&server.durable, restored);
    let texts = |page: &onestore::page::Page| -> Vec<String> {
        page.objects
            .iter()
            .filter_map(|object| match object {
                onestore::page::PageObject::Outline(outline) => Some(outline),
                _ => None,
            })
            .flat_map(|outline| &outline.paragraphs)
            .filter_map(|p| p.text().map(|t| t.text.text().to_owned()))
            .collect()
    };
    assert_eq!(texts(&page), texts(&local));
    assert!(texts(&page)[0].starts_with("Kept "));
    let others: Vec<_> = published
        .iter()
        .map(|(page, ..)| *page)
        .filter(|page| *page != restored)
        .collect();
    let before: Vec<_> = pages
        .iter()
        .map(|(page, ..)| *page)
        .filter(|page| *page != space)
        .collect();
    assert_eq!(others, before);
    assert_eq!(pages_of(&cache), published);
}

fn pages_of(cache: &Replica) -> Vec<(ExGuid, String, u32)> {
    cache.pages().unwrap()
}
