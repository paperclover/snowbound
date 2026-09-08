# onestore-offline

Durable local editing for a OneNote file, in an optional sibling crate. The current
foundation stores a complete working image and typed text, insertion and formatting intents in a local
SQLite database. `sync_once` provides a reconciliation step and `start_sync` owns
automatic polling and reconnects. Local success does not
acknowledge publication to a shared notebook.

```no_run
use onestore::ExGuid;
use onestore_offline::Replica;
# fn example(path: &std::path::Path, source: &[u8], space: ExGuid, text: ExGuid)
# -> Result<(), Box<dyn std::error::Error>> {
// `space` and `text` are identities from the supplied section's document model.
let cache = Replica::create(path, source)?;
let snapshot = cache.snapshot()?;
let local_id = cache.edit_text(&snapshot, space, text, 0..0, "Offline edit ")?;
drop(cache);

let reopened = Replica::open(path)?;
let current = reopened.snapshot()?;
let pending = reopened.pending()?;
assert_eq!(pending.last().map(|edit| edit.id), local_id);
# Ok(())
# }
```

`insert` accepts the core library's `Insertion` value and durably retains its
intent identities. `Insertion::with_formatting` queues text and
its styles as one intent and one publication. Keep that value across retries; its `text_object()` identifies
the new text for subsequent offline edits. Pending entries expose
`Operation::Text(TextEdit)`, `Operation::Insert(Insertion)` or
`Operation::Format(FormatEdit)` through their
`operation` field. Synchronization applies these in queue order, so an inserted
outline can precede its paragraphs and their later edits. Missing anchors or
existing insertion identities preserve a conflict and the complete local image.

`rebase_paragraph_conflict(id, local, remote, parent, before)` and
`rebase_outline_conflict(id, local, remote, page, x, y)` accept reviewed replacement
placements for the oldest insertion conflict. They preserve creation time, text,
author and object identities, keeping later edits attached to their original targets.
The images must still match the cache; the new placement must be valid in the remote
image. Paragraph intents cannot become outlines or vice versa. Pending edits and
uncertain publication attempts cannot be repositioned through conflict review.

```no_run
use onestore::{ExGuid, Insertion, TextAttribute};
use onestore_offline::{EditStatus, Replica};
# fn add_outline(cache: &Replica, space: ExGuid, page: ExGuid)
# -> Result<(), Box<dyn std::error::Error>> {
let outline = Insertion::outline(page, 144.0, 216.0, "Offline outline", "Author")?
    .with_formatting(0..7, &[TextAttribute::Bold(true)])?;
let id = cache.insert(&cache.snapshot()?, space, &outline)?;
if let Some(id) = id {
    // A running worker may already have advanced this state.
    match cache.status(id)? {
        Some(EditStatus::Published { revision }) => println!("{revision}"),
        state => println!("{state:?}"),
    }
}
# Ok(())
# }
```

`format` accepts the core `TextAttribute` slice and a UTF-16 range. Its durable intent
retains the observed text and selected attribute values. Remote text changes must
leave an unambiguous range; independent remote attributes merge, while competing
values preserve `FormattingChanged`. A remote value that already matches the
requested value is accepted. Enabling superscript or subscript also checks the
opposite attribute that the operation clears. If the remote image already satisfies
the whole operation, guarded confirmation still precedes a durable receipt.

Recognized version-one through version-three caches migrate transactionally to the typed
queue. The migration retains images, local IDs, publication attempts, conflicts,
receipts and the autoincrement sequence; it does not reuse acknowledged IDs when
the pending queue is empty.

Share one `Replica` between application threads. Each edit compares its supplied
snapshot under the cache transaction; stale snapshots return `Io(ResourceBusy)`.
The intent and its resulting image commit together. No-op edits return `None`.
Keep the cache on a local filesystem: the connection holds exclusive ownership
between transactions, and a second open fails busy. No network wait occurs in a
local edit. After a database error, reopen and inspect the durable state before
retrying.

`sync_once(&mut remote)` processes the oldest pending edit through a `Remote`
implementation, returning its ID and `EditStatus`. A durable `Published` receipt
survives reopening. Publication attempts are recorded before network I/O; a retained
attempted revision requires comparison, flushing and refreshed header version
metadata before acknowledgement. If a formatting attempt's revision is missing,
the complete requested effect can instead be confirmed on a uniquely aligned
range; its receipt identifies that confirmed current revision. Otherwise the
missing attempt remains `AwaitingConfirmation`. Neither path replays an uncertain
publication. Overlapping or ambiguous edits retain `Conflict` status, their complete
local image and the last observed remote image returned by `remote_snapshot`.
Transport errors return `Error::Remote` or `Error::RemoteIo`; inspect `status(id)`
after the error to distinguish a retained attempt from a pending edit or receipt.
The error return does not roll back a locally acknowledged intent.

Rebasing accepts only character mappings shared by every minimum insertion/deletion
alignment. UTF-16 ranges must preserve Unicode scalar boundaries. A bounded
alignment search also leaves a conflict when it cannot establish a unique mapping.
The current operation processes one queue head; while edits remain, `snapshot`
preserves the complete local working image. An empty queue can refresh from the
remote image. Synchronization holds a separate owner lock, so local edits can
continue during network waits; competing synchronization calls return `WouldBlock`.

`rebase_conflict(id, local, remote, range)` lets a caller explicitly place the
oldest conflicting text or formatting intent at a reviewed UTF-16 range in the remote image.
The supplied images must still match `snapshot()` and `remote_snapshot()`.
It preserves the original replacement or requested attributes, intent ID, complete local working image
and every later intent; the selected range and remote paragraph become the
intent's new comparison base in one local transaction. Formatting also captures
the reviewed attribute values as its new precondition. This operation performs
no network I/O, clears the conflict to `Pending`, and wakes the worker.
Publication still reads the latest remote image and uses exact guarded comparison;
another overlapping remote edit can produce a new conflict. An uncertain attempt
cannot be rebased, and selecting text that already equals the replacement does
not create a publication acknowledgement.

With the optional `smb` feature, `SmbRemote::new(client, path, limit)` binds an
`onestore_smb::Client` to one share-relative file and snapshot limit. Remote identity uses the logical root
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
# fn example(cache: std::sync::Arc<onestore_offline::Replica>, username: String, password: String)
# -> Result<(), Box<dyn std::error::Error>> {
use onestore_offline::SmbRemote;
use onestore_smb::{Client, Credentials};
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

`export_recovery(new_path)` captures both complete notebook images, the typed
queue, uncertain attempts, conflicts, receipts, downloaded media and the edit-ID sequence in one
SQLite snapshot. It refuses existing destinations and leaves the live queue
unchanged. Export to a local directory from a background thread: copying holds
the cache mutex while capturing the database. Failure after the final rename can
leave a complete archive at the requested path; it never acknowledges a remote
edit. Archives contain notebook content and use a separate database identity, so
`Replica::open` rejects them as writable caches.

```no_run
use onestore_offline::{Recovery, Replica};
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
payload through `onestore-notebook::Source` and durably caches its exact bytes.
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
Opening older live caches migrates them transactionally to schema 5; original
schema-4 archives remain readable without migration and contain no media cache.
