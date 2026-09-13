# notebook

The application-facing crate: notebook discovery, a durable local replica with
reconnect reconciliation, external-asset caching, recovery export and optional
embedded SMB access. The replica stores a complete working image of one section
and the editor's intents in a local SQLite database. `sync_once` provides a
reconciliation step and `start_sync` owns automatic polling and reconnects. Local
success does not acknowledge publication to a shared notebook.

## Saving pages

The editor works on `onestore::page::Page` values and saves whole pages. `save`
diffs the supplied model against the page stored in the supplied local snapshot,
writes the difference into the working image and queues one
`Operation::Page(PageIntent { before, after, author })`; `before` is the page the
edit started from and is the precondition reconciliation checks. Text, styles,
paragraph formatting, hyperlinks (external and internal), bullets, numbering, note tags, table rows, columns, cell shading, nested tables, pictures (insertion in paragraphs or on the page, position, size, description), attachments, ink strokes, equations, paragraph structure,
outline layout, insertions and deletions are all differences between `before` and
`after`; the library never sees editor operations.

```no_run
use onestore::{ExGuid, page::Page};
use notebook::Replica;
# fn example(path: &std::path::Path, source: &[u8], space: ExGuid, edited: &Page)
# -> Result<(), Box<dyn std::error::Error>> {
// `space` identifies the page; `edited` is the editor's current model of it.
let cache = Replica::create(path, source)?;
let snapshot = cache.snapshot()?;
let local_id = cache.save(&snapshot, space, edited, "Author")?;
drop(cache);

let reopened = Replica::open(path)?;
let pending = reopened.pending()?;
assert_eq!(pending.last().map(|edit| edit.id), local_id);
# Ok(())
# }
```

Saves coalesce the way OneNote's own autosave does: while the newest queued intent
for the same page has not been attempted, a later save replaces its `after` model
under the same ID. Once an intent has been attempted or holds a conflict it is
never rewritten, nor is the intent a synchronization step has selected for
publication; the next save then queues a new intent. A save whose model equals
the stored page returns `None`. `PageIntent::text_change` describes an intent that
changes exactly one paragraph's text as the text object, its previous text, the
replaced UTF-16 range and the replacement.

Share one `Replica` between application threads. Each save compares its supplied
snapshot under the cache transaction; stale snapshots return `Io(ResourceBusy)`.
The intent and its resulting image commit together. Keep the cache on a local
filesystem: the connection holds exclusive ownership between transactions, and a
second open fails busy. No network wait occurs in a local save. After a database
error, reopen and inspect the durable state before retrying.

## Sessions

`session::Notebook::open(root, cache_dir)` discovers a notebook directory and
`session::Section::open(file, cache_dir, notify)` opens one section file through
a replica stored under the cache directory, named by the section's document
identity so the same file reopens the same queue after a relaunch. A section
publishes in the background to the file itself under OneNote-compatible
exclusion. `pages()` lists page spaces and titles from the local image,
`page(space)` returns the model to edit (its `identity` and the section's
`identity()` feed `onestore::page::link::internal_link`), and `save(space, before, after,
author)` queues the edited model: `Save::Queued(id)` is durable locally,
`Save::Unchanged` means the model equals the stored page, and `Save::Stale`
means the stored page no longer matches `before` because the section changed
underneath the editor, so the page must be reloaded before saving again.
`events()` drains what the synchronization thread reported since the last
poll: refreshes, attempt outcomes and unreachable files; `notify` runs on that
thread whenever an event is available so the application can wake its event
loop. `close()` stops publication.

`Event::Unreachable` retains an `io::Error`: callers can distinguish permission
denial, missing targets, timeouts, and connection failures through `kind()` without
parsing display text. Document/cache failures are reported as `Event::Failed` and
stop the worker. Durable publication outcomes remain available through `status`.

`Section::resume(file, replica, notify)` starts a session from an owned
`Replica::open(cache_file)` without consulting the remote file. The caller retains
the cache location and publication path for offline relaunch. Local pages and
saves remain available while the target is absent; synchronization verifies the
document identity before adopting or publishing remote content. A different
document stops the worker and retains the pending local edits. `Section::open`
remains the online convenience constructor that discovers the identity from the
file and creates or reopens its cache.

With the `smb` feature, `Section::resume_smb(path, replica, limit, connect,
notify)` binds the same session operations to a share-relative path. `connect`
returns a new SMB client on the synchronization worker and is called again after
transport failure. The caller supplies credentials there, outside the replica
schema; construction and local saving do not wait for a network connection.

## Pages of a section

`create_page` accepts the core `PageCreation` intent and queues its page space
and section entry as one publication. Subsequent saves of the new page use the
intent's stable identities immediately after local acknowledgement. Independent
page additions rebase against the current section order; duplicate titles remain
distinct. An unavailable or no-longer-leading insertion anchor produces
`StructureChanged`. `rebase_page_creation_conflict(id, local, remote, before)`
reviews a replacement anchor against both cache images while retaining the new
page and dependent object identities. Existing page identities require
reconciliation; a matching page alone does not establish a receipt.

`pages(snapshot, edits)` queues a slice of core `PageEdit` intents as one atomic
publication. `PagePosition::Keep` preserves remote movement during indentation-only
edits. Every requested level remains explicit: a competing remote level produces
`StructureChanged` unless it already matches the requested level. Moves compare
the selected page's position relative to surviving observed pages; an unchanged
or already-satisfied position can proceed, and new remote pages remain present.
Indistinguishable competing moves retain a conflict and the complete local image.
`rebase_pages_conflict(id, local, remote, edits)` reviews the batch against both
cache images. Use `PageEdit::reposition(position, level)` on the retained intents
to revise anchors or indentation; replacing page or allocated series identities
is rejected. Dependent edits remain queued. Attempt evidence includes every
changed space plus the receipt space when that space is unchanged. An existing
section revision alone cannot confirm a page edit. The
[offline page corpus](../../corpus/page-lifecycle/offline-edits/README.md)
contains native comparisons, reproduction commands and stateful sanitizer seeds.

`delete_pages(snapshot, pages)` queues the permanent removal of page spaces;
a page already absent remotely produces `TargetUnavailable`.

## Reconciliation

`sync_once(&mut remote)` processes the oldest pending intent through a `Remote`
implementation, returning its ID and `EditStatus`. When the remote page still
equals `before`, the intent publishes as prepared. When the remote page changed,
a three-way merge keeps everything the remote changed and re-applies the local
changes wherever the two sides touched different objects, fields or text ranges:
a local text edit merges with a remote edit elsewhere in the same paragraph, an
outline move merges with a remote text change, a new paragraph survives a remote
deletion elsewhere. Any overlap retains `Conflict(ContentChanged)` together with
the complete local image and the last observed remote image returned by
`remote_snapshot`; a page removed remotely retains `TargetUnavailable`, and a
page the writer can no longer express retains `UnsupportedEdit`.

Publication attempts are recorded before network I/O. A retained attempt
requires its recorded revision to remain present in the remote, or the remote
page to equal the intent's `after` model exactly, followed by comparison,
flushing and refreshed header version metadata before acknowledgement. Otherwise
the attempt remains `AwaitingConfirmation`; an uncertain publication is never
replayed. A durable `Published` receipt survives reopening. Transport errors
return `Error::Remote` or `Error::RemoteIo`; inspect `status(id)` after the error
to distinguish a retained attempt from a pending edit or receipt. The error return
does not roll back a locally acknowledged intent.

The current operation processes one queue head; while edits remain, `snapshot`
preserves the complete local working image. An empty queue can refresh from the
remote image. Synchronization holds a separate owner lock, so local saves can
continue during network waits; competing synchronization calls return `WouldBlock`.

`review_page(id, local, remote, after)` resolves the oldest page conflict with a
model the user reviewed against the current remote page: the intent's `before`
becomes that remote page and its `after` the reviewed model, under the same ID,
in one local transaction. The supplied images must still match `snapshot()` and
`remote_snapshot()`. Review performs no network I/O, clears the conflict to
`Pending` and wakes the worker; publication still reads the latest remote image,
so another overlapping remote edit can produce a new conflict. Pending intents
and uncertain attempts cannot be reviewed.

An attempt that stays `AwaitingConfirmation` is released by review, never by
replay: `release_attempt(id, local, remote, archive, after)` on the replica
(`release` on a session) exports the branch to a new archive, then retires
the oldest edit's attempt. `Some(page)` continues from a page reviewed
against the current remote image as a fresh intent under the same id,
keeping later edits; `None` abandons the local branch, whose ids report
`EditStatus::Archived { archive }` from then on, and the working image
returns to the remote image. Neither writes a receipt: the uncertain
publication may or may not have landed, and the archive is the record of
what was attempted.

Opening recognizes the application identity and the current schema version only;
caches and archives written by other versions are refused unchanged. Until the
application is usable end to end there are no migrations.

With the optional `smb` feature, `SmbRemote::new(client, path, limit)` binds an
`notebook::smb::Client` to one share-relative file and snapshot limit. Remote identity uses the logical root
object space, which survives the tested native compaction that replaces the file ID.

An `Arc<Replica>` can own one background worker. Supply a connection factory, poll
interval and observer; successful publications drain immediately, durable local
edits wake the worker, and `wake()` requests an immediate reachability retry.
Transport failures discard the old connection and retry through the factory;
Read contention and `NotCommitted` operations with `WouldBlock` or `ResourceBusy`
reuse the connection. Contended `NotCommitted` operations use randomized backoff,
capped at one second, to separate competing retry cycles. Cancellation interrupts this delay; local wake notifications
remain coalesced until its end.
`RemoteIo` distinguishes connection/read failures from
local `Io` errors. Cache/document errors stop the worker. Observers receive every
attempt's result on the worker thread, including unchanged conflict/uncertain
statuses; durable edit state remains available through `status`.

```no_run
# #[cfg(feature = "smb")]
# fn example(cache: std::sync::Arc<notebook::Replica>, username: String, password: String)
# -> Result<(), Box<dyn std::error::Error>> {
use notebook::SmbRemote;
use notebook::smb::{Client, Credentials};
use std::time::Duration;

let worker = cache.start_sync(
    Duration::from_secs(2),
    move || {
        Client::connect(
            "server:445", "notes",
            Credentials { username: &username, password: &password, domain: "" },
            Duration::from_secs(5),
        ).map(|client| SmbRemote::new(client, "Personal/Video.one", 64 * 1024 * 1024))
    },
    |result| {
        if let Err(error) = result { eprintln!("{error}"); }
    },
)?;
// Retain `worker` while synchronization should run; local edits wake it automatically.
worker.stop()?;
# Ok(())
# }
```

`stop()` cancels future steps and joins the worker, returning a fatal cache error
or worker panic. Dropping it requests cancellation without waiting. An in-flight
step completes before releasing ownership; a replacement worker cannot start
while the old one still owns the replica. Remote operations and callbacks must
have bounded execution times if shutdown latency matters. A dropped worker can
briefly retain the cache; use `stop()` before requiring an immediate reopen.
Cancellation and application restart retain pending edits and publication attempts.
Credentials belong to the factory, not the cache database.

Creation refuses existing paths. Opening recognizes the application identity and
schema version, validates database integrity and both notebook images, and rejects
unsupported journal modes without converting them. Failed initialization preserves
the file for inspection. SQLite uses DELETE journaling, EXTRA synchronization and
fullfsync; each required setting is queried back.

The cache and its pending intents contain notebook content. The core `onestore`
crate stays independent of SQLite and network runtimes. Device and simulator
examples compile and link for iOS; recorded process-interruption tests do not
establish physical power-loss durability. Evidence is tracked in [Milestone 9](../../evidence/MILESTONE9.md).

The SMB-enabled `smb_offline_client` example is an owned-lab workload for
`tools/native_collaboration.py --offline --embedded-smb`. It separates local
acknowledgements, remote publication attempts, persisted receipts and cache reopen
checks. Its append-specific conflict review policy lives in the test client;
the library continues to preserve conflicts requiring an explicit decision.

## Recovery archives

`export_recovery(new_path)` captures both complete notebook images, the intent
queue, uncertain attempts, conflicts, receipts, downloaded media and the edit-ID sequence in one
SQLite snapshot. It refuses existing destinations and leaves the live queue
unchanged. Export to a local directory from a background thread: copying holds
the cache mutex while capturing the database. Failure after the final rename can
leave a complete archive at the requested path; it never acknowledges a remote
edit. Archives contain notebook content and use a separate database identity, so
`Replica::open` rejects them as writable caches.

```no_run
use notebook::{Recovery, Replica};
# fn example(cache: &Replica) -> Result<(), Box<dyn std::error::Error>> {
cache.export_recovery("review.sqlite")?;
let review = Recovery::open("review.sqlite")?;
let counts = review.summary()?;
let local = review.snapshot()?;
let remote = review.remote_snapshot()?;
let pending = review.pending()?;
let receipts = review.receipts()?;
# Ok(())
# }
```

`Recovery` provides read-only inspection and no synchronization or restore method.
`status(id)` preserves the same state interpretation as the live replica.
`recovery_summary()` on the live replica and `summary()` on an archive return
counts and byte sizes without notebook text, paths, authors or credentials.
Opening an archive validates its schema and images without migration. A recovery
archive is evidence for a reviewed recovery decision, not a second active queue.

## Downloaded media

`fetch_asset(source, section, filename, limit)` resolves a declared external
payload through `notebook::discover::Source` and durably caches its exact bytes.
The section path is relative to the source root; the filename comes from a
`FileDataReference::External` in the retained working or remote image. Local and
SMB sources use the same API. Downloads release the cache mutex during network
I/O and recheck the reference before committing; edits can continue meanwhile.

`cached_asset(filename, limit)` reads previously downloaded bytes without network
access. `None` means never downloaded; `Some(Vec::new())` is a downloaded empty
payload. Each read checks the stored SHA-256 and enforces the byte limit before
loading the payload. Cache contents describe the prior download, not current
server reachability or presence. A failed fetch returns its error without
silently substituting cached bytes. Different bytes for an already cached file
identity return `AssetChanged` and preserve the previous download.

Downloads do not change pending edits, publication attempts or receipts. Recovery
archives include cached media and expose the same bounded `cached_asset` lookup.

## Discovery

`notebook::discover` provides read-only notebook discovery over a caller-supplied
root, keeping directory traversal out of the single-file storage parser.

Discovery returns ordered sections and nested groups with file identities and
share-relative paths. Section display-name overrides remain distinct from file
names. TOC references whose identities are absent from the directory remain
inspectable; a cached filename never substitutes for an identity match.
Reserved `_onefiles` directories are excluded from section-group traversal;
OneNote's `OneNote_RecycleBin` is listed as the section group OneNote shows.
Encrypted sections and valid storage with an unreadable document graph retain
their identity as `Locked` or `Unreadable`, without being presented as empty pages.
Malformed storage, failed reads and ambiguous identities reject the discovery.

Each file read must be a consistent, bounded snapshot. The result is an
observation across multiple files, not an atomic notebook transaction or
authorization to publish an edit. Refresh rejects observed topology changes and
duplicate physical files with the same logical identity. Retain the last accepted
catalog if discovery fails; a connection failure does not mean files were deleted.

`read_external_asset` resolves a validated UUID `.onebin` filename beneath the
selected section's sibling `_onefiles` folder and returns exact bounded bytes.
Missing files, permissions and size failures retain their I/O error kinds; an
empty payload is a successful empty buffer. Embedded payloads remain available
directly from the core document model.

## Notebook structure

`Notebook::create_section`, `create_group`, `rename`, `set_section_color`,
`reorder` and `delete` change a notebook the way OneNote does: the table of
contents (`Open Notebook.onetoc2`, created when a folder has none) gains,
renames, reorders or loses entries; a section's colour lives in its own
metadata; a deleted section moves into `OneNote_RecycleBin`, a group with its
own TOC. Every created, renamed or moved file is placed with
`onestore::place_file`, which sets the header's ancestor to the parent TOC's
identity and the name CRC OneNote checks on open; a file without them is
re-identified and listed anew. They run over `Storage`: a mounted directory
(`Notebook::open`) or an SMB share (`Notebook::open_smb` with a
`smb::Client`, which gained create, directory creation, rename, delete and
header placement under native writer coordination); `tools/test_smb_structure.py
VM OUTPUT` drives them against a disposable Samba lab VM.

`Notebook::find_page(url)` resolves a stored internal link to a section path
and page space by identity: the linked section first, then every readable
section, so a link follows its page when the section is renamed or moved and
when the page itself was moved to another section. Other URLs and unknown
targets are `None`.

With the optional `protected` feature, `Notebook::unlock(path, password)`
reads the pages of a `Locked` section for display; nothing is cached or
written, and a wrong password is `Error::Protected(PasswordMismatch)`.

`Section::import_page(page, author)` copies a page, usually read from another
section, to the end of this one as a page creation and a save queued like the
user's own edits, under fresh identities (`Page::copy`); payloads travel with
the model, so the copy publishes offline later like any save. Content outside
the model refuses to copy. A move is an import here followed by
`Section::delete_pages` there.

`Notebook::refresh` rereads the directory on the caller's schedule and reports
what another client changed as `Change`s keyed by file identity: a renamed or
moved section is `Moved`, not removed and added, and a deleted section moves
into `OneNote_RecycleBin`; a folder whose surviving entries changed sequence
is `Reordered`. A failed read returns the error and keeps
the previous catalog, so an unreachable share never reads as an emptied
notebook.

## SMB (feature `smb`)

`notebook::smb` provides blocking SMB access for OneNote sections and
table-of-contents files. The core `onestore` crate remains independent of network runtimes. This is an
experimental Rust API with native interoperability evidence in the repository's
[Milestone 9](../../evidence/MILESTONE9.md).

```no_run
use notebook::smb::{Client, Credentials};
use std::time::Duration;

let client = Client::connect(
    "server:445",
    "notes",
    Credentials { username: "user", password: "password", domain: "" },
    Duration::from_secs(5),
)?;
let snapshot = client.read("Personal/Video.one", 64 * 1024 * 1024)?;
let store = onestore::Store::parse(&snapshot)?;
let revisions = onestore::RevisionIndex::parse(&store)?;
let document = onestore::document::Document::parse(&revisions)?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

Paths are relative to the share. The read limit bounds the complete physical
snapshot. Call from a background thread outside a Tokio runtime. Use identities
from the document and the same snapshot with `Client::commit_text` or
`Client::commit_property_bytes`; their errors retain `onestore::CommitState`.
`PreparedEdit::page` separates preparation from I/O: inspect the immutable image
and persist the intended revision identity before `Client::commit_prepared`.
`Client::confirm_snapshot` compares and flushes an observed image, then refreshes
its header version metadata without adding a revision. The caller must first
establish which intents that image contains and reread before another commit.

Readers use shared native guards while writers publish under native write-open
and byte-lock exclusion. Maintenance is excluded during each operation; pathname
identity is checked after acquiring the guards. Connection loss retires the
client. Reconnect for subsequent operations, and reconcile an `Unknown` edit
before retrying it. The transport does not automatically replay requests.

`Client::read_dir(path, entry_limit)` enumerates a directory, including the share
root with an empty path. It follows every response page and returns no partial
list on interruption, entry-limit overflow or close failure. Entries retain exact
Unicode names, observed sizes and MS-FSCC attributes, including directory/reparse
flags. Concurrent directory changes are not an atomic snapshot; repeated names
are rejected with `ResourceBusy`. Notebook identities come from the files, not
directory names or sizes. Missing paths, denied access and non-directory paths
have distinct I/O error kinds.

`Client::read_asset(path, byte_limit)` reads an external payload under a read-only
share handle that excludes writes and deletion. Empty files succeed; limits,
interrupted reads and failed close never return partial bytes. This payload read
does not parse a OneStore header or acquire its reader-coordination bytes.

`python3 tools/test_smb_directory.py VM OUTPUT` checks a caller-owned disposable
Linux lab VM against its filesystem listing and interrupts directory requests,
responses and close. It creates synthetic files in that VM; the caller retains
responsibility for VM teardown. The parser also has bounded-record/truncation
tests independent of the server.

Device and simulator builds link for iOS. Native acceptance uses disposable
OneNote 2010 clients and Samba; it does not establish on-device execution or
physical power-loss durability.

## Queue measurement

`cargo run -p notebook --release --example queue_scale -- NEW_DIRECTORY 1000`
measures alternating-page saves that retain separate queue IDs, cache reopen,
recovery export and local-file publication. JSON lines record each acknowledgement
and publication latency plus image/cache sizes. The workload verifies every ID
and both final page titles, then independently reopens the recovery archive.
Use an external resource monitor for peak memory; these local-file timings do not
measure SMB or phone performance.
