# onestore

An experimental native Rust library for OneNote revision stores (`.one` and
`.onetoc2`). It reads committed object graphs, creates a small notebook without
a template, appends property, text, paragraph, outline and formatting edits with a
recoverable commit protocol, and interprets MS-ONE document structure, formatting, media, and historical pages.
The storage gates are recorded in [PROGRESS.md](../../evidence/PROGRESS.md); document-model
verification is recorded in [M6-ACCEPTANCE.md](../../evidence/M6-ACCEPTANCE.md). Concurrent-editing
verification is recorded in [MILESTONE7.md](../../evidence/MILESTONE7.md). Crash recovery and the
read/write HTML diagnostic editor are recorded in [MILESTONE8.md](../../evidence/MILESTONE8.md).
Embedded SMB coordination, durable offline editing and document-growth acceptance
are recorded in [MILESTONE9.md](../../evidence/MILESTONE9.md#document-writer-and-offline-acceptance).

Use disposable copies for notebook editing. Header version notification now
follows durable transaction publication, fixing a native cached-reader race.
The [lost-reply acceptance](../../evidence/MILESTONE9.md#lost-reply-acceptance-with-version-notification-published-last)
records the failure, reduced regression model, twelve-client repeat and cold
OneNote verification of all 3200 editing intents.

## Workspace

`crates/onestore` contains the library, examples and integration tests. New Rust
prototypes belong in sibling directories under `crates/` and depend on
`onestore = { path = "../onestore" }`. The root manifest discovers these crates.
The consumer boundary and API tradeoffs are recorded in [API-AUDIT.md](../../evidence/API-AUDIT.md).
`onestore-diagnostic` in the `notebook` crate backs the [HTML diagnostic editor](../../evidence/DIAGNOSTIC.md).
[`notebook`](../notebook/README.md) provides notebook discovery, optional embedded
network access (feature `smb`) and local SQLite persistence and reconnect reconciliation for text, insertion and formatting.
Shared native fixtures, specifications, evidence and Python/VM tools stay at the
repository root; `fuzz/` remains an independent cargo-fuzz workspace.

Run Cargo commands from the root. Select `-p onestore` when working only on the
library, or `--workspace` for checks across all crates. Example binary paths
remain `target/debug/examples/…` for the native verification tools. The collaboration
harness also accepts `--client-profile release`.

## Supported surface

| API | Contract |
| --- | --- |
| `Store`, `RevisionIndex`, `ResolvedRevision` | Parse storage, resolve revisions and reference graphs, expose roots and objects |
| `PropertySets`, `Object::references` | Decode properties and ID streams while retaining raw values |
| `Object::file_reference`, `Store::file_data` | Identify internal/external payloads and read internal payload bytes |
| `document::Document`, `Revision::text_runs` | Interpret document objects and inherited text formatting while retaining unknown properties and revision identities |
| `Document::active`, `Document::pages_in`, `Revision::parents`, `RevisionIndex::active` | Resolve the active revision, the pages of a space and parent links without repeating the lookups |
| `Page::copy` | The page's content under fresh identities for copying into another section, definitions and payloads included; indent levels only an outline group carries refuse |
| `page::link::internal_link`, `page::link::parse_internal_link` | Build and read the `onenote:#…` URLs OneNote stores for links to sections, pages and paragraphs, by identity |
| `page::Recording`, `page::MediaIndex` | A recording attachment's identity and type, and a paragraph's link to a moment in recordings; read from native pages and kept through edits, never authored |
| `page::Page`, `page::Paragraph`, `page::Ink`, `page::Math` | Build an editable page model (title, outlines, paragraphs with coalesced text spans, tables, images, attachments, ink drawings and handwriting decoded to stroke polylines in page points) with stored identities; equations parse from their linear text and run data into a tree that renders the MathML OneNote exports; content outside the model is retained as `Unsupported` |
| `protected::UnlockedSection` (optional feature) | Own decoded buffers for explicit known-password inspection; clear those buffers on drop; derived document strings/exports remain caller-owned |
| `create_section` | Create one page containing one plain-text paragraph and an author, including Unicode |
| `PageCreation`, `SectionOp::Create` | Add an empty top-level page and its section entry atomically, under identities the intent retains across retries |
| `PageEdit`, `SectionOp::Pages` | Publish explicitly selected page moves and indentation changes together, preserving page content and historical revisions |
| `SectionOp::Delete` | Remove explicit pages and their section references atomically while retaining stored revisions |
| `create_table_of_contents` | Create ordered section entries from filenames and file identities |
| `TocEdit`, `edit_table_of_contents` | Add, rename, order and remove a table of contents' section and group entries, and colour the notebook, as one revision's `Transaction`; a section's colour is `SectionOp::Color` in its own metadata |
| `place`, `place_file` | Name a file for its notebook as OneNote does on adoption (parent TOC identity and name CRC in the header), so OneNote keeps its identity, on any `CommitIo` or under the filesystem adapter |
| `TextAttribute`, `PageOp::Format` | Change character formatting over a UTF-16 range while sharing immutable styles; preserve unselected runs |
| `OutlineEdit`, `PageOp::Outline` | Change ordinary outline position/width or a paragraph's saved expansion default, preserving identities and content |
| `Transaction`, `Stamp` | The bytes a commit writes (appended data, in-place list-tail and log patches, header) and the header and length it requires unchanged; `commit` under caller-held exclusion, `commit_file` under the conservative filesystem adapter, `apply` to the base image in memory; serializable for queues |
| `Arena`, `Section` | Keep a section parsed across edits: `open` validates an image once; `seal` appends one revision per changed space as a `Transaction` on `stamp`, checking only what it appends; `replay` applies a queued transaction; `image` and `page` read the result |
| `op::{Edit, Op, PageOp, TableEdit, SectionOp}`, `Section::apply` | Object-level edits (text, formatting, links, equations, paragraph insertion/split/join/move/deletion/levels, outlines, lists, tags, styles, paragraph formatting, tables, pictures, attachments, ink, page creation/import/moves/removal, conflict pages) applied whole or not to a kept-open section with emitter-chosen identities, UTF-16 ranges and the edit's time; refusals name the target, identity or structure at fault |
| `SectionOp::Conflict`, `Section::conflicts`, `ConflictPage` | Keep the version of a page a merge could not take as OneNote 2010 does: a read-only conflict page (`jcidConflictPageMetaData`, `IsConflictPage`, conflict objects marked) under the page's manifest, which says it has conflict pages; list each page's, newest first, and delete one (`SectionOp::Delete`) as OneNote's Delete Conflict Page does |
| `Section::versions`, `Section::version`, `PageVersion`, `SectionOp::RestoreVersion`, `SectionOp::DeleteVersions` | A page's versions as OneNote 2010 keeps them: earlier revisions of the page's own space, each current under its own context and listed by the page's version history, newest first with its author and time; read one as a page (O(section)); restore one as Restore Version does (the page's next revision builds on it and the page as it stood becomes the newest version) or unlist some as Delete Version does, their revisions staying stored ([corpus](../../corpus/page-versions/README.md)) |
| `op::lower`, `op::lower_page` | The ops turning a range of paragraphs, or a whole page model, into another, computed from the models alone, for editors and imports |
| `op::predict` | The page an op leaves, as the section stores it and reading it back shows it |
| `read_file` | Read a snapshot under whole-file exclusion |
| `read_snapshot` | Read a validated snapshot through fresh positioned I/O while the caller excludes maintenance |
| `CommitIo`, `confirm`, `confirm_file` | Supply another storage backend with equivalent exclusion and ordered durability; check that a stamp still holds |

Edits enter a kept-open `Section` as ops and leave as one appended revision per
changed space; only section and page creation, imports and opening files handle
whole images. Text edits maintain run boundaries, inherit the insertion run's formatting,
and promote legacy text to Unicode when needed. Explicit and body-derived
navigation titles update in the same transaction; unsupported fields,
protected objects and split surrogate pairs are
rejected before writing. Appended snapshots cap revision dependency depth at 512
while retaining historical revisions. TOC snapshots can remap encoded CompactIDs
without changing their resolved references. Password-protected
sections retain their encrypted structure and payloads. With the optional
`protected` feature, `protected::UnlockedSection` opens native OneNote 2010
AES-128/CBC, SHA-1 password wrappers into a borrowed document view. Incorrect
passwords, unsupported protection profiles and work-limit failures remain distinct.
The source stays encrypted. `UnlockedSection::apply` applies page ops and seals them
under the section's key (fresh IV per object, payloads under the file IV); `Section`
rejects protected sections.
`PageCreation::new` appends, or inserts before the first page space of an existing
series. `Some("")` creates an empty title field; `None` omits the title node.
`dated(date, time)` gives the title OneNote 2010's date and time fields showing that
text, as OneNote titles a new page; `keeping(identity, created)` keeps another page's
identity and creation time, as a page moved to the recycle bin keeps them. The page has
no body outlines or applied template; `create_empty_section` makes a section for such
pages, and `PageOp::Color` sets or clears a page's colour (`0x14001d2a` on the page node,
absent for "No color").
Retain the intent to preserve its page, title and space identities; existing
identities require reconciliation before retry. Body insertion and title edits
use those identities through page ops. [Native page-creation fixtures](../../corpus/page-lifecycle/creation/README.md)
cover duplicate Unicode titles, native edits and Rust follow-up edits.
`PageEdit::set_level` changes one page's indentation in place. `PageEdit::move_to`
moves before an existing page space, or appends for `None`, and sets its level.
`SectionOp::Pages` applies moves in slice order and publishes the final order,
series membership and metadata levels in one transaction. Each page occurs once;
levels are 1–3 and the final first page must have level 1. A following deeper-level
page remains in place unless explicitly selected. To move a group, supply all its
pages in order. Retain the intents across retries so newly formed series keep
their identities; `reposition(PagePosition, level)` revises their placement while
preserving those identities. [Native page-edit fixtures](../../corpus/page-lifecycle/page-edits/README.md)
cover individual tabs, selected and collapsed groups, nesting and promotion.
`SectionOp::Delete` removes exactly the supplied page spaces,
including subpages only when selected explicitly. The first remaining page becomes
top-level; other page levels and surviving content are retained. The operation
creates no recycle-bin copies and preserves prior revisions, so it is not secure
erasure. Duplicate, missing or non-page identities reject the entire batch;
an empty selection leaves the file unchanged.
`PageOp::Insert` and `PageOp::Add` update child references, reference counts,
modification times and automatic titles atomically. Paragraphs can be nested or
inserted into table cells; outline coordinates use points. Inserted text keeps its
spans' formats; `PageOp::Format` over an empty text's `0..0` sets its insertion style.
New objects carry the emitter's identities, so a queued edit replays onto another
image unchanged; an identity already on the page refuses the edit. Formatting accepts explicit attributes, preserves inherited values,
and gives retired immutable styles zero current references while retaining history.
Outline layout edits use points. Width is at least 36 points; an explicit user width
and an automatic maximum-width hint remain distinct. Native layout generates the
rendered height. Saved paragraph collapse defaults can be overridden by the native
client's cached view. [Native layout captures](../../corpus/outline-edit/README.md)
verify these edits through a fresh OneNote cache, including fields and nested content.
`PageOp::Move` takes an existing parent and an optional direct sibling to insert
before; `None` appends. Paragraphs retain their descendants and explicit list styles.
Outlines remain page children, so reordering changes their stacking order while
retaining coordinates. `PageOp::Delete` removes the selected subtree from the active
graph and preserves historical objects. Empty outlines/groups are removed; surviving
group indentation is normalized without shifting other paragraphs. An edit that
leaves a table cell without a paragraph is refused; insert its replacement in the
same edit.
Title/protected content, ambiguous ancestry, cycles, and incompatible destinations
reject before publication. Modification times, move attribution and automatic titles
publish with the tree change. These explicit destinations differ from keyboard list
indentation, which can also substitute list markers.
Paragraph splits preserve character formatting, retain tags on the left, and clone
mutable list objects without restarting numbering. `PageOp::Split` names
the new right paragraph/text identities. Its publication includes
the complete child graph and title metadata; repeating an existing identity requires
reconciliation. Title containers, generated fields, recording-linked text and
associated run metadata are rejected before I/O. Native split controls and subsequent
typing checks reside in [the paragraph corpus](../../corpus/paragraph-edit/README.md).
Joins retain the left paragraph. Nonempty left text keeps its identity; empty left
text adopts the right text identity. **The left tags win: right-side tags are removed
from active text even when the left text is empty.** History retains the original
objects. Select the preceding leaf text; where that leaf is deeper than the right
paragraph, right children move to its ancestor at the right paragraph's level.
Ambiguous ancestry, unsupported indentation transitions and unknown implicit
font/language inheritance reject before I/O. This is a logical join, so keyboard
actions that only change list or indentation state remain separate operations.
Generated fields, protected targets and unsupported run-data boundary changes are
rejected before publication. The [notebook crate](../notebook/README.md)
documents durable local operations and reconciliation. The
[document-writer acceptance](../../evidence/MILESTONE9.md#document-writer-and-offline-acceptance)
includes twelve mixed native/Rust clients, outages, lost replies and native revision retirement.

External `.onebin` references identify payloads for the caller to obtain. Cloud
FSSHTTP synchronization and a C ABI are outside the implemented surface.

## Try it

Requires Rust 1.97 or later for the verified build. Examples create new destinations
and refuse to overwrite them. The Python tools require Python 3.10 or later and Pillow.

```sh
cargo run --example create_notebook -- /tmp/one-demo 'Hello from Rust.' 'Example Author'
cargo run --example inventory -- /tmp/one-demo/synthetic.one
cargo run --example inspect -- /tmp/one-demo/synthetic.one
cargo run --example document -- /tmp/one-demo/synthetic.one /tmp/one-model
cargo build -p notebook --bin onestore-diagnostic
python3 tools/notebook_report.py /tmp/one-demo /tmp/one-report --timezone America/Los_Angeles
```

A seeded text edit on a disposable copy records its page, UTF-16 range,
replacement and outcome as JSON:

```sh
cargo run --example random_edit -- /tmp/one-demo/synthetic.one /tmp/edited.one 42
```

The report contains readable pages, document JSON, assets, source identities and
coordinates. It preserves paragraph nesting, lists, tables, links and tags.
Historical contexts, recycle-bin pages and default templates are represented
separately. Native ink is decoded to strokes and equations to MathML; both
retain their source data. The report is a reading view; its native
PDF references supply the original canvas layout.

Read and validate a snapshot before interpreting its graph:

```rust,no_run
use onestore::{read_file, RevisionIndex, Store};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bytes = read_file("notebook/synthetic.one")?;
    let store = Store::parse(&bytes)?;
    if !store.checksum_mismatches.is_empty() {
        return Err("Transaction checksum damage".into());
    }
    let index = RevisionIndex::parse(&store)?;
    index.validate_current()?;
    Ok(())
}
```

`Store::parse` exposes checksum mismatches for diagnostic readers; the writer
rejects them. The resolved graph borrows the snapshot. Open it as a `Section`,
apply ops naming objects from that graph, seal, and commit the `Transaction`. The
`edit_property` example demonstrates selection by JCID/property/expected bytes,
with optional explicit IDs when conflict copies contain identical text.

## Commit behavior

```text
exclusive lock → header and length check
              → append data                 → flush
              → prepare header metadata     → flush
              → publish transaction counter → flush
              → finish counter rollover     → flush
              → notify cached readers       → flush → unlock
```

Stale snapshots fail before writing: every committed transaction and placement rewrites
the header (MS-ONESTORE 2.3.1), so the body is never compared. Live readers must use
equivalent exclusion.
Native conflict creation can still expose cross-space references before their
targets are saved; such snapshots must be rejected and reread while synchronization
proceeds. `read_file` and `Transaction::commit_file` serialize within the process because
macOS SMB locks can be reentrant. The lock is nonblocking across processes;
contention requires a fresh read and a later retry.

| Error state | Meaning and caller action |
| --- | --- |
| `NotCommitted` | This edit was not published. Preparation bytes may exist. Reread before retrying. |
| `Unknown` | Publication may have persisted despite the error. Reread and resolve the intended edit before retrying. |
| `Committed` | Publication was durably acknowledged; counter cleanup or lock release failed. Reopen the committed result instead of replaying the edit. |

At counter rollover, empty transactions make intermediate published counts valid.
The highest changed byte is flushed before lower bytes are cleaned up. The
255→256 and 65535→65536 boundaries and interrupted cleanup states have independent
native acceptance captures. A no-op still flushes; a failed flush has an unknown
durability outcome.

The filesystem adapter uses whole-file locking. On macOS it acquires the lock
as part of opening the file (`O_EXLOCK` to commit, `O_SHLOCK` to read, with
`O_NONBLOCK`): separate open and `flock` calls allowed overlapping exclusive holders
and stranded server locks under multi-process SMB contention.
`tools/smb_lock_race.py` reproduces that failure without notebook parsing or writing,
and with `--shared-reads` checks that shared readers never overlap a writer. On the
tested macOS SMB mount, POSIX byte-range locks returned `ENOTSUP`, and `flock` of
either kind became an exclusive lock over the whole file, which fails OneNote's reads.
smbfs sends `O_SHLOCK` and `O_EXLOCK` as share modes: a read excludes writers,
OneNote's included, but not OneNote's readers; a commit excludes everyone. It does not
reproduce native reader/writer concurrency. The
[locking audit](../../evidence/LOCKING.md) records native coordination bytes, write-open share
modes, and a working macOS SMB-specific byte-range lock probe. `sync_all` falls
back to `fsync` on macOS only when `F_FULLFSYNC` is unsupported. Successful SMB FLUSH replies were observed on the
wire. Correctness requires the backend to honor exclusion and ordered flushes.
The evidence covers transport failures, not physical server power loss or every
filesystem's lock implementation.

For shared network notebooks, use the optional
[`notebook::smb`](../notebook/README.md) module. It uses native share modes,
shared reader guards, writer exclusion and fresh pathname identity checks without
an OS-mounted share. Its [coordination acceptance](../../evidence/MILESTONE9.md) covers native
maintenance, mixed readers/writers, reconnects and uncertain publication. The
filesystem adapter retains its conservative locking; mounted-path freshness
across native replacement is not established by that exclusion.

## Verification

```sh
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
python3 tools/verify-corpus.py
python3 tools/verify-reader.py       # requires Pillow and the local private corpus
python3 tools/verify-writer.py
python3 tools/verify-collaboration.py
python3 tools/verify-document.py /path/to/copied/notebook /path/to/native/read
```

The frozen private corpus is excluded from version control. Its verifier checks 26 pages,
581 text objects, hyperlink targets, exact image bytes, and native image conversions.
Synthetic corpus manifests bind binary fixtures to independent native XML and
attachment captures. `verify-corpus.py` also requires that private corpus.

Storage tests exercise malformed references, deep properties, historical revision
preservation, short I/O, stale snapshots, counter tears, and every injected I/O
failure point at ordinary and rollover commits. The crash model persists arbitrary
subsets of unflushed bytes and is shared with the stateful commit fuzzer.

```sh
cargo +nightly fuzz run revisions -- -max_total_time=120 -max_len=262144 -rss_limit_mb=2048
cargo +nightly fuzz run commit -- -max_total_time=300 -max_len=4096 -rss_limit_mb=2048
cargo +nightly fuzz run paragraph -- -max_total_time=120 -max_len=160 -rss_limit_mb=2048
```

Fuzz targets cover storage, properties, revisions, scalar edits, creation, and
multi-edit interrupted commits. Seed the revision target with native `.one` files
using symlinks under `fuzz/corpus/revisions`; empty seed directories mostly exercise
header rejection. Bounded run counts and native findings live in `PROGRESS.md`.

The document fuzzer mutates native property streams, repairs their checksums, and
traverses every retained revision and resolved text run. Its public seeds live in
the source target; private seeds are supplied only at runtime. Native edit-history
tests compare independently generated operations, OneNote XML and the Rust model;
failed histories can be replayed and shrunk in fresh disposable clones. The
document feature matrix is in [FEATURES.md](../../evidence/FEATURES.md), and the milestone's
acceptance contract is in [MILESTONE6.md](../../evidence/MILESTONE6.md).

The stage-5 gate uses OneNote 2010 build 14.0.7015.1000 on Windows 7, a macOS SMB
mount, and Samba on zenith. [The collaboration corpus](../../corpus/collaboration/round-01)
captures native lock contention, different-paragraph merging, same-paragraph
conflicts, offline editing/reconnection, and lost successful FLUSH replies at
preparation, publication, and counter cleanup. Each final notebook was reopened
from a fresh native cache. Competing text survives as native conflict pages;
OneNote's COM hierarchy omits those pages, so the offline case also includes a
native UI capture. Full-page COM updates produced an extra conflict copy during
the disjoint case and two recorded geometry changes; the verifier checks those
exact changes as well as retained image, ink, table, and attachment content.

`tools/native/profile.ps1` parks/restores the personal native profile and cache;
`cold.ps1`, `read.ps1`, and `collaborate.ps1` operate on disposable test roots.
`tools/zenith-locks` observes server locks. `tools/smb-proxy.py` traces and interrupts
a dedicated loopback test session; its control JSON selects the successful response
and occurrence to withhold. The captured trace and result files are the regression
oracle; replaying the native experiments requires the supplied Windows/share setup.

## Protected inspection

```rust,no_run
# #[cfg(feature = "protected")]
# fn example() {
use onestore::{RevisionIndex, Store, protected::{Limits, UnlockedSection}};
# fn inspect(bytes: &[u8], password: &str) -> Result<(), Box<dyn std::error::Error>> {
let store = Store::parse(bytes)?;
let index = RevisionIndex::parse(&store)?;
let unlocked = UnlockedSection::open(&index, password, Limits::default())?;
let document = unlocked.document()?;
assert!(!document.pages()?.is_empty());
drop(document);
drop(unlocked);
# Ok(()) }
# }
```

This example requires `features = ["protected"]`. The owner retains no password;
its derived key is cleared on drop. Its source-buffer views cannot outlive it; copies of parsed
strings, serialized models and exports have their own lifetimes. These copies are
plaintext, and dropping the unlock owner does not clear them. CBC has no general
ciphertext-authentication guarantee; native read-only hashes and model validation
check the corresponding structure. Internal payloads are decoded; external payload
references remain references, and their protected decoding is not implemented.

For a deliberate plaintext diagnostic export, build the notebook exporter with
`--features protected` and pass `--password-file PATH` after the source and optional
new output directory. The file contains exact UTF-8 password bytes; no newline is
removed or Unicode normalization applied. The exporter creates protected exports
under an owner-only directory on Unix and reports protected external payloads as
unsupported. It never rewrites the encrypted source.
