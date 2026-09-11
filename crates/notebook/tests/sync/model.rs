//! Page-model saves: one intent per page until it is attempted, reconciliation against the
//! remote page, reviewed conflicts, and independence between pages.

use super::*;
use notebook::{Operation, PageIntent};
use onestore::page::{Page, PageObject};

const OUTLINES: &[u8] =
    include_bytes!("../../../../corpus/outline-edit/before/notebook/synthetic.one");

fn page_of(bytes: &[u8], space: ExGuid) -> Page {
    let store = Store::parse(bytes).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    Page::from_space(&document, space).unwrap()
}

fn page_titled(bytes: &[u8], title: &str) -> (ExGuid, Page) {
    let store = Store::parse(bytes).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    document
        .pages()
        .unwrap()
        .into_iter()
        .find_map(|(sid, _)| {
            let page = Page::from_space(&document, sid).unwrap();
            (page.title == title).then_some((sid, page))
        })
        .unwrap()
}

/// Appends `suffix` to the first body paragraph of the first body outline.
fn append(page: &mut Page, suffix: &str) {
    let outline = page
        .objects
        .iter_mut()
        .find_map(|object| match object {
            PageObject::Outline(outline) => Some(outline),
            _ => None,
        })
        .unwrap();
    let text = outline.paragraphs[0].text_mut().unwrap();
    let end = text.text.utf16_offset(text.text.text().len()).unwrap();
    let format = text.text.format_at(end).unwrap().clone();
    text.text
        .apply(onestore::page::text::Edit {
            range: end..end,
            replacement: onestore::page::Paragraph::new(suffix.into(), format),
        })
        .unwrap();
}

fn first_text(page: &Page) -> String {
    page.objects
        .iter()
        .find_map(|object| match object {
            PageObject::Outline(outline) => {
                Some(outline.paragraphs[0].text().unwrap().text.text().to_owned())
            }
            _ => None,
        })
        .unwrap()
}

#[test]
fn a_saved_page_model_publishes_and_reads_back() {
    let (space, mut after) = page_titled(OUTLINES, "Move leaf down");
    let directory = tempfile::tempdir().unwrap();
    let cache = Replica::create(directory.path().join("cache.sqlite"), OUTLINES).unwrap();
    append(&mut after, " saved 🦀");
    let id = cache
        .save(OUTLINES, space, &after, "Model author")
        .unwrap()
        .unwrap();
    assert_eq!(cache.status(id).unwrap(), Some(EditStatus::Pending));
    assert_eq!(page_of(&cache.snapshot().unwrap(), space), after);
    let pending = cache.pending().unwrap();
    assert_eq!(pending.len(), 1);
    let Operation::Page(PageIntent {
        before,
        after: stored,
        author,
    }) = &pending[0].operation
    else {
        panic!()
    };
    assert_eq!(
        (before, stored, author.as_str()),
        (&page_of(OUTLINES, space), &after, "Model author")
    );
    let mut server = Server::new(OUTLINES);
    assert!(
        matches!(cache.sync_once(&mut server).unwrap(), Some((n, EditStatus::Published { .. })) if n == id)
    );
    assert_eq!(page_of(&server.durable, space), after);
    assert_eq!(server.publications, 1);
    assert!(cache.pending().unwrap().is_empty());
    assert_eq!(
        cache
            .save(&cache.snapshot().unwrap(), space, &after, "Model author")
            .unwrap(),
        None
    );
}

#[test]
fn saves_before_an_attempt_coalesce_into_one_publication() {
    let (space, mut after) = page_titled(OUTLINES, "Move leaf down");
    let directory = tempfile::tempdir().unwrap();
    let cache = Replica::create(directory.path().join("cache.sqlite"), OUTLINES).unwrap();
    append(&mut after, " one");
    let id = cache
        .save(OUTLINES, space, &after, "Model author")
        .unwrap()
        .unwrap();
    let mut later = page_of(&cache.snapshot().unwrap(), space);
    append(&mut later, " two");
    assert_eq!(
        cache
            .save(&cache.snapshot().unwrap(), space, &later, "Model author")
            .unwrap(),
        Some(id)
    );
    let pending = cache.pending().unwrap();
    assert_eq!(pending.len(), 1);
    let Operation::Page(intent) = &pending[0].operation else {
        panic!()
    };
    assert_eq!(intent.before, page_of(OUTLINES, space));
    assert_eq!(intent.after, later);
    let mut server = Server::new(OUTLINES);
    assert!(
        matches!(cache.sync_once(&mut server).unwrap(), Some((n, EditStatus::Published { .. })) if n == id)
    );
    assert_eq!(server.publications, 1);
    assert_eq!(
        first_text(&page_of(&server.durable, space)),
        first_text(&later)
    );
    let mut third = page_of(&cache.snapshot().unwrap(), space);
    append(&mut third, " three");
    let next = cache
        .save(&cache.snapshot().unwrap(), space, &third, "Model author")
        .unwrap()
        .unwrap();
    assert!(next > id);
}

#[test]
fn an_attempted_save_is_never_rewritten() {
    let (space, mut after) = page_titled(OUTLINES, "Move leaf down");
    let directory = tempfile::tempdir().unwrap();
    let cache = Replica::create(directory.path().join("cache.sqlite"), OUTLINES).unwrap();
    append(&mut after, " uncertain");
    let id = cache
        .save(OUTLINES, space, &after, "Model author")
        .unwrap()
        .unwrap();
    let mut server = Server::new(OUTLINES);
    server.fault = Fault::UnknownAfter;
    assert!(cache.sync_once(&mut server).is_err());
    assert!(matches!(
        cache.status(id).unwrap(),
        Some(EditStatus::AwaitingConfirmation { .. })
    ));
    let mut more = page_of(&cache.snapshot().unwrap(), space);
    append(&mut more, " more");
    let next = cache
        .save(&cache.snapshot().unwrap(), space, &more, "Model author")
        .unwrap()
        .unwrap();
    assert!(next > id);
    let pending = cache.pending().unwrap();
    assert_eq!(pending.iter().map(|p| p.id).collect::<Vec<_>>(), [id, next]);
    let Operation::Page(first) = &pending[0].operation else {
        panic!()
    };
    assert_eq!(first.after, after);
    let Operation::Page(second) = &pending[1].operation else {
        panic!()
    };
    assert_eq!((&second.before, &second.after), (&after, &more));
}

#[test]
fn a_remote_change_to_the_saved_page_conflicts_until_reviewed() {
    let (space, mut after) = page_titled(OUTLINES, "Move leaf down");
    let directory = tempfile::tempdir().unwrap();
    let cache = Replica::create(directory.path().join("cache.sqlite"), OUTLINES).unwrap();
    append(&mut after, " local");
    let id = cache
        .save(OUTLINES, space, &after, "Model author")
        .unwrap()
        .unwrap();
    let mut remote_page = page_of(OUTLINES, space);
    append(&mut remote_page, " remote");
    let remote = PreparedEdit::page(OUTLINES, space, &remote_page, "Native author").unwrap();
    let mut server = Server::new(remote.as_bytes());
    assert_eq!(
        cache.sync_once(&mut server).unwrap(),
        Some((id, EditStatus::Conflict(ConflictKind::ContentChanged)))
    );
    assert_eq!(server.publications, 0);
    assert!(cache.pending().unwrap().len() == 1);
    let local = cache.snapshot().unwrap();
    let observed = cache.remote_snapshot().unwrap();
    assert_eq!(observed, remote.as_bytes());
    let mut reviewed = page_of(&observed, space);
    append(&mut reviewed, " local");
    assert!(
        cache
            .review_page(id + 1, &local, &observed, &reviewed)
            .is_err()
    );
    assert!(cache.review_page(id, &local, OUTLINES, &reviewed).is_err());
    cache.review_page(id, &local, &observed, &reviewed).unwrap();
    assert_eq!(cache.status(id).unwrap(), Some(EditStatus::Pending));
    assert!(
        matches!(cache.sync_once(&mut server).unwrap(), Some((n, EditStatus::Published { .. })) if n == id)
    );
    assert_eq!(
        first_text(&page_of(&server.durable, space)),
        first_text(&reviewed)
    );
    assert!(first_text(&page_of(&server.durable, space)).ends_with(" remote local"));
}

#[test]
fn remote_changes_to_other_pages_do_not_conflict() {
    let (space, mut after) = page_titled(OUTLINES, "Move leaf down");
    let (other, mut other_page) = page_titled(OUTLINES, "Delete leaf");
    let directory = tempfile::tempdir().unwrap();
    let cache = Replica::create(directory.path().join("cache.sqlite"), OUTLINES).unwrap();
    append(&mut after, " local");
    let id = cache
        .save(OUTLINES, space, &after, "Model author")
        .unwrap()
        .unwrap();
    append(&mut other_page, " elsewhere");
    let remote = PreparedEdit::page(OUTLINES, other, &other_page, "Native author").unwrap();
    let mut server = Server::new(remote.as_bytes());
    assert!(
        matches!(cache.sync_once(&mut server).unwrap(), Some((n, EditStatus::Published { .. })) if n == id)
    );
    assert_eq!(page_of(&server.durable, space), after);
    assert_eq!(page_of(&server.durable, other), other_page);
    assert_eq!(page_of(&cache.snapshot().unwrap(), other), other_page);
}

#[test]
fn pending_saves_survive_reopening_the_cache() {
    let (space, mut after) = page_titled(OUTLINES, "Move leaf down");
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("cache.sqlite");
    let cache = Replica::create(&path, OUTLINES).unwrap();
    append(&mut after, " durable");
    let id = cache
        .save(OUTLINES, space, &after, "Model author")
        .unwrap()
        .unwrap();
    let pending = cache.pending().unwrap();
    drop(cache);
    let reopened = Replica::open(&path).unwrap();
    assert_eq!(reopened.pending().unwrap(), pending);
    assert_eq!(reopened.status(id).unwrap(), Some(EditStatus::Pending));
    assert_eq!(page_of(&reopened.snapshot().unwrap(), space), after);
}
