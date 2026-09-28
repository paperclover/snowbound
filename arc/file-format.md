# The file format and `onestore`

Everything Snowbound promises (opening the notebooks you already have, editing
them beside OneNote, collaborating without a server) comes down to one thing:
reading and writing OneNote 2010's files exactly as OneNote does. `onestore` is
the crate that owns that. It knows nothing about SQLite, networks or pixels. It
parses files, turns them into a model an editor can work with, and turns edits
back into bytes OneNote will accept.

## A revision store

A notebook is a folder. Each section is a `.one` file. The folder's
`Open Notebook.onetoc2` lists the sections and section groups in order, and a
section group is a subfolder with its own `.onetoc2`. Both kinds of file share
one container format, the *revision store* described in Microsoft's
MS-ONESTORE specification. The content inside them follows MS-ONE.

A revision store is closer to a small database than to a document. Logically
it looks like this (physically, list fragments and revision data interleave as
the file grows):

```text
┌──────────────────────┐
│ header (1024 bytes)  │  commit point: transaction count, file version GUID, list roots
├──────────────────────┤
│ file node lists      │  chains of fragments; each fragment ends by pointing at the next
│  ├ root list         │    object spaces in the file
│  ├ per object space  │    its revisions, in order
│  ├ transaction log   │    how many list entries each committed transaction added
│  └ file data store   │    embedded pictures and attachments
├──────────────────────┤
│ revision  1          │  objects the revision declares, in object groups
│ revision  2 ─dep─► 1 │  only what changed, plus the ancestors that point at it
│ revision  3 ─dep─► 2 │
│ …  (appended)        │
└──────────────────────┘
```

- **Object spaces** partition the content. A section has a root space for
  section-wide metadata and the page series, plus one space per page. A
  conflict page, when there is one, gets a space of its own too.
- **Revisions** are per space. A revision declares the objects it adds or
  changes and depends on the revision before it. OneNote itself typically
  appends a small dependent revision for each edit: the changed objects plus
  the chain of containers above them, whose modification times moved.
- **Contexts** label revisions besides the current one. A page's *versions* are earlier
  revisions of its own space kept current under contexts of their own, and its version
  history is one more revision, under a fixed context, listing them. Restoring a version
  forks the page's chain from it; deleting one only unlists it.
- **Objects** have a type (a JCID) and a property set. They reference each
  other by *ExtendedGUID*: a GUID plus a small integer. Inside a revision these
  are compressed to compact IDs through a global ID table.
- **The header** is the commit point. Bytes appended past the old end of the
  file mean nothing until the header's transaction count says a transaction
  holding them has committed.

External payloads (large pictures, attachments, recordings) can live beside
the section as `.onebin` files in a `_onefiles` folder. The section refers to
them by name.

## What "append one revision" means

Every edit Snowbound makes ends as a `Transaction`: bytes appended at the old
end of the file, a few small patches inside it (the tail of each list gains a
link to its new fragment, and the transaction log gains an entry), and a new
header. A transaction is written for a particular base, named by its `Stamp`:
the base's header and its length.

```text
exclusive lock ─► stamp still matches?  no ─► NotCommitted, reread and rebase
                    │ yes
                    ▼
                  append new fragments, patch list tails  ─► flush
                  header fields that describe the append  ─► flush
                  transaction count (the commit)          ─► flush
                  file version GUID (wakes cached readers)─► flush ─► unlock
```

The stamp works because every committed transaction rewrites the header,
including the file version GUID. So equal stamps mean the same committed image,
and a commit never has to compare the file's body. The flushes are what make a
torn write recoverable. At any cut point, the file is either the old image with
some ignored bytes past its end, or the new image. The version GUID goes last
because OneNote's cached readers watch it, not the transaction count.
Publishing it any earlier was once a real race with native readers.

A commit ends in one of three states. Callers must honour them:

| State | Meaning |
| --- | --- |
| `NotCommitted` | Nothing was published. Reread before retrying. |
| `Unknown` | The reply was lost after the point of no return, so it may have landed. Reread and find out before doing anything else. |
| `Committed` | Durable. Only cleanup failed. Never replay it. |

The only operations that handle a whole image are creating a section or page,
and opening a file. Everything else is an appended revision. The project holds
this as a rule rather than an optimisation. A writer that regenerates a page
from a model does page-sized work (parsing, diffing, rereading the file) for
every keystroke. On a share, that work happens inside the writer lock every
other client is waiting on.

## Kept open: `Section`

`Section` is a section file parsed once and kept in memory across edits. Ops
apply to its spaces in memory. `seal` then turns everything that changed into
one `Transaction` that appends one revision per changed space. A seal checks
only what it appends: every fragment is linked from its list's old tail, every
declared object parses and resolves, reference counts are the incremental
counts, and the log entries have the CRC the state predicts. The full-file
validator runs when a file opens and throughout the tests. The section borrows
its bytes from an arena that lives beside it, so there is no self-reference and
no `unsafe` (the crate forbids it).

The writer caps revision dependency chains with a checkpoint revision, because
native cold opens fail on very long chains. It also keeps a small reservation
after a transaction-log fragment that ends the file, because OneNote does.

## The page model

`onestore::page` is the editable view of a page: title, outlines, paragraphs
of text with formatting spans, lists, tags, tables, pictures, attachments, ink,
and equations. Every node carries its stored identity. Content outside the
model isn't dropped. It becomes `Unsupported`, which keeps its identity, type
and layout, and its bytes stay untouched in the file. The canvas edits this
model and the notebook crate stores it. Neither needs to know how it is encoded.

## Ops

`onestore::op` is how an edit is expressed: what the editor emits, what the
queue stores and what `Section::apply` writes. An `Edit` is one user action (a
list of ops and a timestamp), and it applies entirely or not at all. The ops are
object-level, for example "replace this UTF-16 range of this text object",
"split this paragraph here, naming the new paragraph", "move this subtree
before that sibling" or "add these table rows". A refusal names the target,
identity or structure at fault.

Three properties make ops work as a sync currency:

- **The emitter chooses new identities.** Replaying the same op on another copy
  of the section creates the same objects, so an edit keeps its meaning across
  retries, rebases and devices. The identity scheme copies OneNote's own
  (`{page guid},n`, counting from 1). An earlier scheme that used 0 was accepted
  by OneNote's integrity check, but OneNote then silently dropped elements in
  roughly one build in five. Only a large native gate caught that.
- **Ranges are UTF-16 code units.** That is how the file stores text, and it is
  what UIKit's text input speaks. Ranges that would split a surrogate pair or a
  hidden field are refused.
- **Ops are plain data.** They serialize, they carry no UI types, and payload
  bytes travel beside them by hash.

`op::model` interprets ops on the page model without touching bytes. The
lowering code (`op::lower`, which turns a model change into ops) uses it to
predict what each op leaves, and the tests use it as an oracle.

## Documented, observed, and reverse-engineered

MS-ONESTORE and MS-ONE are good, but they are not the whole truth:

- Some things OneNote writes aren't in the spec, such as the author initials it
  stores beside every author name. Snowbound writes them too.
- Some things the spec requires aren't needed by OneNote to open a file.
  Snowbound writes them anyway, because other readers exist.
- Ink, equations and media recordings are absent from MS-ONE entirely. Ink and
  math were reverse-engineered, each against an independent oracle. For ink,
  the stroke extents from OneNote's own export. For math, the MathML it exports,
  matched byte for byte. Recordings and embedded objects are read and kept, but
  never authored.

Readers stay tolerant (files in the wild are older, odder, or written by other
tools), while the writer stays strict.

## Protected sections

Password-protected sections keep their encrypted structure. With the
`protected` feature, `onestore` opens OneNote 2010's AES wrapper for a supplied
password. Edits are sealed back under the section's key, with a fresh IV for
each object. Plaintext never reaches the replica's cache. A protected section is
edited directly in its file and never queued.

## Looking inside

The crate's examples are the fastest way to get a feel for a file.
`inventory`, `inspect` and `document` dump the lists, revisions and document
model. `tools/notebook_report.py` renders a readable report of a whole
notebook, and the `onestore-diagnostic` binary in `notebook` is a small HTML
editor for poking at one. The [crate README](../crates/onestore/README.md) is
the reference for the public surface.
