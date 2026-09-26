//! Paragraph splits and joins expressed as page-model differences: reconciliation against
//! native OneNote captures, dependent edits, uncertain attempts and competing clients.

use notebook::{ConflictKind, EditStatus, Replica};
use onestore::{
    ExGuid, PreparedEdit, RevisionIndex, Store,
    document::Document,
    page::{Page, PageObject, PageParagraph, ParagraphContent, TextObject, text::new_id},
};
use std::path::Path;

#[path = "support/server.rs"]
mod server;
use server::{Fault, Server, pages};
#[path = "support/model_ops.rs"]
mod model_ops;
use model_ops::{AUTHOR, outlines_mut, page_of, replace_text};

const BEFORE: &[u8] =
    include_bytes!("../../../corpus/paragraph-edit/reconciliation/before/notebook/synthetic.one");
const KEYBOARD: &[u8] =
    include_bytes!("../../../corpus/paragraph-edit/reconciliation/keyboard/notebook/synthetic.one");
const REPLACED: &[u8] =
    include_bytes!("../../../corpus/paragraph-edit/reconciliation/remote/notebook/synthetic.one");
const CORPUS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus/paragraph-edit");

fn at(list: &[PageParagraph], text: ExGuid) -> Option<usize> {
    list.iter()
        .position(|p| p.text().is_some_and(|t| t.id == text))
}

/// Applies `edit` to the paragraph list holding `text`, descending into table cells.
fn in_list<T>(
    list: &mut Vec<PageParagraph>,
    text: ExGuid,
    edit: &mut impl FnMut(&mut Vec<PageParagraph>, usize) -> T,
) -> Option<T> {
    if let Some(index) = at(list, text) {
        return Some(edit(list, index));
    }
    for paragraph in list {
        if let ParagraphContent::Table(table) = &mut paragraph.content {
            for row in &mut table.rows {
                for cell in &mut row.cells {
                    if let Some(value) = in_list(&mut cell.paragraphs, text, edit) {
                        return Some(value);
                    }
                }
            }
        }
    }
    None
}

fn locate_in<T>(
    page: &mut Page,
    text: ExGuid,
    mut edit: impl FnMut(&mut Vec<PageParagraph>, usize) -> T,
) -> T {
    for outline in outlines_mut(page) {
        if let Some(value) = in_list(&mut outline.paragraphs, text, &mut edit) {
            return value;
        }
    }
    panic!("the edited paragraph is on the page");
}

/// Splits the paragraph carrying `text` at a UTF-16 offset: the text tail and the children
/// move to a new following paragraph, as pressing Enter mid-paragraph does.
fn split_paragraph(page: &mut Page, text: ExGuid, offset: u32) {
    locate_in(page, text, |list, index| {
        // One identity per split, numbered as OneNote numbers the paragraph it creates.
        // A GUID of the split's own: `new_id` shares its GUID across identities.
        let ExGuid { mut guid, n } = new_id().unwrap();
        guid[14] ^= n as u8;
        guid[15] ^= 0xff;
        let mut right = list[index].clone();
        right.id = ExGuid { guid, n: 1 };
        right.tags.clear();
        let source = list[index].text().unwrap().text.clone();
        let end = source.utf16_offset(source.text().len()).unwrap();
        list[index].text_mut().unwrap().text = source.slice(0..offset).unwrap();
        right.content = ParagraphContent::Text(TextObject {
            id: ExGuid { guid, n: 2 },
            date_field: None,
            text: source.slice(offset..end).unwrap(),
            tags: Vec::new(),
        });
        let (left, tail) = (list[index].id, right.id);
        for paragraph in &mut list[index + 1..] {
            if paragraph.parent == Some(left) {
                paragraph.parent = Some(tail);
            }
        }
        list.insert(index + 1, right);
    })
}

/// Joins the paragraph carrying `right` into the one carrying `left`; an empty left text
/// object is replaced by a non-empty right one, which is the identity OneNote keeps. The
/// right paragraph's children keep their depth, under the left paragraph or the nearest of
/// its ancestors above them.
fn join_paragraphs(page: &mut Page, left: ExGuid, right: ExGuid) {
    locate_in(page, left, |list, first| {
        let second = at(list, right).expect("the joined paragraphs share a container");
        let (parent, orphaned, removed) = (
            list[first].id,
            list[second].id,
            list[second].text().unwrap().clone(),
        );
        let adopts = !removed.text.text().is_empty();
        let target = list[first].text_mut().unwrap();
        let mut joined = target.text.clone();
        joined.append(removed.text).unwrap();
        if adopts && target.text.text().is_empty() {
            target.id = removed.id;
        }
        target.text = joined;
        list.remove(second);
        let nesting: std::collections::BTreeMap<_, _> =
            list.iter().map(|p| (p.id, (p.level, p.parent))).collect();
        for paragraph in list {
            if paragraph.parent == Some(orphaned) {
                let mut adopter = Some(parent);
                while let Some(id) = adopter
                    && nesting[&id].0 >= paragraph.level
                {
                    adopter = nesting[&id].1;
                }
                paragraph.parent = adopter;
            }
        }
    });
}

/// The paragraphs of a page's single body outline.
fn body(page: &Page) -> &[PageParagraph] {
    page.objects
        .iter()
        .find_map(|object| match object {
            PageObject::Outline(outline) => Some(&outline.paragraphs[..]),
            _ => None,
        })
        .unwrap()
}

fn texts(page: &Page) -> Vec<String> {
    body(page)
        .iter()
        .filter_map(|p| p.text().map(|t| t.text.text().to_owned()))
        .collect()
}

/// The page space and model holding a text object anywhere on a page, table cells included.
fn holder(bytes: &[u8], text: ExGuid) -> (ExGuid, Page) {
    let store = Store::parse(bytes).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    document
        .pages()
        .unwrap()
        .into_iter()
        .find_map(|(space, _)| {
            let mut page = Page::from_space(&document, space).ok()?;
            let found = outlines_mut(&mut page)
                .into_iter()
                .any(|outline| in_list(&mut outline.paragraphs, text, &mut |_, _| ()).is_some());
            found.then_some((space, page))
        })
        .expect("text object belongs to an active page")
}

fn save_page(
    cache: &Replica,
    text: ExGuid,
    edit: impl FnOnce(&mut Page),
) -> Result<Option<u64>, notebook::Error> {
    let source = cache.snapshot()?;
    let (space, mut page) = holder(&source, text);
    edit(&mut page);
    model_ops::save_as(cache, space, &page, AUTHOR)
}

/// Resolves a conflict by taking the remote page, then saves `edit` of it.
fn review_page(cache: &Replica, id: u64, text: ExGuid, edit: impl FnOnce(&mut Page)) {
    cache.resolve(id, notebook::Resolution::Theirs).unwrap();
    save_page(cache, text, edit).unwrap().unwrap();
}

/// The styled text of an object anywhere on a page.
fn content(page: &Page, text: ExGuid) -> onestore::page::Paragraph {
    locate_in(&mut page.clone(), text, |list, index| {
        list[index].text().unwrap().text.clone()
    })
}

/// Rewrites one character of a text object, the dependent edit the native lane checks.
fn dependent_edit(page: &mut Page, text: ExGuid) {
    let content = content(page, text);
    match content.text().find('c') {
        Some(byte) => {
            let start = content.utf16_offset(byte).unwrap();
            replace_text(page, text, start..start + 1, "C");
        }
        None => replace_text(page, text, 1..2, "I"),
    }
}

/// The reconciliation fixture's split and join controls, in page order.
fn controls() -> Vec<(ExGuid, String, bool)> {
    let store = Store::parse(BEFORE).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    document
        .pages()
        .unwrap()
        .into_iter()
        .filter_map(|(space, _)| {
            let title = Page::from_space(&document, space).unwrap().title;
            let split = title.starts_with("Split ");
            (split || title.starts_with("Join ")).then_some((space, title, split))
        })
        .collect()
}

/// Publishes every control's split or join against the native keyboard capture, reviewing
/// whatever conflicts, then a dependent edit of the resulting paragraph. Returns the
/// reconciled remote image and one manifest entry per control.
fn reconcile_controls() -> (Vec<u8>, Vec<serde_json::Value>) {
    let mut server = Server::new(KEYBOARD);
    let mut recorded = Vec::new();
    let mut automatic = 0;
    for (space, name, split) in controls() {
        let original = page_of(BEFORE, space);
        let left = body(&original)[0].text().unwrap().id;
        let right = (!split).then(|| body(&original)[1].text().unwrap().id);
        let apply = |page: &mut Page| match right {
            Some(right) => join_paragraphs(page, left, right),
            None => split_paragraph(page, left, 2),
        };
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("reconciliation.sqlite");
        let mut cache = Replica::create(&path, BEFORE).unwrap();
        let mut after = original.clone();
        apply(&mut after);
        let id = model_ops::save_as(&cache, space, &after, AUTHOR)
            .unwrap()
            .unwrap();
        let pending = cache.pending().unwrap();
        drop(cache);
        cache = Replica::open(&path).unwrap();
        assert_eq!(cache.pending().unwrap(), pending);
        let (actual, status) = cache.sync_once(&mut server).unwrap().edit.unwrap();
        assert_eq!(actual, id);
        let reviewed = matches!(status, EditStatus::Conflict(_));
        let receipt = if reviewed {
            assert_eq!(cache.pending().unwrap(), pending);
            review_page(&cache, id, left, apply);
            cache.sync_once(&mut server).unwrap().edit.unwrap().1
        } else {
            automatic += 1;
            status
        };
        let EditStatus::Published { revision } = receipt else {
            panic!("{name}: {receipt:?}")
        };
        let published = page_of(&server.visible, space);
        let index = body(&published)
            .iter()
            .position(|p| {
                p.text()
                    .is_some_and(|t| t.id == left || Some(t.id) == right)
            })
            .unwrap();
        let intent = match right {
            Some(right) => serde_json::json!({"author": AUTHOR, "left": left, "right": right}),
            None => {
                // The native lane derives both new identities from the guid recorded here.
                let tail = &body(&published)[index + 1];
                let guid = tail.id.guid;
                assert_eq!(tail.id, ExGuid { guid, n: 1 }, "{name}");
                assert_eq!(tail.text().unwrap().id, ExGuid { guid, n: 2 }, "{name}");
                serde_json::json!({"author": AUTHOR, "offset": 2, "text": left, "guid": guid})
            }
        };
        let target = body(&published)[if split { index + 1 } else { index }]
            .text()
            .unwrap()
            .id;
        drop(cache);
        cache = Replica::open(&path).unwrap();
        let dependent = save_page(&cache, target, |page| dependent_edit(page, target))
            .unwrap()
            .unwrap();
        let (actual, status) = cache.sync_once(&mut server).unwrap().edit.unwrap();
        assert_eq!(actual, dependent);
        let EditStatus::Published {
            revision: dependent_revision,
        } = status
        else {
            panic!("{name} dependent: {status:?}")
        };
        assert!(cache.pending().unwrap().is_empty());
        assert_eq!(cache.snapshot().unwrap(), server.durable);
        recorded.push(
            serde_json::json!({"case": name, "space": space, "intent": intent,
            "outcome": if reviewed { "reviewed" } else { "automatic" },
            "receipt": revision, "dependent_receipt": dependent_revision}),
        );
    }
    assert_eq!((recorded.len(), automatic), (16, 8));
    assert_eq!(server.publications, 32);
    (server.durable, recorded)
}

#[test]
fn native_keyboard_edits_merge_with_offline_splits_and_joins_or_survive_review() {
    let (durable, cases) = reconcile_controls();
    let expected = [
        ("Split remote prefix", "automatic", vec!["Xab", "🦀Cd"]),
        ("Split remote boundary", "automatic", vec!["abX", "🦀Cd"]),
        ("Split remote child", "reviewed", vec!["ab", "🦀Cd"]),
        // A split names copies of its paragraph's list nodes; the remote's new list has none.
        ("Split remote list", "reviewed", vec!["ab", "🦀Cd"]),
        ("Split remote tag", "automatic", vec!["ab", "🦀Cd"]),
        ("Split remote format", "reviewed", vec!["ab", "🦀Cd"]),
        ("Split remote sibling", "automatic", vec!["ab", "🦀Cd"]),
        ("Join remote prefix", "reviewed", vec!["Xab🦀CdRightY"]),
        ("Join left boundary", "automatic", vec!["ab🦀CdXRight"]),
        ("Join right boundary", "reviewed", vec!["ab🦀CdXRight"]),
        ("Join remote child", "automatic", vec!["ab🦀CdRight"]),
        ("Join remote list", "reviewed", vec!["ab🦀CdRight"]),
        ("Join remote tag", "reviewed", vec!["ab🦀CdRight"]),
        ("Join remote format", "reviewed", vec!["ab🦀CdRight"]),
        ("Join remote sibling", "automatic", vec!["ab🦀CdRight"]),
        ("Join empty adoption", "automatic", vec!["XIight"]),
    ];
    for (case, (name, outcome, prefix)) in cases.iter().zip(expected) {
        assert_eq!(case["case"], name);
        assert_eq!(case["outcome"], outcome, "{name}");
        let space: ExGuid = serde_json::from_value(case["space"].clone()).unwrap();
        let published = texts(&page_of(&durable, space));
        assert_eq!(&published[..prefix.len()], prefix, "{name}");
        assert!(
            published.iter().any(|text| text.contains("sibling")),
            "{name}: {published:?}"
        );
    }
}

#[test]
fn a_native_text_object_replacement_conflicts_with_every_offline_split_and_join() {
    let mut server = Server::new(REPLACED);
    for (space, name, split) in controls() {
        let original = page_of(BEFORE, space);
        let left = body(&original)[0].text().unwrap().id;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("replaced.sqlite");
        let cache = Replica::create(&path, BEFORE).unwrap();
        let mut after = original.clone();
        if split {
            split_paragraph(&mut after, left, 2);
        } else {
            join_paragraphs(&mut after, left, body(&original)[1].text().unwrap().id);
        }
        let id = model_ops::save_as(&cache, space, &after, AUTHOR)
            .unwrap()
            .unwrap();
        let local = cache.snapshot().unwrap();
        let pending = cache.pending().unwrap();
        assert_eq!(
            cache.sync_once(&mut server).unwrap().edit,
            Some((id, EditStatus::Conflict(ConflictKind::ContentChanged))),
            "{name}"
        );
        drop(cache);
        let cache = Replica::open(&path).unwrap();
        assert_eq!(pages(&cache.snapshot().unwrap()), pages(&local));
        assert_eq!(cache.pending().unwrap(), pending);
        assert_eq!(
            cache.status(id).unwrap(),
            Some(EditStatus::Conflict(ConflictKind::ContentChanged))
        );
    }
    assert_eq!(server.publications, 0);
    assert_eq!(server.visible, REPLACED);
}

/// Replays a recorded native split or join intent as a page-model difference.
fn replay(page: &mut Page, intent: &serde_json::Value, split: bool) {
    if split {
        split_paragraph(
            page,
            serde_json::from_value(intent["text"].clone()).unwrap(),
            u32::try_from(intent["offset"].as_u64().unwrap()).unwrap(),
        );
    } else {
        join_paragraphs(
            page,
            serde_json::from_value(intent["left"].clone()).unwrap(),
            serde_json::from_value(intent["right"].clone()).unwrap(),
        );
    }
}

/// The text object a recorded intent starts from.
fn origin(intent: &serde_json::Value, split: bool) -> ExGuid {
    serde_json::from_value(intent[if split { "text" } else { "left" }].clone()).unwrap()
}

/// Replays every recorded native split and join intent as a page-model save, against a
/// remote that prefixed the edited text wherever a prefix is distinguishable.
fn offline_paragraphs(output: Option<&Path>) {
    let root = Path::new(CORPUS);
    for (name, source, recorded, split) in [
        (
            "splits",
            "before/notebook/synthetic.one",
            "rust-split/manifest.json",
            true,
        ),
        (
            "joins",
            "split/notebook/synthetic.one",
            "rust-join/split/manifest.json",
            false,
        ),
        (
            "inheritance",
            "join-edges/before/notebook/synthetic.one",
            "rust-join/inheritance/manifest.json",
            false,
        ),
        (
            "tags",
            "join-tags/before/notebook/synthetic.one",
            "rust-join/tags/manifest.json",
            false,
        ),
    ] {
        let source = std::fs::read(root.join(source)).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let folder = output.map_or_else(|| directory.path().to_owned(), |at| at.join(name));
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join("cache.sqlite");
        let mut cache = Replica::create(&path, &source).unwrap();
        let mut server = Server::new(&source);
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join(recorded)).unwrap()).unwrap();
        let cases = if split { &manifest["cases"] } else { &manifest };
        let mut entries = Vec::new();
        let mut refused: Vec<String> = Vec::new();
        let mut reviewed: Vec<String> = Vec::new();
        for case in cases
            .as_array()
            .unwrap()
            .iter()
            .filter(|case| case.get("intent").is_some())
        {
            let intent = &case["intent"];
            let left = origin(intent, split);
            let (space, page) = holder(&cache.snapshot().unwrap(), left);
            let offset = if split {
                intent["offset"].as_u64().unwrap()
            } else {
                1
            };
            let prefixed = offset > 0 && !content(&page, left).text().is_empty();
            if prefixed {
                server.visible =
                    onestore::replace_text(&server.visible, space, left, 0..0, "\u{2602}").unwrap();
                server.durable.clone_from(&server.visible);
            }
            let Ok(queued) = save_page(&cache, left, |page| replay(page, intent, split)) else {
                // Joining runs that differ in language or spacing is not a page-model edit.
                refused.push(case["case"].as_str().unwrap().to_owned());
                continue;
            };
            let id = queued.unwrap();
            let mut entry = serde_json::json!({"case": case["case"], "id": id, "space": space,
                "intent": intent, "remote_prefix": prefixed});
            drop(cache);
            cache = Replica::open(&path).unwrap();
            let case = entry["case"].as_str().unwrap().to_owned();
            let intent = entry["intent"].clone();
            let mut uncertain = id % 2 == 0;
            let revision = loop {
                if uncertain {
                    server.fault = Fault::UnknownAfter;
                }
                match cache.sync_once(&mut server) {
                    Err(_) => {
                        assert!(matches!(
                            cache.status(id).unwrap(),
                            Some(EditStatus::AwaitingConfirmation { .. })
                        ));
                        uncertain = false;
                        drop(cache);
                        cache = Replica::open(&path).unwrap();
                    }
                    Ok(notebook::Synced {
                        edit: Some((_, EditStatus::Published { revision })),
                        ..
                    }) => {
                        assert!(matches!(
                            cache.status(id).unwrap(),
                            Some(EditStatus::Published { .. })
                        ));
                        break revision;
                    }
                    Ok(notebook::Synced {
                        edit: Some((actual, EditStatus::Conflict(_))),
                        ..
                    }) => {
                        assert_eq!(actual, id);
                        server.fault = Fault::None;
                        reviewed.push(case.clone());
                        // The reviewed split sits after the remote's one-unit prefix.
                        let mut placed = intent.clone();
                        if split && entry["remote_prefix"] == true {
                            placed["offset"] = (intent["offset"].as_u64().unwrap() + 1).into();
                        }
                        review_page(&cache, id, origin(&intent, split), |page| {
                            replay(page, &placed, split)
                        });
                    }
                    other => panic!("{name} {case}: {other:?}"),
                }
            };
            entry["revision"] = serde_json::to_value(revision).unwrap();
            entries.push(entry);
        }
        let unrepresentable = ["Join inherited styles".to_owned()];
        let expected: (usize, &[String], &[String]) = match name {
            "splits" | "joins" => (12, &[], &[]),
            "inheritance" => (4, &[], &unrepresentable),
            _ => (3, &[], &[]),
        };
        assert_eq!(
            (entries.len(), reviewed.as_slice(), refused.as_slice()),
            expected,
            "{name}"
        );
        assert_eq!(server.publications, entries.len());
        assert_eq!(cache.sync_once(&mut server).unwrap().edit, None);
        assert_eq!(cache.snapshot().unwrap(), server.durable);
        if output.is_some() {
            cache
                .export_recovery(folder.join("recovery.sqlite"))
                .unwrap();
            std::fs::create_dir(folder.join("candidate")).unwrap();
            std::fs::write(folder.join("candidate/synthetic.one"), &server.durable).unwrap();
            std::fs::write(
                folder.join("manifest.json"),
                serde_json::to_vec_pretty(&entries).unwrap(),
            )
            .unwrap();
        }
    }
}

/// A control page of the reconciliation fixture, by title.
fn control(title: &str) -> (ExGuid, Page) {
    let (space, ..) = controls()
        .into_iter()
        .find(|(_, name, _)| name == title)
        .unwrap();
    (space, page_of(BEFORE, space))
}

/// A remote holding the control page with `change` applied by a native author.
fn remote_with(space: ExGuid, change: impl FnOnce(&mut Page)) -> Server {
    let mut page = page_of(BEFORE, space);
    change(&mut page);
    Server::new(
        PreparedEdit::page(BEFORE, space, &page, "Native author")
            .unwrap()
            .as_bytes(),
    )
}

#[test]
fn a_split_publishes_two_paragraphs_and_a_join_puts_them_back() {
    let (space, page) = control("Join remote prefix");
    let left = body(&page)[0].text().unwrap().id;
    let directory = tempfile::tempdir().unwrap();
    let cache = Replica::create(directory.path().join("cache.sqlite"), BEFORE).unwrap();
    let mut server = Server::new(BEFORE);
    save_page(&cache, left, |page| split_paragraph(page, left, 2))
        .unwrap()
        .unwrap();
    assert!(matches!(
        cache.sync_once(&mut server).unwrap().edit,
        Some((_, EditStatus::Published { .. }))
    ));
    let published = page_of(&server.durable, space);
    assert_eq!(
        texts(&published),
        ["ab", "\u{1f980}cd", "Right", "Preserved sibling"]
    );
    let right = body(&published)[1].text().unwrap().id;
    save_page(&cache, left, |page| join_paragraphs(page, left, right))
        .unwrap()
        .unwrap();
    assert!(matches!(
        cache.sync_once(&mut server).unwrap().edit,
        Some((_, EditStatus::Published { .. }))
    ));
    assert_eq!(
        texts(&page_of(&server.durable, space)),
        ["ab\u{1f980}cd", "Right", "Preserved sibling"]
    );
    assert_eq!(server.publications, 2);
    assert_eq!(cache.snapshot().unwrap(), server.durable);
}

#[test]
fn a_split_merges_with_a_remote_edit_to_another_paragraph() {
    let (space, page) = control("Join remote prefix");
    let (left, other) = (
        body(&page)[0].text().unwrap().id,
        body(&page)[1].text().unwrap().id,
    );
    let directory = tempfile::tempdir().unwrap();
    let cache = Replica::create(directory.path().join("cache.sqlite"), BEFORE).unwrap();
    let id = save_page(&cache, left, |page| split_paragraph(page, left, 2))
        .unwrap()
        .unwrap();
    let mut server = remote_with(space, |page| replace_text(page, other, 0..0, "Remote "));
    assert!(
        matches!(cache.sync_once(&mut server).unwrap().edit, Some((actual, EditStatus::Published { .. })) if actual == id)
    );
    assert_eq!(
        texts(&page_of(&server.durable, space)),
        ["ab", "\u{1f980}cd", "Remote Right", "Preserved sibling"]
    );
}

#[test]
fn a_join_conflicts_when_the_remote_changed_the_paragraph_it_removes() {
    let (space, page) = control("Join remote prefix");
    let (left, right) = (
        body(&page)[0].text().unwrap().id,
        body(&page)[1].text().unwrap().id,
    );
    let directory = tempfile::tempdir().unwrap();
    let cache = Replica::create(directory.path().join("cache.sqlite"), BEFORE).unwrap();
    let id = save_page(&cache, left, |page| join_paragraphs(page, left, right))
        .unwrap()
        .unwrap();
    let mut server = remote_with(space, |page| replace_text(page, right, 0..0, "Remote "));
    assert_eq!(
        cache.sync_once(&mut server).unwrap().edit,
        Some((id, EditStatus::Conflict(ConflictKind::ContentChanged)))
    );
    assert_eq!(server.publications, 0);
    review_page(&cache, id, left, |page| join_paragraphs(page, left, right));
    assert!(matches!(
        cache.sync_once(&mut server).unwrap().edit,
        Some((_, EditStatus::Published { .. }))
    ));
    assert_eq!(
        texts(&page_of(&server.durable, space))[0],
        "ab\u{1f980}cdRemote Right"
    );
}

#[test]
fn edits_dependent_on_a_published_split_survive_reopen() {
    let (space, page) = control("Join remote prefix");
    let (left, other) = (
        body(&page)[0].text().unwrap().id,
        body(&page)[1].text().unwrap().id,
    );
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("dependent.sqlite");
    let mut cache = Replica::create(&path, BEFORE).unwrap();
    let mut server = Server::new(BEFORE);
    save_page(&cache, left, |page| split_paragraph(page, left, 2))
        .unwrap()
        .unwrap();
    cache.sync_once(&mut server).unwrap().edit.unwrap();
    let tail = body(&page_of(&cache.snapshot().unwrap(), space))[1]
        .text()
        .unwrap()
        .id;
    let dependent = save_page(&cache, tail, |page| {
        replace_text(page, tail, 0..0, "Local ")
    })
    .unwrap()
    .unwrap();
    let local = cache.snapshot().unwrap();
    let pending = cache.pending().unwrap();
    drop(cache);
    cache = Replica::open(&path).unwrap();
    assert_eq!(pages(&cache.snapshot().unwrap()), pages(&local));
    assert_eq!(cache.pending().unwrap(), pending);
    let mut remote = page_of(&server.durable, space);
    replace_text(&mut remote, other, 0..0, "Remote ");
    server.visible = PreparedEdit::page(&server.durable, space, &remote, "Native author")
        .unwrap()
        .as_bytes()
        .to_vec();
    server.durable.clone_from(&server.visible);
    assert!(
        matches!(cache.sync_once(&mut server).unwrap().edit, Some((actual, EditStatus::Published { .. })) if actual == dependent)
    );
    assert_eq!(
        texts(&page_of(&server.durable, space)),
        [
            "ab",
            "Local \u{1f980}cd",
            "Remote Right",
            "Preserved sibling"
        ]
    );
}

#[test]
fn uncertain_splits_and_joins_keep_the_original_attempt_across_reopen() {
    for join in [false, true] {
        for fault in [
            Fault::Before,
            Fault::UnknownBefore,
            Fault::UnknownAfter,
            Fault::Committed,
        ] {
            let (space, page) = control("Join remote prefix");
            let (left, right) = (
                body(&page)[0].text().unwrap().id,
                body(&page)[1].text().unwrap().id,
            );
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("uncertain.sqlite");
            let cache = Replica::create(&path, BEFORE).unwrap();
            let id = save_page(&cache, left, |page| {
                if join {
                    join_paragraphs(page, left, right);
                } else {
                    split_paragraph(page, left, 2);
                }
            })
            .unwrap()
            .unwrap();
            let local = cache.snapshot().unwrap();
            let pending = cache.pending().unwrap();
            let mut server = Server::new(BEFORE);
            server.fault = fault;
            assert!(cache.sync_once(&mut server).is_err());
            let state = cache.status(id).unwrap().unwrap();
            drop(cache);
            let cache = Replica::open(&path).unwrap();
            assert_eq!(cache.status(id).unwrap(), Some(state.clone()));
            match fault {
                Fault::Before => {
                    assert_eq!(state, EditStatus::Pending);
                    assert_eq!(cache.pending().unwrap(), pending);
                    assert!(matches!(
                        cache.sync_once(&mut server).unwrap().edit.unwrap().1,
                        EditStatus::Published { .. }
                    ));
                    assert_eq!(server.publications, 2);
                }
                Fault::UnknownBefore => {
                    assert!(matches!(state, EditStatus::AwaitingConfirmation { .. }));
                    assert_eq!(cache.pending().unwrap(), pending);
                    assert_eq!(pages(&cache.snapshot().unwrap()), pages(&local));
                    for _ in 0..3 {
                        assert_eq!(
                            cache.sync_once(&mut server).unwrap().edit,
                            Some((id, state.clone()))
                        );
                    }
                    assert_eq!(server.publications, 1);
                    assert_eq!(server.durable, BEFORE);
                    continue;
                }
                Fault::UnknownAfter => {
                    assert!(matches!(state, EditStatus::AwaitingConfirmation { .. }));
                    assert_eq!(pages(&cache.snapshot().unwrap()), pages(&local));
                    assert!(matches!(
                        cache.sync_once(&mut server).unwrap().edit.unwrap().1,
                        EditStatus::Published { .. }
                    ));
                    assert_eq!(server.publications, 1);
                }
                _ => {
                    assert!(matches!(state, EditStatus::Published { .. }));
                    assert_eq!(server.publications, 1);
                }
            }
            assert!(cache.pending().unwrap().is_empty());
            let expected: &[&str] = if join {
                &["ab\u{1f980}cdRight", "Preserved sibling"]
            } else {
                &["ab", "\u{1f980}cd", "Right", "Preserved sibling"]
            };
            assert_eq!(texts(&page_of(&server.durable, space)), expected);
            assert_eq!(cache.sync_once(&mut server).unwrap().edit, None);
            assert_eq!(cache.snapshot().unwrap(), server.durable);
        }
    }
}

#[test]
fn a_competing_join_leaves_the_other_client_a_reviewable_split() {
    let (space, page) = control("Join remote prefix");
    let (left, right) = (
        body(&page)[0].text().unwrap().id,
        body(&page)[1].text().unwrap().id,
    );
    let directory = tempfile::tempdir().unwrap();
    let first = Replica::create(directory.path().join("first.sqlite"), BEFORE).unwrap();
    let path = directory.path().join("second.sqlite");
    let second = Replica::create(&path, BEFORE).unwrap();
    save_page(&first, left, |page| join_paragraphs(page, left, right))
        .unwrap()
        .unwrap();
    let id = save_page(&second, right, |page| split_paragraph(page, right, 2))
        .unwrap()
        .unwrap();
    let local = second.snapshot().unwrap();
    let pending = second.pending().unwrap();
    let mut server = Server::new(BEFORE);
    assert!(matches!(
        first.sync_once(&mut server).unwrap().edit.unwrap().1,
        EditStatus::Published { .. }
    ));
    assert_eq!(
        second.sync_once(&mut server).unwrap().edit,
        Some((id, EditStatus::Conflict(ConflictKind::ContentChanged)))
    );
    assert_eq!(server.publications, 1);
    drop(second);
    let second = Replica::open(&path).unwrap();
    assert_eq!(pages(&second.snapshot().unwrap()), pages(&local));
    assert_eq!(second.pending().unwrap(), pending);
    assert_eq!(
        second.status(id).unwrap(),
        Some(EditStatus::Conflict(ConflictKind::ContentChanged))
    );
    review_page(&second, id, left, |page| {
        split_paragraph(page, left, 2);
    });
    assert!(matches!(
        second.sync_once(&mut server).unwrap().edit.unwrap().1,
        EditStatus::Published { .. }
    ));
    assert_eq!(
        texts(&page_of(&server.durable, space)),
        ["ab", "\u{1f980}cdRight", "Preserved sibling"]
    );
}

#[test]
fn recorded_native_splits_and_joins_replay_over_a_remote_prefix() {
    offline_paragraphs(None);
}

#[test]
#[ignore = "exports native-controlled reconciliation for cold OneNote validation"]
fn export_native_reconciliation() {
    let output = std::path::PathBuf::from(
        std::env::var_os("ONESTORE_NATIVE_RECONCILIATION_OUTPUT").unwrap(),
    );
    assert!(output.is_absolute());
    std::fs::create_dir(&output).unwrap();
    let (bytes, cases) = reconcile_controls();
    std::fs::create_dir(output.join("candidate")).unwrap();
    std::fs::write(output.join("candidate/synthetic.one"), bytes).unwrap();
    std::fs::write(
        output.join("manifest.json"),
        serde_json::to_vec_pretty(&cases).unwrap(),
    )
    .unwrap();
}

#[test]
#[ignore = "exports reconciled paragraph edits for independent native validation"]
fn export_native_offline_paragraphs() {
    let output =
        std::path::PathBuf::from(std::env::var_os("ONESTORE_OFFLINE_PARAGRAPH_OUTPUT").unwrap());
    assert!(output.is_absolute());
    std::fs::create_dir(&output).unwrap();
    offline_paragraphs(Some(&output));
}
