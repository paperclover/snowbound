//! Outline layout through page-model saves: reconciliation against the remote page,
//! review of competing layout, twelve offline writers, and the native outline fixtures.

#[path = "../../onestore/tests/support/ops.rs"]
mod ops;

use notebook::{EditStatus, Replica};
use onestore::{
    ExGuid, RevisionIndex, Store,
    document::Document,
    page::{Outline, Page, PageObject, PageParagraph},
};
use serde_json::json;

#[path = "support/server.rs"]
mod server;
use server::{Fault, Server, conflicts, pages, snapshot};
#[path = "support/model_ops.rs"]
mod model_ops;
use model_ops::{
    delete_paragraph, insert_outline, outlines_mut, page_of, paragraph_with, replace_text, restyle,
    save,
};
#[path = "support/model_schedule.rs"]
mod model_schedule;
#[path = "../../onestore/tests/support/sweep.rs"]
mod sweep;
#[path = "sync/tree.rs"]
mod tree;

const BEFORE: &[u8] = include_bytes!("../../../corpus/outline-edit/before/notebook/synthetic.one");
const NATIVE: &[u8] = include_bytes!("../../../corpus/outline-edit/after/notebook/synthetic.one");

/// One model change, boxed so alternative cases can be listed together.
pub(crate) type Change<T> = Box<dyn Fn(&mut T)>;

/// A one-outline page with its space, outline and first paragraph's text identities.
pub(crate) fn fixture() -> (Vec<u8>, ExGuid, ExGuid, ExGuid) {
    let source = onestore::create_section("layout.one", "Original 🦀 é", "Author").unwrap();
    let store = Store::parse(&source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let space = document.pages().unwrap()[0].0;
    let page = Page::from_space(&document, space).unwrap();
    let outline = body_outlines(&page)[0];
    let ids = (outline.id, outline.paragraphs[0].text().unwrap().id);
    (source, space, ids.0, ids.1)
}

pub(crate) fn body_outlines(page: &Page) -> Vec<&Outline> {
    page.objects
        .iter()
        .filter_map(|object| match object {
            PageObject::Outline(outline) => Some(outline),
            _ => None,
        })
        .collect()
}

pub(crate) fn outline_of(page: &Page, id: ExGuid) -> &Outline {
    body_outlines(page)
        .into_iter()
        .find(|outline| outline.id == id)
        .expect("the outline is on the page")
}

pub(crate) fn outline_mut(page: &mut Page, id: ExGuid) -> &mut Outline {
    outlines_mut(page)
        .into_iter()
        .find(|outline| outline.id == id)
        .expect("the outline is on the page")
}

pub(crate) fn paragraph_mut(page: &mut Page, text: ExGuid) -> &mut PageParagraph {
    outlines_mut(page)
        .into_iter()
        .flat_map(|outline| outline.paragraphs.iter_mut())
        .find(|paragraph| paragraph.text().is_some_and(|value| value.id == text))
        .expect("the paragraph is on the page")
}

/// The text identity of the stored paragraph carrying exactly `value`.
pub(crate) fn text_with(page: &Page, value: &str) -> ExGuid {
    body_outlines(page)
        .into_iter()
        .flat_map(|outline| outline.paragraphs.iter())
        .find_map(|paragraph| {
            let text = paragraph.text()?;
            (text.text.text() == value).then_some(text.id)
        })
        .expect("a paragraph carries the text")
}

pub(crate) fn text_of(page: &Page, text: ExGuid) -> String {
    paragraph_with(page, text)
        .and_then(PageParagraph::text)
        .expect("the paragraph is on the page")
        .text
        .text()
        .to_owned()
}

/// A remote image holding `change`, published as another writer would.
pub(crate) fn remote_with(source: &[u8], space: ExGuid, change: impl FnOnce(&mut Page)) -> Vec<u8> {
    let mut page = page_of(source, space);
    change(&mut page);
    ops::saved(source, space, &page)
        .unwrap()
        .as_slice()
        .to_vec()
}

/// Runs one synchronization step, which publishes; true when the remote's version of
/// `space` stayed and the local one became a conflict page under it.
pub(crate) fn conflicted(cache: &Replica, server: &mut Server, space: ExGuid) -> bool {
    let count = |image: &[u8]| {
        conflicts(image)
            .into_iter()
            .filter(|(page, _)| *page == space)
            .map(|(_, pages)| pages.len())
            .sum::<usize>()
    };
    let before = count(&server.durable);
    assert!(matches!(
        cache.sync_once(server).unwrap().edit,
        Some((_, EditStatus::Published { .. }))
    ));
    count(&server.durable) > before
}

/// Publishes the batch holding `id`, unless an earlier step already did.
pub(crate) fn published(cache: &Replica, server: &mut Server, id: u64) {
    if !matches!(
        cache.status(id).unwrap(),
        Some(EditStatus::Published { .. })
    ) {
        assert!(matches!(
            cache.sync_once(server).unwrap().edit,
            Some((_, EditStatus::Published { .. }))
        ));
    }
    assert!(matches!(
        cache.status(id).unwrap(),
        Some(EditStatus::Published { .. })
    ));
}

fn moved(x: f32, y: f32) -> impl Fn(&mut Outline) {
    move |outline| {
        outline.layout.x = Some(x);
        outline.layout.y = Some(y);
    }
}

fn resized(points: f32, user_set: bool) -> impl Fn(&mut Outline) {
    move |outline| {
        outline.layout.max_width = Some(points);
        outline.layout.width_set_by_user = Some(user_set);
    }
}

#[test]
fn layout_and_dependent_text_survive_reopen_and_a_remote_format_of_the_same_page() {
    let (source, space, outline, text_id) = fixture();
    let changes: [Change<Page>; 3] = [
        Box::new(move |page| moved(180.0, 216.0)(outline_mut(page, outline))),
        Box::new(move |page| resized(144.0, true)(outline_mut(page, outline))),
        Box::new(move |page| paragraph_mut(page, text_id).collapsed = true),
    ];
    for change in changes {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("cache.sqlite");
        let cache = Replica::create(&path, &source).unwrap();
        let id = save(&cache, text_id, &change).unwrap().unwrap();
        let expected = page_of(&snapshot(&cache), space);
        let pending = cache.pending().unwrap();
        let local = snapshot(&cache);
        drop(cache);
        let cache = Replica::open(&path).unwrap();
        assert_eq!(cache.pending().unwrap(), pending);
        assert_eq!(pages(&snapshot(&cache)), pages(&local));
        let mut server = Server::new(&remote_with(&source, space, |page| {
            restyle(page, text_id, 0..8, |format| format.bold = Some(true));
        }));
        published(&cache, &mut server, id);
        let durable = page_of(&server.durable, space);
        assert_eq!(
            outline_of(&durable, outline).layout,
            outline_of(&expected, outline).layout
        );
        assert_eq!(
            paragraph_with(&durable, text_id).unwrap().collapsed,
            paragraph_with(&expected, text_id).unwrap().collapsed
        );
        assert!(cache.pending().unwrap().is_empty());

        let dependent = save(&cache, text_id, |page| {
            replace_text(page, text_id, 0..0, "Local ")
        })
        .unwrap()
        .unwrap();
        drop(cache);
        let cache = Replica::open(&path).unwrap();
        published(&cache, &mut server, dependent);
        let durable = page_of(&server.durable, space);
        assert_eq!(text_of(&durable, text_id), "Local Original 🦀 é");
        let text = &paragraph_with(&durable, text_id)
            .unwrap()
            .text()
            .unwrap()
            .text;
        assert_eq!(text.spans()[0].format.bold, Some(true));
        assert_eq!(
            outline_of(&durable, outline).layout,
            outline_of(&expected, outline).layout
        );
        assert_eq!(server.publications, 2);
        drop(cache);
        let cache = Replica::open(&path).unwrap();
        for id in [id, dependent] {
            assert!(matches!(
                cache.status(id).unwrap(),
                Some(EditStatus::Published { .. })
            ));
        }
    }
}

#[test]
fn an_uncertain_move_confirms_from_the_remote_revision_and_adopts_the_remote_title() {
    let (source, space, outline, _) = fixture();
    let mut page = page_of(&source, space);
    insert_outline(&mut page, 144.0, 36.0, "Second");
    let source = ops::saved(&source, space, &page)
        .unwrap()
        .as_slice()
        .to_vec();
    let second = text_with(&page_of(&source, space), "Second");
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("titles.sqlite");
    let cache = Replica::create(&path, &source).unwrap();
    let id = save(&cache, second, |page| {
        moved(216.0, 72.0)(outline_mut(page, outline));
    })
    .unwrap()
    .unwrap();
    drop(cache);
    let mut server = Server::new(&remote_with(&source, space, |page| {
        replace_text(page, second, 0..6, "Remote second 🐈");
    }));
    server.fault = Fault::UnknownAfter;
    let cache = Replica::open(&path).unwrap();
    assert!(cache.sync_once(&mut server).is_err());
    assert!(matches!(
        cache.status(id).unwrap(),
        Some(EditStatus::AwaitingConfirmation { .. })
    ));
    assert_eq!(page_of(&server.durable, space).title, "Original 🦀 é");
    assert_eq!(page_of(&server.visible, space).title, "Remote second 🐈");
    drop(cache);
    let cache = Replica::open(&path).unwrap();
    published(&cache, &mut server, id);
    assert_eq!(page_of(&server.durable, space).title, "Remote second 🐈");
    assert!(cache.pending().unwrap().is_empty());
    assert_eq!((server.publications, server.confirmations), (1, 1));
}

/// Competing layout keeps the remote's on the page and the local one on its conflict page.
#[test]
fn competing_layout_keeps_the_local_one_on_a_conflict_page() {
    let (source, space, outline, text_id) = fixture();
    let cases: [(Change<Outline>, Change<Outline>); 2] = [
        (Box::new(moved(180.0, 216.0)), Box::new(moved(288.0, 216.0))),
        (
            Box::new(resized(144.0, true)),
            Box::new(resized(216.0, false)),
        ),
    ];
    for (local_change, remote_change) in cases {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("cache.sqlite");
        let cache = Replica::create(&path, &source).unwrap();
        save(&cache, text_id, |page| {
            local_change(outline_mut(page, outline))
        })
        .unwrap()
        .unwrap();
        let local = page_of(&snapshot(&cache), space);
        let remote = remote_with(&source, space, |page| {
            remote_change(outline_mut(page, outline));
        });
        let mut server = Server::new(&remote);
        assert!(conflicted(&cache, &mut server, space));
        assert_eq!(server.publications, 1);
        let durable = page_of(&server.durable, space);
        assert_eq!(
            outline_of(&durable, outline).layout,
            outline_of(&page_of(&remote, space), outline).layout
        );
        let arena = onestore::Arena::default();
        let mut section = onestore::Section::open(&arena, server.durable.clone()).unwrap();
        let conflict = section.conflicts().unwrap()[0].1[0].space;
        let kept = section.page(conflict).unwrap();
        assert_eq!(
            body_outlines(&kept)[0].layout,
            outline_of(&local, outline).layout
        );
        let dependent = save(&cache, text_id, |page| {
            replace_text(page, text_id, 0..0, "Local ")
        })
        .unwrap()
        .unwrap();
        drop(cache);
        let cache = Replica::open(&path).unwrap();
        published(&cache, &mut server, dependent);
        assert_eq!(
            text_of(&page_of(&server.durable, space), text_id),
            "Local Original 🦀 é"
        );
    }
}

#[test]
fn a_local_move_and_a_remote_resize_of_one_outline_converge() {
    let (source, space, outline, text_id) = fixture();
    let directory = tempfile::tempdir().unwrap();
    let cache = Replica::create(directory.path().join("cache.sqlite"), &source).unwrap();
    let id = save(&cache, text_id, |page| {
        moved(180.0, 216.0)(outline_mut(page, outline))
    })
    .unwrap()
    .unwrap();
    let mut server = Server::new(&remote_with(&source, space, |page| {
        resized(252.0, true)(outline_mut(page, outline));
    }));
    published(&cache, &mut server, id);
    let layout = outline_of(&page_of(&server.durable, space), outline)
        .layout
        .clone();
    assert_eq!((layout.x, layout.y), (Some(180.0), Some(216.0)));
    assert_eq!(
        (layout.max_width, layout.width_set_by_user),
        (Some(252.0), Some(true))
    );
}

#[test]
fn twelve_offline_writers_preserve_all_layout_intents_through_review_and_restarts() {
    let (source, space, outline, text_id) = fixture();
    let directory = tempfile::tempdir().unwrap();
    let mut server = Server::new(&source);
    let mut reviewed = 0;
    for round in 0..4 {
        for actor in 0..12 {
            let path = directory.path().join(format!("{actor}.sqlite"));
            let cache = if round == 0 {
                Replica::create(&path, &source).unwrap()
            } else {
                Replica::open(&path).unwrap()
            };
            let change = |page: &mut Page| {
                let layout = &mut outline_mut(page, outline).layout;
                layout.x = Some((actor + 3) as f32 * 36.0);
                layout.y = Some((round + 3) as f32 * 36.0);
                layout.max_width = Some((actor + round + 3) as f32 * 36.0);
                layout.width_set_by_user = Some(true);
                paragraph_mut(page, text_id).collapsed = (actor + round) % 2 != 0;
            };
            let id = save(&cache, text_id, change).unwrap().unwrap();
            let expected = page_of(&snapshot(&cache), space);
            drop(cache);
            let cache = Replica::open(&path).unwrap();
            if conflicted(&cache, &mut server, space) {
                // The writer makes the change again over the remote's version.
                let again = save(&cache, text_id, change).unwrap().unwrap();
                reviewed += 1;
                published(&cache, &mut server, again);
            }
            assert!(matches!(
                cache.status(id).unwrap(),
                Some(EditStatus::Published { .. })
            ));
            assert!(cache.pending().unwrap().is_empty());
            let durable = page_of(&server.durable, space);
            assert_eq!(
                outline_of(&durable, outline).layout,
                outline_of(&expected, outline).layout
            );
            assert_eq!(
                paragraph_with(&durable, text_id).unwrap().collapsed,
                (actor + round) % 2 != 0
            );
            assert_eq!(text_of(&durable, text_id), "Original 🦀 é");
        }
    }
    assert_eq!(reviewed, 47);
}

#[test]
fn an_uncertain_layout_attempt_confirms_by_revision_or_by_an_equal_remote_page() {
    let (source, space, outline, text_id) = fixture();
    for fault in [Fault::UnknownBefore, Fault::UnknownAfter] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("cache.sqlite");
        let cache = Replica::create(&path, &source).unwrap();
        let id = save(&cache, text_id, |page| {
            resized(144.0, true)(outline_mut(page, outline))
        })
        .unwrap()
        .unwrap();
        let local = snapshot(&cache);
        let mut server = Server::new(&source);
        server.fault = fault;
        assert!(cache.sync_once(&mut server).is_err());
        let status = cache.status(id).unwrap().unwrap();
        assert!(matches!(status, EditStatus::AwaitingConfirmation { .. }));
        drop(cache);
        let cache = Replica::open(&path).unwrap();
        if matches!(fault, Fault::UnknownAfter) {
            published(&cache, &mut server, id);
            let layout = outline_of(&page_of(&server.durable, space), outline)
                .layout
                .clone();
            assert_eq!(layout.max_width, Some(144.0));
            assert_eq!((server.publications, server.confirmations), (1, 1));
            continue;
        }
        assert_eq!(
            cache.sync_once(&mut server).unwrap().edit,
            Some((id, status.clone()))
        );
        server.visible = remote_with(&source, space, |page| {
            resized(216.0, true)(outline_mut(page, outline));
        });
        assert_eq!(
            cache.sync_once(&mut server).unwrap().edit,
            Some((id, status.clone()))
        );
        assert_eq!(pages(&snapshot(&cache)), pages(&local));
        server.visible = remote_with(&source, space, |page| {
            resized(144.0, true)(outline_mut(page, outline));
        });
        published(&cache, &mut server, id);
        assert_eq!((server.publications, server.confirmations), (1, 1));
    }
}

/// The layout or collapse change the native fixture page is reconciled against.
fn native_case(name: &str, outline: ExGuid, target: ExGuid) -> Change<Page> {
    let layout: Option<Change<Outline>> = match name {
        "Move outline" => Some(Box::new(moved(252.0, 288.0))),
        "Resize outline" | "Automatic outline size" | "Delete outline" => {
            Some(Box::new(resized(252.0, true)))
        }
        _ => None,
    };
    let collapsed = name != "Expand subtree";
    Box::new(move |page| match &layout {
        Some(change) => {
            if body_outlines(page).iter().any(|o| o.id == outline) {
                change(outline_mut(page, outline));
            }
        }
        None => {
            if paragraph_with(page, target).is_some() {
                paragraph_mut(page, target).collapsed = collapsed;
            }
        }
    })
}

/// Native fixture pages with two body outlines: the page, its first outline, the
/// "Target " paragraph's text and an independent paragraph's text in the second outline.
pub(crate) fn native_pages(source: &[u8]) -> Vec<(String, ExGuid, ExGuid, ExGuid, ExGuid)> {
    let store = Store::parse(source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let mut cases = Vec::new();
    for (space, _) in document.pages().unwrap() {
        let page = Page::from_space(&document, space).unwrap();
        let outlines = body_outlines(&page);
        if outlines.len() != 2 || page.title == "Unicode rich text" {
            continue;
        }
        let target = outlines[0]
            .paragraphs
            .iter()
            .find(|paragraph| {
                paragraph
                    .text()
                    .is_some_and(|text| text.text.text().starts_with("Target "))
            })
            .expect("each fixture page marks its target paragraph");
        cases.push((
            page.title.clone(),
            space,
            outlines[0].id,
            target.text().unwrap().id,
            outlines[1].paragraphs[0].text().unwrap().id,
        ));
    }
    cases
}

#[test]
fn native_moves_deletions_and_layout_changes_merge_or_keep_a_conflict_page() {
    let mut server = Server::new(NATIVE);
    let mut records = Vec::new();
    for (name, space, outline, target, dependent) in native_pages(BEFORE) {
        let change = native_case(&name, outline, target);
        let save_both = |page: &mut Page| {
            change(page);
            replace_text(page, dependent, 0..0, "Local ");
        };
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("cache.sqlite");
        let cache = Replica::create(&path, BEFORE).unwrap();
        let id = save(&cache, dependent, save_both).unwrap().unwrap();
        let merged = !conflicted(&cache, &mut server, space);
        assert!(matches!(
            cache.status(id).unwrap(),
            Some(EditStatus::Published { .. })
        ));
        if !merged {
            // The dependent text merged; the writer makes the change again.
            drop(cache);
            let cache = Replica::open(&path).unwrap();
            // Unless the remote made the same change.
            if let Some(again) = save(&cache, dependent, &change).unwrap() {
                published(&cache, &mut server, again);
            }
        }
        let durable = page_of(&server.durable, space);
        assert!(
            text_of(&durable, dependent).starts_with("Local "),
            "{name} keeps its dependent text"
        );
        let mut reapplied = durable.clone();
        change(&mut reapplied);
        assert_eq!(reapplied, durable, "{name} keeps its reconciled change");
        let (object, change) = match name.as_str() {
            "Move outline" => (outline, json!({"Position": {"x": 252.0, "y": 288.0}})),
            "Resize outline" | "Automatic outline size" | "Delete outline" => (
                outline,
                json!({"Width": {"points": 252.0, "user_set": true}}),
            ),
            _ => (
                paragraph_with(&page_of(BEFORE, space), target).unwrap().id,
                json!({"Collapsed": name != "Expand subtree"}),
            ),
        };
        records.push(json!({
            "name": name,
            "outcome": if merged { "merged" } else { "reviewed" },
            "object": object.to_string(),
            "change": change,
            "dependent_text": dependent.to_string(),
        }));
    }
    let reviewed = records
        .iter()
        .filter(|record| record["outcome"] == "reviewed")
        .count();
    assert_eq!((records.len(), reviewed), (14, 7), "{records:?}");
    if let Some(output) = std::env::var_os("ONESTORE_OFFLINE_OUTLINE_OUTPUT") {
        let output = std::path::PathBuf::from(output);
        assert!(output.is_absolute());
        std::fs::create_dir_all(output.join("candidate")).unwrap();
        std::fs::write(output.join("candidate/synthetic.one"), &server.durable).unwrap();
        std::fs::write(
            output.join("cases.json"),
            serde_json::to_vec_pretty(&records).unwrap(),
        )
        .unwrap();
    }
}

#[test]
fn a_new_native_wrap_reservation_keeps_the_local_width_on_a_conflict_page() {
    let (_, space, outline, _, text_id) = native_pages(BEFORE)
        .into_iter()
        .find(|(name, ..)| name == "Move outline")
        .unwrap();
    let stored = outline_of(&page_of(BEFORE, space), outline).layout.clone();
    let remote = outline_of(&page_of(NATIVE, space), outline).layout.clone();
    assert_eq!(
        (stored.max_width, stored.width_set_by_user),
        (remote.max_width, remote.width_set_by_user)
    );
    assert!(remote.reserved_width.is_some());
    let directory = tempfile::tempdir().unwrap();
    let cache = Replica::create(directory.path().join("cache.sqlite"), BEFORE).unwrap();
    let id = save(&cache, text_id, |page| {
        resized(144.0, true)(outline_mut(page, outline))
    })
    .unwrap()
    .unwrap();
    let mut server = Server::new(NATIVE);
    assert!(conflicted(&cache, &mut server, space));
    assert!(matches!(
        cache.status(id).unwrap(),
        Some(EditStatus::Published { .. })
    ));
    let again = save(&cache, text_id, |page| {
        resized(144.0, true)(outline_mut(page, outline))
    })
    .unwrap()
    .unwrap();
    published(&cache, &mut server, again);
    let layout = outline_of(&page_of(&server.durable, space), outline)
        .layout
        .clone();
    assert_eq!((layout.x, layout.y), (remote.x, remote.y));
    assert_eq!(layout.max_width, Some(144.0));
    assert!(layout.reserved_width.is_none());
}
