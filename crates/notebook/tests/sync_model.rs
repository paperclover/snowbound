//! Page-model saves: one intent per page until it is attempted, reconciliation against the
//! remote page, reviewed conflicts, and independence between pages.

use notebook::{ConflictKind, EditStatus, Replica};
use onestore::{ExGuid, PreparedEdit, RevisionIndex, Store, document::Document};

#[path = "support/server.rs"]
mod server;
use server::*;
#[path = "support/model_ops.rs"]
mod model_ops;

use notebook::{Operation, PageIntent};
use onestore::page::{Page, PageObject};

const OUTLINES: &[u8] =
    include_bytes!("../../../corpus/outline-edit/before/notebook/synthetic.one");

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

/// Edits paragraph `index` of the first body outline: replaces its text range with `replacement`.
fn edit_paragraph(page: &mut Page, index: usize, range: std::ops::Range<u32>, replacement: &str) {
    let outline = page
        .objects
        .iter_mut()
        .find_map(|object| match object {
            PageObject::Outline(outline) => Some(outline),
            _ => None,
        })
        .unwrap();
    let text = outline.paragraphs[index].text_mut().unwrap();
    let format = text.text.format_at(range.start).unwrap().clone();
    text.text
        .apply(onestore::page::text::Edit {
            range,
            replacement: onestore::page::Paragraph::new(replacement.into(), format),
        })
        .unwrap();
}

fn texts(page: &Page) -> Vec<String> {
    page.objects
        .iter()
        .find_map(|object| match object {
            PageObject::Outline(outline) => Some(
                outline
                    .paragraphs
                    .iter()
                    .filter_map(|p| p.text().map(|t| t.text.text().to_owned()))
                    .collect(),
            ),
            _ => None,
        })
        .unwrap()
}

fn remote_with(space: ExGuid, change: impl FnOnce(&mut Page)) -> Server {
    let mut page = page_of(OUTLINES, space);
    change(&mut page);
    Server::new(
        PreparedEdit::page(OUTLINES, space, &page, "Native author")
            .unwrap()
            .as_bytes(),
    )
}

#[test]
fn concurrent_edits_to_different_paragraphs_merge() {
    let (space, mut after) = page_titled(OUTLINES, "Move leaf down");
    let directory = tempfile::tempdir().unwrap();
    let cache = Replica::create(directory.path().join("cache.sqlite"), OUTLINES).unwrap();
    edit_paragraph(&mut after, 0, 0..0, "Local ");
    let id = cache
        .save(OUTLINES, space, &after, "Model author")
        .unwrap()
        .unwrap();
    let mut server = remote_with(space, |page| edit_paragraph(page, 2, 0..0, "Remote "));
    assert!(
        matches!(cache.sync_once(&mut server).unwrap(), Some((n, EditStatus::Published { .. })) if n == id)
    );
    let published = texts(&page_of(&server.durable, space));
    assert!(published[0].starts_with("Local Anchor"), "{published:?}");
    assert!(published[2].starts_with("Remote Trailing"), "{published:?}");
    assert_eq!(
        cache
            .status(id)
            .unwrap()
            .map(|s| matches!(s, EditStatus::Published { .. })),
        Some(true)
    );
}

#[test]
fn concurrent_edits_to_one_paragraph_merge_unless_their_ranges_overlap() {
    let (space, mut after) = page_titled(OUTLINES, "Move leaf down");
    let directory = tempfile::tempdir().unwrap();
    let cache = Replica::create(directory.path().join("cache.sqlite"), OUTLINES).unwrap();
    append(&mut after, " local");
    let id = cache
        .save(OUTLINES, space, &after, "Model author")
        .unwrap()
        .unwrap();
    let mut server = remote_with(space, |page| edit_paragraph(page, 0, 0..0, "Remote "));
    assert!(
        matches!(cache.sync_once(&mut server).unwrap(), Some((n, EditStatus::Published { .. })) if n == id)
    );
    assert_eq!(
        texts(&page_of(&server.durable, space))[0],
        "Remote Anchor local"
    );

    let cache = Replica::create(directory.path().join("overlap.sqlite"), OUTLINES).unwrap();
    let mut after = page_of(OUTLINES, space);
    edit_paragraph(&mut after, 0, 0..6, "Local");
    let id = cache
        .save(OUTLINES, space, &after, "Model author")
        .unwrap()
        .unwrap();
    let mut server = remote_with(space, |page| edit_paragraph(page, 0, 0..6, "Remote"));
    assert_eq!(
        cache.sync_once(&mut server).unwrap(),
        Some((id, EditStatus::Conflict(ConflictKind::ContentChanged)))
    );
    assert_eq!(server.publications, 0);
}

#[test]
fn a_local_insertion_merges_with_a_remote_deletion_elsewhere() {
    let (space, mut after) = page_titled(OUTLINES, "Move leaf down");
    let directory = tempfile::tempdir().unwrap();
    let cache = Replica::create(directory.path().join("cache.sqlite"), OUTLINES).unwrap();
    let template = texts(&after);
    assert_eq!(template.len(), 3);
    {
        let outline = after
            .objects
            .iter_mut()
            .find_map(|object| match object {
                PageObject::Outline(outline) => Some(outline),
                _ => None,
            })
            .unwrap();
        let mut fresh = outline.paragraphs[0].clone();
        fresh.id = onestore::page::text::new_id().unwrap();
        fresh.style = None;
        let text = fresh.text_mut().unwrap();
        text.id = onestore::page::text::new_id().unwrap();
        text.text = onestore::page::Paragraph::new(
            "Inserted locally".into(),
            text.text.format_at(0).unwrap().clone(),
        );
        outline.paragraphs.insert(1, fresh);
    }
    let id = cache
        .save(OUTLINES, space, &after, "Model author")
        .unwrap()
        .unwrap();
    let mut server = remote_with(space, |page| {
        let outline = page
            .objects
            .iter_mut()
            .find_map(|object| match object {
                PageObject::Outline(outline) => Some(outline),
                _ => None,
            })
            .unwrap();
        outline.paragraphs.pop();
    });
    assert!(
        matches!(cache.sync_once(&mut server).unwrap(), Some((n, EditStatus::Published { .. })) if n == id)
    );
    assert_eq!(
        texts(&page_of(&server.durable, space)),
        vec![
            template[0].clone(),
            "Inserted locally".to_owned(),
            template[1].clone()
        ]
    );
}

#[test]
fn a_remote_deletion_of_the_edited_paragraph_conflicts() {
    let (space, mut after) = page_titled(OUTLINES, "Move leaf down");
    let directory = tempfile::tempdir().unwrap();
    let cache = Replica::create(directory.path().join("cache.sqlite"), OUTLINES).unwrap();
    edit_paragraph(&mut after, 2, 0..0, "Local ");
    let id = cache
        .save(OUTLINES, space, &after, "Model author")
        .unwrap()
        .unwrap();
    let mut server = remote_with(space, |page| {
        let outline = page
            .objects
            .iter_mut()
            .find_map(|object| match object {
                PageObject::Outline(outline) => Some(outline),
                _ => None,
            })
            .unwrap();
        outline.paragraphs.pop();
    });
    assert_eq!(
        cache.sync_once(&mut server).unwrap(),
        Some((id, EditStatus::Conflict(ConflictKind::ContentChanged)))
    );
}

#[test]
fn outline_moves_merge_with_remote_text_edits_but_not_with_remote_moves() {
    let (space, mut after) = page_titled(OUTLINES, "Move leaf down");
    let directory = tempfile::tempdir().unwrap();
    let cache = Replica::create(directory.path().join("cache.sqlite"), OUTLINES).unwrap();
    let outline_id = {
        let PageObject::Outline(outline) = after
            .objects
            .iter_mut()
            .find(|o| matches!(o, PageObject::Outline(_)))
            .unwrap()
        else {
            unreachable!()
        };
        outline.layout.x = Some(200.0);
        outline.layout.y = Some(300.0);
        outline.id
    };
    let id = cache
        .save(OUTLINES, space, &after, "Model author")
        .unwrap()
        .unwrap();
    let mut server = remote_with(space, |page| edit_paragraph(page, 0, 0..0, "Remote "));
    assert!(
        matches!(cache.sync_once(&mut server).unwrap(), Some((n, EditStatus::Published { .. })) if n == id)
    );
    let published = page_of(&server.durable, space);
    let PageObject::Outline(outline) = published
        .objects
        .iter()
        .find(|o| o.id() == outline_id)
        .unwrap()
    else {
        unreachable!()
    };
    assert_eq!(
        (outline.layout.x, outline.layout.y),
        (Some(200.0), Some(300.0))
    );
    assert!(
        outline.paragraphs[0]
            .text()
            .unwrap()
            .text
            .text()
            .starts_with("Remote ")
    );

    let cache = Replica::create(directory.path().join("moves.sqlite"), OUTLINES).unwrap();
    let id = cache
        .save(OUTLINES, space, &after, "Model author")
        .unwrap()
        .unwrap();
    let mut server = remote_with(space, |page| {
        let PageObject::Outline(outline) = page
            .objects
            .iter_mut()
            .find(|o| matches!(o, PageObject::Outline(_)))
            .unwrap()
        else {
            unreachable!()
        };
        outline.layout.x = Some(50.0);
    });
    assert_eq!(
        cache.sync_once(&mut server).unwrap(),
        Some((id, EditStatus::Conflict(ConflictKind::ContentChanged)))
    );
}

#[test]
fn a_local_bullet_merges_with_a_remote_text_edit_and_publishes_its_definition() {
    let (space, before) = page_titled(OUTLINES, "Move leaf down");
    let directory = tempfile::tempdir().unwrap();
    let cache = Replica::create(directory.path().join("cache.sqlite"), OUTLINES).unwrap();
    let mut after = before.clone();
    let bullet = onestore::page::text::new_id().unwrap();
    after.definitions.insert(
        bullet,
        onestore::page::Definition {
            kind: onestore::document::Kind::List {
                font: Some("Courier New".into()),
                format: Some("\u{25cb}".into()),
                restart: None,
                bullet: Some(4),
            },
            format: onestore::document::Format {
                font_size: Some(11.0),
                color: Some(0xff000000),
                ..Default::default()
            },
        },
    );
    let outline = after
        .objects
        .iter_mut()
        .find_map(|object| match object {
            PageObject::Outline(outline) if !outline.title => Some(outline),
            _ => None,
        })
        .unwrap();
    outline.paragraphs[0].lists = vec![bullet];
    let id = cache
        .save(OUTLINES, space, &after, "Model author")
        .unwrap()
        .unwrap();
    let mut server = remote_with(space, |page| edit_paragraph(page, 1, 0..0, "Remote "));
    assert!(
        matches!(cache.sync_once(&mut server).unwrap(), Some((n, EditStatus::Published { .. })) if n == id)
    );
    let published = page_of(&server.durable, space);
    let outline = published
        .objects
        .iter()
        .find_map(|object| match object {
            PageObject::Outline(outline) if !outline.title => Some(outline),
            _ => None,
        })
        .unwrap();
    assert_eq!(outline.paragraphs[0].lists, vec![bullet]);
    assert!(matches!(
        published.definitions[&bullet].kind,
        onestore::document::Kind::List {
            bullet: Some(4),
            ..
        }
    ));
    assert!(texts(&published)[1].starts_with("Remote "));
}
