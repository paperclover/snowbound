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
`crates/onestore-diagnostic` backs the [HTML diagnostic editor](../../evidence/DIAGNOSTIC.md).
[`onestore-smb`](../onestore-smb/README.md) provides optional embedded network
access; [`onestore-offline`](../onestore-offline/README.md) provides local
SQLite persistence and reconnect reconciliation for text, insertion and formatting.
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
| `protected::UnlockedSection` (optional feature) | Own decoded buffers for explicit known-password inspection; clear those buffers on drop; derived document strings/exports remain caller-owned |
| `create_section` | Create one page containing one plain-text paragraph and an author, including Unicode |
| `PageCreation`, `PreparedEdit::create_page` | Add an empty top-level page and its section entry atomically, retaining page identities across retries |
| `PageEdit`, `PreparedEdit::pages` | Publish explicitly selected page moves and indentation changes together, preserving page content and historical revisions |
| `create_table_of_contents` | Create ordered section entries from filenames and file identities |
| `replace_property_bytes` | Append one scalar-property revision; preserve prior revisions and unrelated property values and references |
| `replace_text`, `commit_text`, `commit_file_text` | Replace a UTF-16 range across ordinary text runs; publish text, run boundaries and modification time together |
| `Insertion`, `PreparedEdit::insert` | Insert paragraphs into editable containers or positioned outlines into a page, retaining intent identities across rebases |
| `ParagraphSplit`, `PreparedEdit::split` | Split ordinary text at a UTF-16 scalar boundary, retaining the original left identities and moving children to the right |
| `ParagraphJoin`, `PreparedEdit::join` | Join adjacent ordinary text while preserving inherited character styles and native text-identity rules |
| `TextAttribute`, `PreparedEdit::format` | Change character formatting over a UTF-16 range while sharing immutable styles; preserve unselected runs |
| `OutlineEdit`, `PreparedEdit::outline` | Change ordinary outline position/width or a paragraph's saved expansion default, preserving identities and content |
| `TreeEdit`, `PreparedEdit::tree` | Move or delete a subtree on one page, normalize surviving containers, and replace an emptied table cell's paragraph atomically |
| `PreparedEdit::commit`, `PreparedEdit::commit_file` | Publish the exact prepared image under caller-held exclusion or the conservative filesystem adapter |
| `read_file` | Read a snapshot under whole-file exclusion |
| `read_snapshot` | Read a validated snapshot through fresh positioned I/O while the caller excludes maintenance |
| `commit_file_property` | Lock, compare the source snapshot, append and flush, then publish the revision |
| `CommitIo`, `commit_property_bytes` | Supply another storage backend with equivalent exclusion and ordered durability |

Scalar edits accept encoded values and require the caller to maintain MS-ONE
semantics. Text edits maintain run boundaries, inherit the insertion run's formatting,
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
The source stays encrypted; protected writes are rejected.
`PageCreation::new` appends, or inserts before the first page space of an existing
series. `Some("")` creates an empty title field; `None` omits the title node.
The page has no body outlines, generated date/time text or applied template.
Retain the intent to preserve its page, title and space identities; existing
identities require reconciliation before retry. Body insertion and title edits
use those identities through the existing APIs. [Native page-creation fixtures](../../corpus/page-lifecycle/creation/README.md)
cover duplicate Unicode titles, native edits and Rust follow-up edits.
`PageEdit::set_level` changes one page's indentation in place. `PageEdit::move_to`
moves before an existing page space, or appends for `None`, and sets its level.
`PreparedEdit::pages` applies moves in slice order and publishes the final order,
series membership and metadata levels in one transaction. Each page occurs once;
levels are 1–3 and the final first page must have level 1. A following deeper-level
page remains in place unless explicitly selected. To move a group, supply all its
pages in order. Retain the intents across retries so newly formed series keep
their identities; `reposition(PagePosition, level)` revises their placement while
preserving those identities. [Native page-edit fixtures](../../corpus/page-lifecycle/page-edits/README.md)
cover individual tabs, selected and collapsed groups, nesting and promotion.
Insertions update child references, reference counts, modification times and automatic
titles atomically. Paragraphs can be nested or inserted into table cells; outline
coordinates use points. `Insertion::with_formatting` includes nonoverlapping UTF-16
spans in that same publication; gaps keep the default insertion style. Empty text
accepts one `0..0` span for subsequent typing. Retain the `Insertion` value for rebasing: creating another
value creates different object identities. Duplicate insertion identities require
reconciliation. Formatting accepts explicit attributes, preserves inherited values,
and gives retired immutable styles zero current references while retaining history.
Outline layout edits use points. Width is at least 36 points; an explicit user width
and an automatic maximum-width hint remain distinct. Native layout generates the
rendered height. Saved paragraph collapse defaults can be overridden by the native
client's cached view. [Native layout captures](../../corpus/outline-edit/README.md)
verify these edits through a fresh OneNote cache, including fields and nested content.
`TreeEdit::move_to` takes an existing parent and an optional direct sibling to insert
before; `None` appends. Paragraphs retain their descendants and explicit list styles.
Outlines remain page children, so reordering changes their stacking order while
retaining coordinates. `TreeEdit::delete` removes the selected subtree from the active
graph and preserves historical objects. Empty outlines/groups are removed; surviving
group indentation is normalized without shifting other paragraphs. Empty table cells
receive new empty paragraph/text objects with identities retained in the intent.
Title/protected content, ambiguous ancestry, cycles, and incompatible destinations
reject before publication. Modification times, move attribution and automatic titles
publish with the tree change. These explicit destinations differ from keyboard list
indentation, which can also substitute list markers.
Paragraph splits preserve character formatting, retain tags on the left, and clone
mutable list objects without restarting numbering. The new right paragraph/text
identities belong to the retained `ParagraphSplit` intent. Its publication includes
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
rejected before publication. The [offline crate](../onestore-offline/README.md)
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
cargo build -p onestore-diagnostic
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
separately. Native ink and structured equations retain their source data and
appear explicitly as opaque content. The report is a reading view; its native
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
rejects them. The resolved graph borrows the snapshot. Select the object-space ID,
object ID, and property from that graph, then pass those IDs, the same snapshot,
and the replacement's encoded bytes to `commit_file_property`. The
`edit_property` example demonstrates selection by JCID/property/expected bytes,
with optional explicit IDs when conflict copies contain identical text.

## Commit behavior

```text
exclusive lock → exact snapshot comparison
              → append data                 → flush
              → prepare header metadata     → flush
              → publish transaction counter → flush
              → finish counter rollover     → flush
              → notify cached readers       → flush → unlock
```

Stale snapshots fail before writing. Live readers must use equivalent exclusion.
Native conflict creation can still expose cross-space references before their
targets are saved; such snapshots must be rejected and reread while synchronization
proceeds. `read_file` and `commit_file_property` serialize within the process because
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
as part of opening the file (`O_EXLOCK | O_NONBLOCK`): separate open and `flock`
calls allowed overlapping exclusive holders and stranded server locks under
multi-process SMB contention. `tools/smb_lock_race.py` reproduces that failure
without notebook parsing or writing. On the tested macOS SMB mount,
POSIX byte-range locks returned `ENOTSUP`; whole-file locks excluded native
OneNote's lock ranges. This adapter serializes readers and writers during each
operation; it does not reproduce native reader/writer concurrency. The
[locking audit](../../evidence/LOCKING.md) records native coordination bytes, write-open share
modes, and a working macOS SMB-specific byte-range lock probe. `sync_all` falls
back to `fsync` on macOS only when `F_FULLFSYNC` is unsupported. Successful SMB FLUSH replies were observed on the
wire. Correctness requires the backend to honor exclusion and ordered flushes.
The evidence covers transport failures, not physical server power loss or every
filesystem's lock implementation.

For shared network notebooks, use the optional
[`onestore-smb`](../onestore-smb/README.md) crate. It uses native share modes,
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

This example requires `features = ["protected"]`. The owner retains no password or
key after opening. Its source-buffer views cannot outlive it; copies of parsed
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
