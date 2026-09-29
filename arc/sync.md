# The data layer and sync

OneNote 2010 got multi-user editing without a server. A notebook is a folder
on a file share. Every client edits the section files in place, and the
format's append-only revisions plus some careful file locking let clients
merge each other's work. Snowbound joins that arrangement as one more peer. It
has no service, no account and no sync protocol of its own. The share is the
source of truth, and whatever OneNote can do to a file while Snowbound is
using it, Snowbound has to handle.

This essay follows an edit from the keyboard to the share, then covers what
happens when the share is busy, changed or gone.

## Three threads

```text
UI thread (winit, UIKit)      section thread                    sync thread
────────────────────────      ──────────────────────────────    ──────────────────────────────
editor emits Edit (ops) ───►  apply to the parsed Section       poll the stamp (header + length)
  and returns at once         write each burst to SQLite          unchanged: ask for a seal,
page reads ◄────────────────  answered from the Section                      publish it
events ◄────────────────────  Changed, Rejected, published        changed:   read once, ask
                              seal ◄──────────────────────────               for a rebase
                              rebase onto the new image ◄─────   acknowledge: base += transaction
```

- **The UI thread only emits ops.** The editor records the ops each change
  lowers to as it makes the change. Submitting them to the session returns
  immediately. Parsing, revision building and SQLite never run on a frame.
- **The section thread** (`notebook::working`) is the only owner of the parsed
  `onestore::Section`. It applies edits, writes each burst of them to the
  replica in one durable commit, and answers page reads. A rebase runs on a
  fresh thread that builds its own section while the old one keeps answering
  reads, so opening a page never waits on the network.
- **The sync thread** (`notebook::worker`, stepping `notebook::sync`) does all
  network I/O. It polls, publishes and rereads. When the queue is idle it
  reads nothing but the header.

`notebook::session` wraps this up for apps. `Notebook` covers discovery and
structure, `Section` covers one section's pages, edits, events and conflict
pages. Both desktop and iOS use exactly this surface.

## The replica

Each open section has a replica: a SQLite database in the app's cache,
identified by the section's logical identity, so the same file reopens the
same queue after a relaunch. It holds:

- **the base**: the last remote image the queue applies to, stored in chunks so
  that acknowledging a publication rewrites only the chunks the transaction
  touched;
- **batches of edits**: each edit's ops, serialized, grouped into the batch
  that will publish together;
- **payloads**: picture and attachment bytes, stored once each, keyed by hash.

The working state is never stored. It is the base with each sealed batch
replayed and the open batch applied, rebuilt on open. The database runs in WAL
mode with full synchronous commits, and every setting is read back to check it
took.

An attached file is a payload like a picture: its bytes ride in the edit that
inserts it and publish inside that edit's revision. No other file is written,
so nothing needs to reach the share before the revision that names it. That
ordering would matter only for `_onefiles` payloads, which OneNote 2010 never
writes ([file format](file-format.md)). A large file makes a publication as
large as itself.

A keystroke that publishes immediately costs three commits: the edit, the seal
(which also records the publication attempt), and the receipt. Each one is
ordered against something outside the cache. The edit must be durable before
`apply` answers. The attempt must be durable before any byte reaches the
share, because an attempt that might have landed is never replayed blindly.
The receipt follows the file's own commit. Merging any two of them would open a
window where a crash forgets or duplicates work.

## A sync step

```text
stamp = remote.stamp()                          one small, uncoordinated read
if stamp == base.stamp:
    publish the sealed batch (if any)           Ok ─► acknowledge
                                                NotCommitted ─► retry later
                                                Unknown ─► attempted; confirm before anything else
else:
    image = remote.read()                       coordinated snapshot, only now
    rebase the queue onto image                 merge, maybe conflict pages
    base := image
```

A batch stays open while the sync thread is busy, so keystrokes that arrive
during one round trip publish together in the next. Polling costs a
kilobyte-sized read. A publication costs one appended revision, not the
section.

Uncertainty is handled explicitly. A publication attempt is recorded before
the first network byte. If the reply is lost, the attempt confirms only when
the remote holds its revisions (or every page it changed, as it changed them).
Otherwise it waits as `AwaitingConfirmation`, and `release` resolves it
explicitly: it exports a recovery archive first, then republishes or abandons.

## Merging

When the stamp has moved, someone else committed. The queue replays on the new
image one op at a time (`notebook::merge`):

- an op whose objects the remote left alone applies as it is;
- a text op on text the remote also changed shifts past the remote's changes,
  provided its range stays clear of them;
- a move, deletion or setting the remote already made is dropped as done;
- anything else conflicts.

Conflicts follow OneNote 2010 exactly, because the other clients are OneNote.
The conflicting op is dropped, along with every later op that names what it
named. The remote's version stays the page. The local version becomes a
read-only **conflict page** under it, with the conflicting objects marked and
the page labelled with its author, so OneNote and Snowbound both show the
same bar, the same highlighted paragraphs and the same Delete Conflict Page. A
conflict never blocks the queue: it becomes one more queued edit. Page-list
edits merge the way OneNote merges page series (a page someone moved keeps
their placement, and a page deleted remotely but edited locally comes back as a
copy). Each rule was checked against a capture of two real OneNote clients
doing it first (`corpus/conflict-page`).

## Why an embedded SMB client

OneNote doesn't use lock files. It coordinates through the section file
itself, with share modes on open and one-byte locks far past the end of the
data:

| Role | Open | Byte locks |
| --- | --- | --- |
| reader | read, sharing read/write/delete | shared lock on the reader byte, held for the read |
| writer | read/write, denying other writers | the reader byte shared, plus an exclusive writer byte |
| maintenance (Optimize) | as a writer | an exclusive range that covers the reader byte |

Maintenance rewrites the file under a new name and renames it into place, so a
handle to the old file goes stale even though its contents still parse.

Snowbound has to take exactly these locks, or OneNote will either trample it
or be locked out. macOS's `smbfs` can't express them. POSIX byte-range locks
are unsupported. `flock` of either kind becomes an exclusive lock over the
whole file, which fails OneNote's reads. The only exclusive-byte primitive is
private and has no shared form. On top of that, `smbfs` serves reads from its
own lease-backed cache, defers closes, writes whole cached pages back on
`fsync`, and leaves AppleDouble `._` files beside sections it writes. On iOS
an app sees a share only through Files, with no control over locks.

So `notebook::smb` speaks SMB2 itself (on the `smb2` crate's message layer)
and takes OneNote's opens and bytes exactly, without leases. It never denies
OneNote a read. The leases matter because a Windows client holding a lease
keeps its byte locks local and only reveals them when another open breaks the
lease. So a peer must open before it locks, and never hold handles between
operations. A commit is a handful of compound requests: open; take the locks,
check identity and stamp; write appended bytes and patches, then flush; write
the header, the counter and the version, each flushed in order; close.

A notebook on a share that macOS has mounted is opened through the embedded
client with the account the system keeps for that mount. It falls back to the
mount only when it can't sign in that way.

A file's identity is its root object space, not its server file ID, which
changes when maintenance replaces the file. Polling opens by path each time
for the same reason.

## Offline

Offline is just a sync step that fails. Edits keep landing in the replica, and
the sync thread retries with backoff and wakes on new local edits. A notebook's
last listing lets it open while the server is unreachable, and a section opens
from its replica. On reconnect the queue publishes, or rebases and publishes,
through the same path as always. A failed directory read keeps the previous
catalog, so an unreachable share never looks like an emptied notebook.
Working offline on purpose, as OneNote's Work Offline does, is the same state
chosen: the sync thread stops stepping until Sync Now or until the user works
online again.

## Sections that aren't open

OneNote keeps every section of an open notebook in sync, not just the one on screen: it
reads a section another client changed within seconds and publishes a closed section's
offline edits the moment the share is back. Snowbound's `session::Background` does the
same with one thread per notebook. Each round reads every section file's stamp. Only a
section whose replica has edits waiting, or whose file moved past the replica's base,
has its replica opened for the usual sync steps, and it is closed again afterwards. The
open section is left to its own session.

## Notebook structure

Sections, groups and the notebook's own colour live in the `.onetoc2` files,
which are revision stores edited through the same transaction path. Moving or
renaming a file also rewrites two header fields OneNote checks on open (the
parent TOC's identity and a CRC of the file's name). Without them OneNote
treats the file as a stranger and re-identifies it. Deleting sends sections and
pages to `OneNote_RecycleBin`, as OneNote does.

## Recovery and migration

`export_recovery` snapshots a replica into a single read-only archive: base and
remote images, queue, attempts, receipts and media. It is the evidence behind
any decision that could lose work. Cache schema conversions check their own
output: every converted page must equal the page the old cache held, or the
conversion rolls back and names the archive it made first.

The [notebook README](../crates/notebook/README.md) documents the API in detail.
