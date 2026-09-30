# notebook

The application-facing crate: notebook discovery, a durable local replica with
reconnect reconciliation, external-asset caching, recovery export and optional
embedded SMB access. A replica keeps one section's queue in a local SQLite database:
the image the queued edits apply to (the base, in 16 KiB chunks), the edits as
`onestore::op` ops in publication batches, and picture and attachment bytes once
each by hash. Local success does not acknowledge publication to a shared notebook.

```text
editor ── Edit ──► section thread ─ Section::apply, INSERT edit ─┐
                   (the parsed section: base + sealed + open)     │ one fsync per burst
sync thread: remote.stamp() == base ? publish(sealed batch) ──────┤ base chunks += transaction
             otherwise read once, replay the queue on it ─────────┘ base := remote, conflict pages queued
```

## Edits

`Replica::apply(author, edit)` applies an `onestore::op::Edit` to the section the
queue leaves and returns its id once written; a refused edit returns `Rejected`
and changes nothing. `page(space)` and `pages()` read what the queue leaves, for
opening and reloading. Edits collect in an open batch until the sync thread seals
it, so edits arriving while a publication is in flight publish together.

```no_run
use onestore::{ExGuid, op::{Edit, Op, PageOp}};
use notebook::Replica;
# fn example(path: &std::path::Path, source: &[u8], space: ExGuid, text: ExGuid)
# -> Result<(), Box<dyn std::error::Error>> {
let cache = Replica::create(path, source)?;
let typed = PageOp::Text { text, range: 0..0, with: "Hello ".into() };
let id = cache.apply("Author", Edit { at: 133_000_000_000_000_000, ops: vec![Op::Page { space, op: typed }] })?;
drop(cache);

let reopened = Replica::open(path)?;
assert_eq!(reopened.pending()?.last().map(|edit| edit.id), Some(id));
# Ok(())
# }
```

Page reads never wait for the sync thread: a rebase or reread of the queue runs on a
new thread while the current one keeps answering `page` and `pages` from the section as
it was, then hands its requests over; edits sent meanwhile apply to the rebuilt section.

Share one `Replica` between threads. Keep the cache on a local filesystem: the
connection holds exclusive ownership between transactions, and a second open fails
busy. No network wait occurs in a local edit. After a database error the section
thread rereads the queue.

## Sessions

`session::Notebook::open(root, cache_dir)` discovers a notebook directory, reading only
the files its folders list otherwise than when last opened (`discover::Cache`, kept under
`cache_dir/listings`), and
`session::Section::open(file, cache_dir, notify)` opens one section file through
a replica named by the section's document identity, so the same file reopens the
same queue after a relaunch. A section is `Send + Sync`: `apply(author, edit)`
returns at once and reports a refusal as `Event::Rejected`; `events()` drains
remote changes (`Changed(spaces)`), publication outcomes, unreachable files and
failures; `notify` runs on a background thread whenever an event waits.
`conflicts()` lists each page's conflict pages, which `page` reads and `delete_pages`
removes; `release(id, archive, Resolution)` ends an uncertain attempt. The author an
edit names is the host's: the app passes the account's full name.
`import_page` creates a page holding a copy of another; `delete_pages` removes pages.
`sync_status()` gives when the section file was last reached, why it could not be since
and how many edits wait for it. `set_offline(true)` works offline as OneNote does: the
worker stops connecting and edits queue until `wake()` (Sync Now) or `set_offline(false)`.

`session::Background` keeps the sections no session holds in sync, as OneNote 2010 keeps
every section of an open notebook: `Background::smb(root, limit, connect, notify)` on a
share, `Notebook::background(watched, copies, notify)` for a mounted notebook, then
`watch(notebook.replicas())`. On a share, one CHANGE_NOTIFY on the notebook's folder
(`smb::Client::watch`) reports what changed; a host watching a mounted folder passes the
changed paths to `touched`, and says whether it reports every change as `watched`. A
reported section is checked a second later; a reported folder is listed a second later and
only its sections listed otherwise are checked. A failing section is tried every 31 seconds,
and any other every `Background::BACKSTOP` while a watch reports, `UNWATCHED` otherwise. On
connecting, and on working online again, every folder is listed: a section whose file lists as
discovery or the last check found it needs no reading, the others are checked one at a time
100 ms apart. A check reads the file's stamp; a section whose replica has edits waiting, or
whose file moved past the replica's base, has the replica opened for the synchronization steps
that publish or rebase it and closed again, and a section without a replica gets one from the
file, its offline copy, on a share or with `copies`. `hold(path, section)` leaves an open
section to its session: its worker no longer polls while a watch reports, the watch wakes it
instead, and once its replica is released the background checks the file at once. A replica
a session holds is otherwise skipped (`Error::busy`), so opening a section may wait out one
step. `status()` gives each section's `SyncStatus`, `changed()` the sections another client
changed, `wake` and `set_offline` follow Sync Now and Work Offline, and `discard` stops the
thread and deletes the replicas holding nothing unpublished, as when the notebook closes.
`Notebook::replica_path(path)` names a section's replica for either kind of notebook.

`Section::resume(file, replica, notify)` starts from an owned `Replica` without
consulting the remote file; with the `smb` feature, `Section::resume_smb(path,
replica, limit, connect, notify)` binds a share-relative path, `connect` running on
the worker again after transport failure.

## Reconciliation

`sync_once(&mut remote)` returns what one step did as `Synced { edit, changed }`:
the state the step's batch reached, named by its newest edit, and the pages a remote
change replaced. While the remote's stamp (header and length) equals the base's, the
oldest sealed batch publishes as its `Transaction`, and the base's chunks take its
writes; nothing is read. When the stamp moved, the remote image is read once and the
queue replays on it (`merge.rs`): an op whose objects the remote left alone applies
as it is; a text op shifts past the remote's changes to the same text when its
range stays clear of them; a move, deletion or property the remote already made is
done; a page the remote already holds as the local edits leave it drops their ops.
Anything else conflicts, as in OneNote 2010: the op is dropped with every later op naming
what it named, the remote's version stays the page, and the local version becomes a
conflict page under it (`SectionOp::Conflict`, queued as one more edit and published with
the rest), its conflicting objects marked, named for the author of the first edit that did
not replay. Page lists merge as OneNote 2010 merges page series: a page the remote moved
keeps the remote's place, a page moved here goes before the next page in the local order
the remote left in place, or last, and a page the remote removed comes back as a new copy
of the local one, placed the same way (`corpus/conflict-page/native-pages`,
`native-restore`). Nothing blocks the queue.

Publication attempts are recorded before network I/O. An attempt confirms only
when the remote holds its revisions, or every page it changed as it changed them,
followed by `Remote::confirm`; otherwise it stays `AwaitingConfirmation` and is never
replayed. `release(id, archive, Resolution)` exports the queue first, then
publishes the batch again (`Mine`) or abandons every unpublished edit (`Theirs`).
A durable `Published` receipt survives reopening.

A schema-14 cache is converted when opened: it is exported to
`<cache>.v14-recovery`, each queued page is lowered against the page the conversion
has so far, and every converted page must equal the page in the old working image,
or the conversion rolls back and the open fails naming the archive; a page it held in
conflict becomes a conflict page. A schema-15 cache's batches lose the review conflict
they recorded when opened.

With the optional `smb` feature, `SmbRemote::new(client, path, limit)` binds an
`notebook::smb::Client` to one share-relative file and snapshot limit. Remote identity uses the logical root
object space, which survives the tested native compaction that replaces the file ID.

An `Arc<Replica>` can own one background worker. Supply a connection factory, poll
interval and observer; successful publications drain immediately, durable local
edits wake the worker, and `wake()` requests an immediate reachability retry.
While nothing is queued, or the queue waits on a remote that has not changed since,
a remote whose `Remote::stamp` holds is not read again; a worker a `Background` holds reads
the stamp only when the notebook's watch wakes it.
Transport failures discard the old connection and retry through the factory;
Read contention and `NotCommitted` operations with `WouldBlock` or `ResourceBusy`
reuse the connection. Contended `NotCommitted` operations use randomized backoff,
capped at one second, to separate competing retry cycles. Cancellation interrupts this delay; local wake notifications
remain coalesced until its end.
`RemoteIo` distinguishes connection/read failures from
local `Io` errors. Cache/document errors stop the worker.

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
or worker panic. Dropping it requests cancellation without waiting. Remote
operations and callbacks must have bounded execution times if shutdown latency
matters. Credentials belong to the factory, not the cache database.

Creation refuses existing paths. Opening validates database integrity and replays
the queue on its base. SQLite runs in WAL mode under exclusive locking with FULL
synchronization and fullfsync, each queried back: every commit is durable, and the only
file beside the cache is `<cache>-wal`, which holds committed pages until a checkpoint
and must travel with the cache when it is copied. Recovery archives are single files.
The cache contains notebook content.

The SMB-enabled `smb_offline_client` example is an owned-lab workload for
`tools/native_collaboration.py --offline --embedded-smb`; its append-specific
conflict policy lives in the client.

## Recovery archives

`export_recovery(new_path)` captures the base and remote images, the edit queue,
uncertain attempts, receipts, downloaded media and the edit-ID sequence in one
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
Dot files and folders (`.DS_Store`, AppleDouble `._` companions, `.snowbound`) and Office's
`~$` owner files are skipped wherever they are, never read as sections.
Encrypted sections and valid storage with an unreadable document graph retain
their identity as `Locked` or `Unreadable`, without being presented as empty pages.
A child folder or section file that is denied or gone while listing (a folder
another client holds delete-pending reads as denied) is kept as `unavailable` and
tried again on the next discovery; the root itself, malformed storage, other failed
reads and ambiguous identities reject the discovery.

`Cache::discover` keeps each file's listing (size and last write time), stamp and what
discovery took from it; the next discovery reads only the files listed otherwise, taking one
from a copy where `Source::copy` has it as it stands (on a share, a replica whose base has the
file's stamp), and `found(path)` gives a section's listing and stamp for `Background::watch`.
`take` hands on, once, the sections it read from the source, up to a memory bound, which
`Notebook::replicas` gives `Background::watch` so that its first check need not read them
again. A file written during discovery is read again next time; a failed discovery leaves the
cache as it was.

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

`Notebook::create`, `create_section`, `create_group`, `rename`, `move_entry`,
`set_section_color`, `reorder` and `delete` change a notebook the way OneNote does: the
table of contents (`Open Notebook.onetoc2`, created when a folder has none) gains,
renames, reorders or loses entries; a new notebook holds "New Section 1", and a new
section an empty section's file plus the page its `PageCreation` makes, in OneNote's
new-section colour order; a section's colour lives in its own metadata; a deleted
section moves into `OneNote_RecycleBin`, a group with its own TOC, and a deleted group's
sections move there too before its folders go. `recycle_pages` keeps copies of pages a
section is about to delete in `OneNote_RecycleBin/OneNote_DeletedPages.one`, with their
identities, titles, dates and creation times, as OneNote does
(`corpus/notebook-management`). A bin or deleted-pages file its TOC does not list is
placed and listed again, a bin whose TOC OneNote named otherwise keeps it, and an
unavailable bin refuses the delete (`corpus/recycle-bin-repair`). An entry a TOC still
lists for a file gone from its folder gives way to the file an edit gives its name.
Edits keep entries in the order their ordering numbers give; a rename or colour keeps
the numbers. Every created, renamed or moved file is placed with
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

`Notebook::read_section(path)` reads a section file as stored, without opening a
replica, and `session::stored_pages(image)` builds its pages with each page's
`LastModifiedTime`: the app's search indexes the sections it has not opened this way.

With the optional `protected` feature, `Notebook::unlock(path, password)`
reads a `Locked` section as `Unlocked { pages, .. }`, and
`Notebook::apply_unlocked(path, password, &mut unlocked, author, edit)`
applies page ops and stores them under the section's key, straight to the file:
nothing of a protected section is cached or queued, a section written since
`unlocked` was read refuses the edit, and a wrong password is
`Error::Protected(PasswordMismatch)`.

`Section::import_page(page, author)` copies a page, usually read from another
section, to the end of this one as one `SectionOp::Import` queued like the
user's own edits, under fresh identities (`Page::copy`); payloads travel with
the model, so the copy publishes offline later like any edit. Content outside
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
# #[cfg(feature = "smb")] {
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
# }
# Ok::<(), Box<dyn std::error::Error>>(())
```

Paths are relative to the share. The read limit bounds the complete physical
snapshot. Call from a background thread outside a Tokio runtime.
`Client::commit_transaction` publishes a `Transaction` from `Section::seal`; its errors
retain `onestore::CommitState`. `Client::stamp` reads a file's header and length in one
round trip without coordination, for polling. `Client::confirm` checks and flushes a
stamp, then refreshes its header version metadata without adding a revision. The caller
must first establish which intents that image contains and reread before another commit.

A commit is eight round trips while its appended bytes and patches fit fifteen 64 KiB
writes: open; the coordination locks, identity queries and the stamp check's reads as one
compound request; the path's identity; the appended bytes and patches, the header, the
counter (twice at a carry) and the version, each as its writes and a flush in one
compound; close. The server
performs a compound in order and answers its flush once the writes are durable, so the
writes carry no write-through flag.

Readers use shared native guards while writers publish under native write-open
and byte-lock exclusion. Maintenance is excluded during each operation; pathname
identity is checked after acquiring the guards. Connection loss retires the
client. Reconnect for subsequent operations, and reconcile an `Unknown` edit
before retrying it. The transport does not automatically replay requests.

`Client::read_dir(path, entry_limit)` enumerates a directory, including the share
root with an empty path. It follows every response page and returns no partial
list on interruption, entry-limit overflow or close failure. Entries retain exact
Unicode names, observed sizes, last write times and MS-FSCC attributes, including
directory/reparse flags. Concurrent directory changes are not an atomic snapshot; repeated names
are rejected with `ResourceBusy`. Notebook identities come from the files, not
directory names or sizes. Missing paths, denied access and non-directory paths
have distinct I/O error kinds.

`Client::watch(path, changed)` arms one CHANGE_NOTIFY with WATCH_TREE on a directory, as
OneNote 2010 watches a notebook's folder, and keeps it armed on the client's runtime without
probing the connection, so an idle watch sends nothing. `changed` hears each batch of changed
paths relative to the directory, `""` when the server lost count, and an error when the watch
ends with its connection.

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
measures alternating-page edits, cache reopen, recovery export and one local-file
publication of the queue. `keystroke_probe PARAGRAPHS KEYSTROKES` types into a large
page and reports the bytes the cache and the section file write per keystroke.
