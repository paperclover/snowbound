//! Converting schema-14 caches: whole-page edits become ops, verified page by page against
//! the old working image, with a recovery archive first and nothing changed on a mismatch;
//! and schema-15 caches, whose batches lose the review conflict.

#[path = "../../onestore/tests/support/ops.rs"]
mod ops;

use notebook::{EditStatus, Replica};
use onestore::{ExGuid, PageCreation, PageEdit, op::SectionOp, page::Page};
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
                let next = ops::saved(&image, *space, after).unwrap();
                (
                    *space,
                    json!({"Page": {"before": before, "after": after, "author": "Author"}}),
                    next.as_slice().to_vec(),
                )
            }
            Queued::Create(creation) => (
                creation.space(),
                json!({"CreatePage": creation}),
                ops::section_op(&image, SectionOp::Create(creation.clone()))
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
                    ops::section_op(&image, SectionOp::Pages(batch.to_vec()))
                        .unwrap()
                        .as_bytes()
                        .to_vec(),
                )
            }
            Queued::Delete(spaces) => (
                root(&image),
                json!({"DeletePages": spaces}),
                ops::section_op(&image, SectionOp::Delete(spaces.to_vec()))
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
                    ops::saved(SOURCE, a, &first)
                        .unwrap()
                        .as_slice(),
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
    let local = pages(&snapshot(&cache));
    assert_eq!(local, pages(&working));
    drop(cache);
    // A converted cache opens as it is.
    let cache = Replica::open(&path).unwrap();
    assert_eq!(pages(&snapshot(&cache)), local);
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
    assert_eq!(published, pages(&snapshot(&cache)));
    assert!(
        published
            .iter()
            .any(|(space, _)| *space == creation.space())
    );
    assert!(published.iter().all(|(space, _)| *space != body[2].0));
}

#[test]
fn an_uncertain_attempt_keeps_its_state_and_a_conflict_becomes_a_conflict_page() {
    let body = body_pages(SOURCE);
    let (a, a_text) = body[0];
    let directory = tempfile::tempdir().unwrap();

    // The oldest edit was attempted: its schema-14 evidence names the revision it published.
    let path = directory.path().join("attempted.sqlite");
    let after = appended(SOURCE, a, a_text, " attempted");
    let published = ops::saved(SOURCE, a, &after).unwrap();
    let revision =
        onestore::RevisionIndex::parse(&onestore::Store::parse(published.as_slice()).unwrap())
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
    let mut landed = Server::new(published.as_slice());
    assert_eq!(
        cache.sync_once(&mut landed).unwrap().edit,
        Some((2, EditStatus::Published { revision }))
    );
    assert_eq!(landed.publications, 0);
    drop(cache);

    // The oldest edit was in conflict: the conversion keeps the remote page the conflict was
    // recorded against and queues the local version as its conflict page.
    let path = directory.path().join("conflicted.sqlite");
    let remote = ops::saved(SOURCE, a, &appended(SOURCE, a, a_text, " remote"))
        .unwrap()
        .as_slice()
        .to_vec();
    let local = appended(SOURCE, a, a_text, " local");
    let working = ops::saved(SOURCE, a, &local)
        .unwrap()
        .as_slice()
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
    assert_eq!(cache.status(2).unwrap(), Some(EditStatus::Pending));
    assert_eq!(
        model_ops::page_of(&snapshot(&cache), a),
        model_ops::page_of(&remote, a)
    );
    let mut server = Server::new(&remote);
    assert!(matches!(
        cache.sync_once(&mut server).unwrap().edit,
        Some((2, EditStatus::Published { .. }))
    ));
    assert_eq!(
        model_ops::page_of(&server.durable, a),
        model_ops::page_of(&remote, a)
    );
    assert_eq!(
        conflicts(&server.durable),
        [(
            a,
            vec![("Author".to_owned(), page_texts(&model_ops::page_of(&working, a)))]
        )]
    );
}

#[test]
fn a_page_the_conversion_cannot_reproduce_leaves_the_cache_untouched() {
    let body = body_pages(SOURCE);
    let (a, a_text) = body[0];
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("cache.sqlite");
    // The working image holds a page the queued edit does not reach.
    let stray = ops::saved(SOURCE, a, &appended(SOURCE, a, a_text, " stray"))
        .unwrap()
        .as_slice()
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

/// A schema-15 cache upgrades in place: its queue, receipts and recovery archives stay, and
/// a conflict it recorded is queued work the next sync keeps as a conflict page.
#[test]
fn a_schema_15_cache_upgrades_in_place_and_its_conflict_becomes_a_conflict_page() {
    let (space, text) = body_pages(SOURCE)[0];
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("cache.sqlite");
    let cache = Replica::create(&path, SOURCE).unwrap();
    let id = model_ops::save(&cache, text, |page| {
        model_ops::replace_text(page, text, 0..0, "Local ")
    })
    .unwrap()
    .unwrap();
    let pending = cache.pending().unwrap();
    let archive = directory.path().join("before.sqlite");
    drop(cache);
    {
        // Schema 15 recorded the review conflict and its page on the batch.
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(&format!(
                "PRAGMA foreign_keys=OFF;
                BEGIN;
                CREATE TABLE old (
                    id INTEGER PRIMARY KEY AUTOINCREMENT CHECK(id>0),
                    sealed TEXT,
                    revisions TEXT,
                    attempted INTEGER NOT NULL DEFAULT 0 CHECK(attempted IN (0,1)),
                    conflict INTEGER CHECK(conflict BETWEEN 0 AND 3),
                    space TEXT,
                    CHECK((sealed IS NULL) = (revisions IS NULL)),
                    CHECK(attempted=0 OR sealed IS NOT NULL),
                    CHECK((conflict IS NULL) = (space IS NULL))
                ) STRICT;
                INSERT INTO old SELECT id, sealed, revisions, attempted, 3, '{space}' FROM batches;
                DROP TABLE batches;
                ALTER TABLE old RENAME TO batches;
                PRAGMA user_version=15;
                COMMIT;"
            ))
            .unwrap();
    }
    let cache = Replica::open(&path).unwrap();
    assert_eq!(cache.pending().unwrap(), pending);
    assert_eq!(cache.status(id).unwrap(), Some(EditStatus::Pending));
    cache.export_recovery(&archive).unwrap();
    drop(cache);
    let connection = Connection::open(&path).unwrap();
    let version: u32 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    let columns: Vec<String> = connection
        .prepare("SELECT name FROM pragma_table_info('batches')")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(version, 16);
    assert_eq!(columns, ["id", "sealed", "revisions", "attempted"]);
    drop(connection);
    assert_eq!(notebook::Recovery::open(&archive).unwrap().pending().unwrap(), pending);
    let cache = Replica::open(&path).unwrap();
    let remote = typed(SOURCE, space, text, 0..0, "Remote ");
    let mut server = Server::new(&remote);
    assert!(matches!(
        cache.sync_once(&mut server).unwrap().edit,
        Some((_, EditStatus::Published { .. }))
    ));
    assert!(conflicted(&server.durable, space));
}
