# onestore

An experimental native Rust library for OneNote revision stores (`.one` and
`.onetoc2`). It reads committed object graphs, creates a small notebook without
a template, appends scalar-property and text edits with a recoverable commit protocol,
and interprets MS-ONE document structure, formatting, media, and historical pages.
The storage gates are recorded in [PROGRESS.md](PROGRESS.md); document-model
verification is recorded in [M6-ACCEPTANCE.md](M6-ACCEPTANCE.md). Concurrent-editing
verification is recorded in [MILESTONE7.md](MILESTONE7.md).

Text edits publish ancestor modification timestamps with the changed content.
Omitting those timestamps caused acknowledged edits to disappear during native
conflict merging. The repaired eight-client replay retains all four competing
results after reconnection, application closure and fresh-cache native inspection;
see `mixed-conflict-06` in the milestone evidence. Use disposable copies for
notebook editing.

## Workspace

`crates/onestore` contains the library, examples and integration tests. New Rust
prototypes belong in sibling directories under `crates/` and depend on
`onestore = { path = "../onestore" }`. The root manifest discovers these crates.
The consumer boundary and API tradeoffs are recorded in [API-AUDIT.md](API-AUDIT.md).
Shared native fixtures, specifications, evidence and Python/VM tools stay at the
repository root; `fuzz/` remains an independent cargo-fuzz workspace.

Run Cargo commands from the root. Select `-p onestore` when working only on the
library, or `--workspace` for checks across all crates. Example binary paths
remain `target/debug/examples/…` for the native verification tools.

## Supported surface

| API | Contract |
| --- | --- |
| `Store`, `RevisionIndex`, `ResolvedRevision` | Parse storage, resolve revisions and reference graphs, expose roots and objects |
| `PropertySets`, `Object::references` | Decode properties and ID streams while retaining raw values |
| `Object::file_reference`, `Store::file_data` | Identify internal/external payloads and read internal payload bytes |
| `document::Document`, `Revision::text_runs` | Interpret document objects and inherited text formatting while retaining unknown properties and revision identities |
| `create_section` | Create one page containing one plain-text paragraph and an author, including Unicode |
| `create_table_of_contents` | Create ordered section entries from filenames and file identities |
| `replace_property_bytes` | Append one scalar-property revision; preserve prior revisions and unrelated property values and references |
| `replace_text`, `commit_text`, `commit_file_text` | Replace a UTF-16 range within one ordinary text run; publish text, run boundaries and modification time together |
| `read_file` | Read a snapshot under whole-file exclusion |
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
sections retain their encrypted structure and payloads; the library does not
derive password keys or decrypt their pages.
External `.onebin` references identify payloads for the caller to obtain. Cloud
FSSHTTP synchronization and a C ABI are outside the implemented surface.

## Try it

Requires Rust 1.97 or later for the verified build. Examples create new destinations
and refuse to overwrite them. The Python report requires Pillow.

```sh
cargo run --example create_notebook -- /tmp/one-demo 'Hello from Rust.' 'Example Author'
cargo run --example inventory -- /tmp/one-demo/synthetic.one
cargo run --example inspect -- /tmp/one-demo/synthetic.one
cargo run --example document -- /tmp/one-demo/synthetic.one /tmp/one-model
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
              → finish counter rollover     → flush → unlock
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
OneNote's lock ranges. `sync_all` falls back to `fsync` on macOS only when
`F_FULLFSYNC` is unsupported. Successful SMB FLUSH replies were observed on the
wire. Correctness requires the backend to honor exclusion and ordered flushes.
The evidence covers transport failures, not physical server power loss or every
filesystem's lock implementation.

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
document feature matrix is in [FEATURES.md](FEATURES.md), and the milestone's
acceptance contract is in [MILESTONE6.md](MILESTONE6.md).

The stage-5 gate uses OneNote 2010 build 14.0.7015.1000 on Windows 7, a macOS SMB
mount, and Samba on zenith. [The collaboration corpus](corpus/collaboration/round-01)
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
