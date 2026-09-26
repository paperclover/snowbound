//! Converting schema-14 caches: whole-page edits become ops, verified page by page against
//! the old working image, with a recovery archive first and nothing changed on a mismatch.

use notebook::{ConflictKind, EditStatus, Replica, Resolution};
use onestore::{ExGuid, PageCreation, PageEdit, PreparedEdit, page::Page};
use rusqlite::{Connection, params};
use serde_json::json;
use std::path::Path;

#[path = "support/server.rs"]
mod server;
use server::*;
#[path = "support/model_ops.rs"]
mod model_ops;

const SOURCE: &[u8] = include_bytes!("../../../corpus/outline-edit/before/notebook/synthetic.one");

/// The body pages of the fixture with their first body text.
fn body_pages(source: &[u8]) -> Vec<(ExGuid, ExGuid)> {
    pages(source)
        .into_iter()
        .filter_map(|(space, page)| {
            let text = page.objects.iter().find_map(|object| match object {
                onestore::page::PageObject::Outline(outline) => {
                    outline.paragraphs.first()?.text().map(|text| text.id)
                }
                _ => None,
            })?;
            Some((space, text))
        })
        .collect()
}

/// A schema-14 queue entry.
enum Queued {
    Page(ExGuid, Page),
    Create(PageCreation),
    Pages(Vec<PageEdit>),
    Delete(Vec<ExGuid>),
}

/// Writes a schema-14 cache holding `queue` on `base`, as that schema stored it; returns
/// the working image.
fn v14(path: &Path, base: &[u8], queue: &[Queued], working: Option<&[u8]>) -> Vec<u8> {
    let mut image = base.to_vec();
    let mut edits = Vec::new();
    for entry in queue {
        let (space, operation, next) = match entry {
            Queued::Page(space, after) => {
                let before = model_ops::page_of(&image, *space);
                let next = PreparedEdit::page(&image, *space, after, "Author").unwrap();
                (
                    *space,
                    json!({"Page": {"before": before, "after": after, "author": "Author"}}),
                    next.as_bytes().to_vec(),
                )
            }
            Queued::Create(creation) => (
                creation.space(),
                json!({"CreatePage": creation}),
                PreparedEdit::create_page(&image, creation)
                    .unwrap()
                    .as_bytes()
                    .to_vec(),
            ),
            Queued::Pages(batch) => {
                let observed: Vec<_> = pages(&image)
                    .into_iter()
                    .map(|(space, _)| (space, 1))
                    .collect();
                (
                    root(&image),
                    json!({"Pages": {"edits": batch, "observed": observed}}),
                    PreparedEdit::pages(&image, batch)
                        .unwrap()
                        .as_bytes()
                        .to_vec(),
                )
            }
            Queued::Delete(spaces) => (
                root(&image),
                json!({"DeletePages": spaces}),
                PreparedEdit::delete_pages_permanently(&image, spaces)
                    .unwrap()
                    .as_bytes()
                    .to_vec(),
            ),
        };
        edits.push((space, operation));
        image = next;
    }
    let working = working.map_or(image, <[u8]>::to_vec);
    std::fs::File::create_new(path).unwrap();
    let connection = Connection::open(path).unwrap();
    connection
        .execute_batch(
            "PRAGMA application_id=1330529615; PRAGMA user_version=14;
            CREATE TABLE replica (id INTEGER PRIMARY KEY CHECK(id=1), base BLOB NOT NULL, working BLOB NOT NULL) STRICT;
            CREATE TABLE edits (id INTEGER PRIMARY KEY AUTOINCREMENT CHECK(id>0), space TEXT NOT NULL, operation TEXT NOT NULL) STRICT;
            CREATE TABLE attempt (id INTEGER PRIMARY KEY CHECK(id=1), edit_id INTEGER NOT NULL UNIQUE REFERENCES edits(id) ON DELETE CASCADE, revisions TEXT NOT NULL) STRICT;
            CREATE TABLE receipts (edit_id INTEGER PRIMARY KEY CHECK(edit_id>0), revision TEXT NOT NULL) STRICT;
            CREATE TABLE archived (edit_id INTEGER PRIMARY KEY CHECK(edit_id>0), archive TEXT NOT NULL) STRICT;
            CREATE TABLE conflicts (edit_id INTEGER PRIMARY KEY REFERENCES edits(id) ON DELETE CASCADE, kind INTEGER NOT NULL CHECK(kind BETWEEN 0 AND 3)) STRICT;
            CREATE TABLE assets (name TEXT PRIMARY KEY NOT NULL, data BLOB NOT NULL, sha256 BLOB NOT NULL CHECK(length(sha256)=32)) STRICT;",
        )
        .unwrap();
    // Schema 14 kept the working image as its length and the runs differing from the base.
    let mut patch = (working.len() as u64).to_le_bytes().to_vec();
    patch.extend_from_slice(&0_u64.to_le_bytes());
    patch.extend_from_slice(&(working.len() as u64).to_le_bytes());
    patch.extend_from_slice(&working);
    connection
        .execute(
            "INSERT INTO replica VALUES (1, ?1, ?2)",
            params![base, patch],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO receipts VALUES (1, ?1)",
            [ExGuid {
                guid: [7; 16],
                n: 1,
            }
            .to_string()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO sqlite_sequence(name, seq) VALUES ('edits', 1)",
            [],
        )
        .unwrap();
    for (space, operation) in edits {
        connection
            .execute(
                "INSERT INTO edits(space, operation) VALUES (?1, ?2)",
                params![space.to_string(), operation.to_string()],
            )
            .unwrap();
    }
    working
}

fn root(image: &[u8]) -> ExGuid {
    onestore::RevisionIndex::parse(&onestore::Store::parse(image).unwrap())
        .unwrap()
        .root
}

fn appended(source: &[u8], space: ExGuid, text: ExGuid, suffix: &str) -> Page {
    let mut page = model_ops::page_of(source, space);
    let end = model_ops::paragraph_with(&page, text)
        .unwrap()
        .text()
        .unwrap()
        .text
        .text()
        .encode_utf16()
        .count() as u32;
    model_ops::replace_text(&mut page, text, end..end, suffix);
    page
}

fn archive(path: &Path) -> std::path::PathBuf {
    let mut archive = path.as_os_str().to_owned();
    archive.push(".v14-recovery");
    archive.into()
}

#[test]
fn a_schema_14_queue_converts_to_ops_that_reach_its_working_pages_and_publish() {
    let body = body_pages(SOURCE);
    let ((a, a_text), (b, b_text)) = (body[0], body[1]);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("cache.sqlite");
    let creation = PageCreation::new(None, Some("Created offline"), "Author").unwrap();
    let first = appended(SOURCE, a, a_text, " one");
    let working = v14(
        &path,
        SOURCE,
        &[
            Queued::Page(a, first.clone()),
            Queued::Page(b, appended(SOURCE, b, b_text, " other")),
            Queued::Create(creation.clone()),
            Queued::Pages(vec![PageEdit::move_to(b, Some(a), 1).unwrap()]),
            Queued::Page(
                a,
                appended(
                    PreparedEdit::page(SOURCE, a, &first, "Author")
                        .unwrap()
                        .as_bytes(),
                    a,
                    a_text,
                    " two",
                ),
            ),
            Queued::Delete(vec![body[2].0]),
        ],
        None,
    );
    let cache = Replica::open(&path).unwrap();
    assert!(archive(&path).exists());
    let pending = cache.pending().unwrap();
    assert_eq!(
        pending.iter().map(|edit| edit.id).collect::<Vec<_>>(),
        [2, 3, 4, 5, 6, 7]
    );
    assert!(pending.iter().all(|edit| !edit.edit.ops.is_empty()));
    assert_eq!(
        cache.status(1).unwrap(),
        Some(EditStatus::Published {
            revision: ExGuid {
                guid: [7; 16],
                n: 1
            }
        })
    );
    let local = pages(&cache.snapshot().unwrap());
    assert_eq!(local, pages(&working));
    drop(cache);
    // A converted cache opens as it is.
    let cache = Replica::open(&path).unwrap();
    assert_eq!(pages(&cache.snapshot().unwrap()), local);
    let next = cache
        .apply(
            "Author",
            onestore::op::Edit {
                at: 133_000_000_000_000_000,
                ops: vec![onestore::op::Op::Page {
                    space: b,
                    op: onestore::op::PageOp::Text {
                        text: b_text,
                        range: 0..0,
                        with: " three".into(),
                    },
                }],
            },
        )
        .unwrap();
    assert!(next > 7);
    let mut server = Server::new(SOURCE);
    assert!(matches!(
        cache.sync_once(&mut server).unwrap().edit,
        Some((id, EditStatus::Published { .. })) if id == next
    ));
    let published = pages(&server.durable);
    assert_eq!(published, pages(&cache.snapshot().unwrap()));
    assert!(
        published
            .iter()
            .any(|(space, _)| *space == creation.space())
    );
    assert!(published.iter().all(|(space, _)| *space != body[2].0));
}

#[test]
fn an_uncertain_attempt_and_a_conflict_keep_their_states() {
    let body = body_pages(SOURCE);
    let (a, a_text) = body[0];
    let directory = tempfile::tempdir().unwrap();

    // The oldest edit was attempted: its schema-14 evidence names the revision it published.
    let path = directory.path().join("attempted.sqlite");
    let after = appended(SOURCE, a, a_text, " attempted");
    let published = PreparedEdit::page(SOURCE, a, &after, "Author").unwrap();
    let revision =
        onestore::RevisionIndex::parse(&onestore::Store::parse(published.as_bytes()).unwrap())
            .unwrap()
            .active(a)
            .unwrap();
    v14(&path, SOURCE, &[Queued::Page(a, after.clone())], None);
    Connection::open(&path)
        .unwrap()
        .execute(
            "INSERT INTO attempt VALUES (1, 2, ?1)",
            [json!({a.to_string(): revision.to_string()}).to_string()],
        )
        .unwrap();
    let cache = Replica::open(&path).unwrap();
    let awaiting = EditStatus::AwaitingConfirmation { revision };
    assert_eq!(cache.status(2).unwrap(), Some(awaiting.clone()));
    let mut unchanged = Server::new(SOURCE);
    assert_eq!(
        cache.sync_once(&mut unchanged).unwrap().edit,
        Some((2, awaiting))
    );
    assert_eq!(unchanged.publications, 0);
    let mut landed = Server::new(published.as_bytes());
    assert_eq!(
        cache.sync_once(&mut landed).unwrap().edit,
        Some((2, EditStatus::Published { revision }))
    );
    assert_eq!(landed.publications, 0);
    drop(cache);

    // The oldest edit was in conflict: the conversion lowered it onto the remote page the
    // conflict was recorded against, and keeping mine publishes it.
    let path = directory.path().join("conflicted.sqlite");
    let remote = PreparedEdit::page(SOURCE, a, &appended(SOURCE, a, a_text, " remote"), "Native")
        .unwrap()
        .as_bytes()
        .to_vec();
    let local = appended(SOURCE, a, a_text, " local");
    let working = PreparedEdit::page(SOURCE, a, &local, "Author")
        .unwrap()
        .as_bytes()
        .to_vec();
    {
        // Schema 14 stored the remote as the base and kept the local working image.
        v14(&path, &remote, &[], Some(&working));
        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "INSERT INTO edits(space, operation) VALUES (?1, ?2)",
                params![
                    a.to_string(),
                    json!({"Page": {"before": model_ops::page_of(SOURCE, a), "after": local, "author": "Author"}}).to_string()
                ],
            )
            .unwrap();
        connection
            .execute("INSERT INTO conflicts VALUES (2, 3)", [])
            .unwrap();
    }
    let cache = Replica::open(&path).unwrap();
    assert_eq!(
        cache.status(2).unwrap(),
        Some(EditStatus::Conflict(ConflictKind::ContentChanged))
    );
    assert_eq!(
        model_ops::page_of(&cache.snapshot().unwrap(), a),
        model_ops::page_of(&working, a)
    );
    let mut server = Server::new(&remote);
    assert_eq!(
        cache.sync_once(&mut server).unwrap().edit,
        Some((2, EditStatus::Conflict(ConflictKind::ContentChanged)))
    );
    cache.resolve(2, Resolution::Mine).unwrap();
    assert!(matches!(
        cache.sync_once(&mut server).unwrap().edit,
        Some((2, EditStatus::Published { .. }))
    ));
    assert_eq!(
        model_ops::page_of(&server.durable, a),
        model_ops::page_of(&working, a)
    );
}

#[test]
fn a_page_the_conversion_cannot_reproduce_leaves_the_cache_untouched() {
    let body = body_pages(SOURCE);
    let (a, a_text) = body[0];
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("cache.sqlite");
    // The working image holds a page the queued edit does not reach.
    let stray = PreparedEdit::page(SOURCE, a, &appended(SOURCE, a, a_text, " stray"), "Author")
        .unwrap()
        .as_bytes()
        .to_vec();
    v14(
        &path,
        SOURCE,
        &[Queued::Page(a, appended(SOURCE, a, a_text, " queued"))],
        Some(&stray),
    );
    let before = std::fs::read(&path).unwrap();
    let Err(notebook::Error::Io(error)) = Replica::open(&path) else {
        panic!("the conversion must fail")
    };
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    assert!(error.to_string().contains(".v14-recovery"), "{error}");
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(archive(&path).exists());
    let version: u32 = Connection::open(archive(&path))
        .unwrap()
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 14);
    // A second open tries again from the same untouched cache.
    assert!(Replica::open(&path).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
}
