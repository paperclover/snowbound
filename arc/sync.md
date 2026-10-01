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
editor emits Edit (ops) ───►  apply to the parsed Section       read the stamp (header + length)
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
  network I/O. It reads the stamp when a local edit waits or the notebook's watch
  reports the file (every two seconds where nothing watches), then publishes or
  rereads. Idle, it reads nothing.

`notebook::session` wraps this up for apps. `Notebook` covers discovery and
structure, `Section` covers one section's pages, edits, events and conflict
pages. Both desktop and iOS use exactly this surface.

## The replica

Each open section has a replica: a SQLite database in the app's cache, named by
where the notebook is and the section's logical identity, so the same file
reopens the same queue after a relaunch. It holds:

- **the base**: the last remote image the queue applies to, stored in chunks so
  that acknowledging a publication rewrites only the chunks the transaction
  touched;
- **batches of edits**: each edit's ops, serialized, grouped into the batch
  that will publish together;
- **payloads**: picture and attachment bytes, stored once each, keyed by hash.

The working state is never stored. It is the base with each sealed batch
replayed and the open batch applied, rebuilt on open.

A password-protected section's replica holds nothing in the clear. Its base and sealed
transactions are the file's own ciphertext, and its edits and payloads are sealed with
AES-256-GCM under a key derived from the section's, so the queue opens, offline or after a
relaunch, only once the section is unlocked. A locked section is never opened in the
background; its edits wait until it is unlocked again. A replica of a section another device
then protects is deleted the next time the notebook is read, unless edits wait in it; those
wait for the key, then come back as copies of the pages they changed. The database runs in WAL
mode with full synchronous commits, and every setting is read back to check it
took.

The location matters because OneNote lets a notebook folder be copied whole, a
Finder duplicate or an iCloud `Name 2` conflict copy, and every section in the
copy keeps its identity. Keyed by identity alone, the copy would open the
original's queue and publish its edits into the wrong file. So replicas live in
`cache/replicas/<hash of location>/<identity>.sqlite` (`notebook::location`),
where a location is a canonical local path or `smb://server/share/root`, and a
`location` file in each folder names it. A section renamed or moved inside its
notebook keeps its replica; a copy of a section inside one notebook (`Cross
2.one` beside `Cross.one`, which OneNote opens as a section of its own) is keyed
by its own path instead, the original being the one the folder's TOC lists.

When a notebook moves, its queue follows by one of three roads:

- **the app moved it** (iOS Rename): `location::moved` moves the folder's
  replicas to the new location;
- **something else moved it** (Finder): on open, a section with no replica takes
  one from the folder of a local location that no longer exists, if the file
  stands as that replica's base or has moved on from it (a higher header
  generation). A replica whose base is newer than the file, or a different state
  of the same generation, belongs to another copy and stays;
- **an older install** named replicas by identity alone in the cache's top or
  its `smb` folder: the first location to open the section takes it.

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
during one round trip publish together in the next. A check costs a
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

A notebook can also be opened from its server's address, with no mount at all (File ▸
Open Notebook from Server…), which is how Mac OS X 10.6 reaches a server that no longer
speaks SMB1, the only version its Finder has. The notebook is kept by that address
(`smb://[domain;]user@server/share/folder`), never with a password; the password lives in
the keychain (the Secret Service on Linux) when the user asks to remember it, and
otherwise the sign-in asks again at the next launch.

A file's identity is its root object space, not its server file ID, which
changes when maintenance replaces the file. A check opens by path each time
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

OneNote keeps every section of an open notebook in sync and on this computer, not just
the one on screen. Watched in the lab with a notebook of 200 sections on Samba, OneNote
2010:

- reads every section file once when it first opens the notebook (all 200 within 15
  seconds), so a section never shown before opens with the share gone; opening it again
  with its cache warm, it only lists the folders and reads the files listed otherwise;
- arms one CHANGE_NOTIFY on the notebook's folder, with WATCH_TREE and a filter of names,
  attributes, size and last write, and otherwise sends nothing: 22 idle minutes put no SMB
  request on the wire, not even an echo, and the section on screen waits on the same watch;
- lists the folder once when the server reports a change, and reads only the section that
  changed, some seconds later;
- tries a section it could not reach again about every 31 seconds.

Snowbound does the same. Opening a notebook lists its folders and reads only the files whose
listed size or last write time changed since it last read them, keeping what it took from
each, with the file's stamp, in the cache (`discover::Cache`). On a share, a changed section
whose replica already holds it as it stands, as after its own edits published, costs just a
stamp read. Each section it did read is handed on with its image, which makes the offline copy
or rebases the replica while the file's stamp is still the image's, so a launch reads each file
once, as OneNote does. Dot files (macOS's `.DS_Store` and AppleDouble `._` companions, the
`.snowbound` folder) and Office's `~$` files are not the notebook's and are skipped.
`session::Background` then keeps the sections in sync, one thread per notebook:

```text
connect:  arm the watch, list every folder
            listed as before:  compare the known stamp with the replica, locally
            listed otherwise:  check, one section every 100 ms
share ─ CHANGE_NOTIFY (tree) ─► the file it names is due in 1 s ─► stamp ─► moved: rebase
                             └► a folder it names is listed in 1 s ─► the files listed otherwise
failing:  again in 31 s          otherwise: again in an hour, against a lost notification
```

A check costs one stamp read. Only a section whose replica has edits waiting, or whose file
moved past the replica's base, has its replica opened for the usual sync steps, and it is
closed again afterwards; a section without one gets its offline copy. The section open on
screen is left to its session, whose worker no longer polls: the watch wakes it, and once the
session lets go of the replica the background checks the file at once. A connection that drops
takes its watch with it, so the next one lists every folder again, and so does working online
again.

A notebook in a folder on this computer has no copies and learns of changes from FSEvents or
inotify. One on a network volume the system mounted keeps copies. A Mac's SMB mount reports
another client's change to FSEvents only for a folder watched in its own right, and names just
that folder, so every folder of the notebook is watched and a report lists it. A Linux SMB
mount reports none of another client's changes to inotify, so there each section's stamp is
read every 15 seconds. On iOS, a folder on the device (Snowbound's own, or On My iPhone) has no
copies and learns of other apps' writes through file coordination (`NSFilePresenter`); one a
file provider keeps elsewhere, as iCloud Drive does, keeps copies and is checked every 15
seconds. Closing a notebook deletes its copies, except any with edits still waiting.

## iCloud Drive

A cloud drive syncs whole files with no lock and no compare-and-swap. The stamp check still runs
before each append, but only against this device's copy; when two devices append to the same
base, iCloud keeps one as the file and the other beside it as a **conflict version**
(`NSFileVersion`), on every device. A conflict version is a stamp check that failed after the
fact, and each sync step merges the versions it finds before anything else:

```text
for each version V the remote lists (Remote::versions)
  C := the file, read coordinated
  per space of V: a := its newest revision C holds, or holds merged
  ancestor := V opened at those revisions (Section::open_at)
  ops := lower_page(ancestor[S], V[S]) per page V changed, V's new pages imported under their
         identities, its deletions, moves and conflict pages
  replay ops on C through merge.rs (conflict pages where they clash), seal_as the named
  revisions, publish on C's stamp; then retire V (resolved, removed)
```

Each revision the merge writes is named after the version's revision it merged (a hash of it),
so "holds merged" is a lookup by revision identity. A version merged once, by this device or
another, merges as nothing the next time; two devices merging one version at once write the
same names, and the merge of their two results finds everything held. Any device merges any
version at once; nothing waits for the device that wrote it. A version of another section, or
one that cannot be read, is kept beside the file as `Name (Device).one` instead.

On a cloud drive every read and write of a section file runs under `NSFileCoordinator`, so the
iCloud daemon never swaps a file between the stamp check and the append. A file iCloud evicted
(a `.Name.icloud` placeholder, or on macOS 14 and later a dataless file under its own name) lists
as last read, or as downloading; the host asks for it, and reads the notebook again as files
arrive. The sync thread never waits for one: the replica serves it meanwhile. Opening a section
with no replica yet waits for its file. macOS and iOS offer no public way to keep a file
downloaded, as the Finder's Keep Downloaded does; evicted files are asked for again whenever
seen. Each publication is an upload and a
chance to conflict, so edits wait for a 3 second pause in typing (at most 30 seconds,
`Section::set_pause`), and publish at once when the app leaves the foreground, the screen locks,
the Mac's window loses focus or the app quits. A folder presenter reports other devices'
changes and conflict versions, which change no file, and every section is checked every 15
seconds besides. Signing out of iCloud closes its notebooks; replicas holding edits stay.

`crates/notebook/tests/cloud.rs` runs devices against a fake iCloud (uploads without
compare-and-swap, either side of a race winning, late deliveries, offline spans) and checks that
every device ends on the same file with each typed string once. OneNote 2010 cold-opens a
merged section with its conflict page as its own (`corpus/icloud-merge`).

## Notebook structure

Sections, groups and the notebook's own colour live in the `.onetoc2` files,
which are revision stores edited through the same transaction path. Moving or
renaming a file also rewrites two header fields OneNote checks on open (the
parent TOC's identity and a CRC of the file's name). Without them OneNote
treats the file as a stranger and re-identifies it. Deleting sends sections and
pages to `OneNote_RecycleBin`, as OneNote does, and opening a notebook empties
what the bin has held unchanged for 60 days, as OneNote prunes it: judged by the
content's last change, the pages in one revision, binned section files deleted
with their TOC entries left (`corpus/recycle-purge`).

What only Snowbound reads lives in the notebook's `.snowbound` folder, which carries the
Windows hidden attribute so that OneNote never makes a section group of it. Its files are
plain files, not revision stores: pictures named by their content, and small JSON
mappings that every writer merges before replacing. The first is tag art: a tag keeps
OneNote's definition and a fallback symbol on the page, and `tags.json` maps its name and
symbol to a picture that Snowbound draws in the symbol's place.

## Recovery and migration

`export_recovery` snapshots a replica into a single read-only archive: base and
remote images, queue, attempts, receipts and media. It is the evidence behind
any decision that could lose work. Cache schema conversions check their own
output: every converted page must equal the page the old cache held, or the
conversion rolls back and names the archive it made first.

The [notebook README](../crates/notebook/README.md) documents the API in detail.
