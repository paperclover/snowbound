# Milestone 6: a reviewable OneNote document model

Completed milestone; verification and review artifacts are recorded in
[M6-ACCEPTANCE.md](M6-ACCEPTANCE.md). Clover chose
extracting a real notebook into a readable document model as the first hands-on
result. This milestone combines document-format work with repeatable native
verification; completion requires no known easily spottable bugs in her notebook
after automated native comparisons and an independent page-by-page inspection.
Her review should find subtle design issues, not serve as the basic QA pass.

## What Clover receives

A local, read-only review report generated from the copied personal notebook,
with notebook/section/page navigation, readable content, extracted media, and a
document-structure view. The report exposes paragraph nesting and ordering,
text-run styles, outline coordinates, tables, links, tags, and source identities.
It accompanies a machine-readable model and an asset directory, so the result is
useful to future applications as well as inspectable by a person.

The report presents a readable interpretation of the document. It does not claim
to reproduce OneNote's layout engine. Coordinate values and native reference
captures let Clover assess spatial information without depending on a new canvas
renderer. Ink, recording data, encrypted sections, and unrecognized structures
must remain explicitly accounted for, with retained payloads or source references
as appropriate. Unsupported interpretation must never turn into silent omission.

Feedback should be attached to identifiable pages/objects and answer:

- Does the section/page hierarchy match the notebook, including subpages and order?
- Does the representation preserve meaningful paragraph/list/table structure?
- Are text, formatting, links, images, attachments, and tags correctly associated?
- Where does spatial arrangement communicate something the readable view loses?
- Which opaque content prevents this model from supporting a useful reader?

The review ends with concrete corrections or priorities for the document model.
Editing API design follows that feedback; no GUI editor is part of this milestone.

## Sequence and gates

| Step | Deliverable | Gate before proceeding |
| --- | --- | --- |
| 1. Repeatable native oracle | Automated fresh-clone runs using the existing VM controllers, named disposable notebooks, bounded captures, and unconditional teardown | Existing reader/writer native cases replay successfully; intentional content corruption causes a failed comparison; a failed run retains a reproducible artifact bundle and leaves no owned VM running |
| 2. Document semantics | A typed view over the resolved store graph, independent of COM and filesystem transport | Every interpreted feature has a native fixture and assertions for its semantic invariants; unknown and encrypted content is represented explicitly |
| 3. Real-notebook review | Machine-readable export, assets, and a human-readable report derived from the same model | All copied sections/pages are accounted for, source hashes remain unchanged, and supported content matches newly captured native evidence |
| 4. Adversarial validation | Seeded native edit histories, parser fuzzing, and replay of existing commit/concurrency cases | The feature/failure matrix passes, discrepancies have regression cases, and the acceptance report distinguishes exact matches, allowed native normalization, and opaque content |

## Document scope

Start with notebook and section structure, ordered pages/subpages, titles, outlines,
paragraphs, and Unicode text runs. Add character/paragraph formatting, list and
indentation semantics, tables, hyperlinks, images, attachments, and note tags.
Expose conflict pages and recycle-bin membership separately from ordinary pages;
retain their relationship to their source page/section. Account for observed
unrecognized page-like metadata instead of filtering it out of the report.

MS-ONE text-run boundaries use character positions with rules distinct from Rust
UTF-8 byte indices. Fixtures must cover surrogate pairs, combining marks, RTL text,
mixed formatting, empty paragraphs, repeated text, and embedded objects. Titles,
cached title strings, styles, and layout metadata need explicit interpretation;
generic scalar decoding alone cannot establish those relationships.

Keep document interpretation above `onestore`'s revision/property graph. Retain
access to raw identities and values for unknown properties. Add types only where
they express document semantics or prevent invalid interpretation; keep the initial
export/API provisional until Clover has reviewed actual notebook content.

## Independent verification

```text
Authored operation history ──→ expected document semantics
             │
             └─→ real OneNote edit/save → captured .one + native XML/payloads
                                               │
                                               └─→ Rust document model
                                                         │
                                      compare all three ─┘

Existing Rust scalar edit → fresh native read/edit/save → Rust reread
```

Native capture runs use a fresh clone/cache for each independent acceptance case.
Tests of a continuing collaboration session deliberately retain that session's
cache, then use a separate fresh verifier after synchronization. Completion must
be observed through file/content state, not inferred from an arbitrary sleep or a
successful asynchronous COM call. COM IDs are cache identities, not persistent
notebook identities.

Compare page relationships/order, paragraph boundaries, text runs/styles, table
cells, positions, links, and payload bytes. Hashes establish artifact integrity;
they do not establish semantic correctness. Native XML supplies an independent
view of supported content. Selected UI captures cover facts COM omits, especially
conflicts and visual interpretation. Whole-file byte equality after a native save
is not the oracle because OneNote rewrites metadata and representation.

Build native-only control cases for observed normalization, including the previous
table-width and ink z-order changes. Comparison exceptions require specific
evidence and a narrow assertion; broad removal of timestamps, IDs, or geometry
must not conceal meaningful changes.

Use a small independent test model for generated operations; expected values must
not be reconstructed by the same Rust decoder being checked. Shrink a failed
history by removing operations and simplifying their parameters, replaying each
candidate from a clean starting state. Preserve seed, operation history, input and
output files, model diff, native XML/payloads, app/server versions, and relevant
trace excerpts. A regression must reproduce the observed discrepancy before its
fix can be accepted.

Build a specification-derived feature matrix, including features absent from the
personal and stock notebooks. Produce large synthetic notebooks both through Rust
and through real OneNote; compare independent generation paths and exercise size,
ordering, repetition, and mixed-feature interactions. Any unsupported feature must
be explicitly accounted for and investigated, not silently omitted from coverage.

Initial randomized target: 100 reproducible native histories of roughly 20
operations, spanning the supported feature matrix. This is a campaign budget,
not a statistical safety claim. Native-generated files then seed fast local
reader fuzzing. Continue the existing stateful short-I/O/partial-persistence
commit tests; run the existing collaboration scenarios with two Windows clients
and the disposable Linux server. Process termination, Samba restart, transport
loss, and abrupt VM termination are distinct fault cases and must be reported as
such. VM termination does not simulate physical host power loss.

## Tooling findings and setup work

The implementation is in `tools/w7`. Windows instances use sparse qcow2 overlays
on a sealed base, unique hostnames/MACs/control targets, and an authenticated agent.
Linux instances also use overlays. A VDE network connects Windows clients to one
Linux Samba appliance at `192.168.77.1`; the Mac has forwarded SSH and SMB ports.
The MCP's up/down/status operations are enough for ordinary fresh-clone tests.
Reuse those controllers from the test runner rather than implementing another
QEMU lifecycle. Add a narrow controlled-crash facility only when fault cases need
it, with ownership checks and restart/recovery of the same overlay.

Start with two Windows clients and one Linux server. Windows uses x86 CPU
emulation on this host; the suggested 25-client capacity has not been measured.
Increase concurrency from measured test throughput and resource use. One Linux
lab address means server restart/configuration tests require exclusive ownership
of that server; ordinary cases can use separate notebook directories.

The scout confirmed:

- Windows 7 clone `m6scout`, PowerShell 5.1.14409.1005, OneNote 14.0.4763.1000.
  The earlier physical-machine corpus used OneNote 14.0.7015.1000. Record both
  builds' evidence separately; success on one must not be silently attributed to
  the other.
- The clone desktop was 800×600, so existing 1280×720 AHK coordinates cannot be
  reused without setting and verifying display configuration.
- Z: mapped to `\\10.0.0.1\agent` (zenith). The disposable Linux share is a
  different destination, `\\192.168.77.1\agent`. Parameterize shared paths rather
  than reusing the old A: convention or remapping Z:.
- Linux reported Samba 4.22.10 and an ext4 data filesystem. Windows read a
  Linux-created marker and Linux read a Windows-created marker through that share.
- A fresh native cache opened the template-free Rust notebook and returned its
  expected paragraph through COM. The captured XML is under
  `evidence/milestone6-scout/page.xml`. Cache reset remains necessary when reusing
  file identities within a clone.
- Procmon started through `win7_spawn` and wrote a PML capture. The unfiltered
  short capture grew to about 126 MB; filter by test process/path and bound capture
  duration before using it in a campaign. Trace contents were not analyzed in this
  scout, and the transient PML was removed with the clone.
- All six VM-tooling tests passed with the scout running. One test assumed the
  first control port was free; it now checks that the registered target points to
  the allocated port, while retaining the separate uniqueness assertion.
- Both scout VMs were stopped and deleted after the smoke checks. Neither the
  sealed images nor the personal notebook were edited.

Existing native scripts assume a parked personal Windows profile, fixed A: paths,
and specific display coordinates. Adapt their orchestration for disposable clones
while retaining the guards used for the physical laptop. Windows base identity,
Linux package/server versions, and relevant configuration must accompany every
run; cloud-init currently installs packages at clone creation time.

## Completion and review boundary

The milestone is ready after the copied notebook passes automated comparison and
an independent page-by-page inspection with no known easily spottable bugs.
Corrections found during that inspection need reproductions and regression checks
before handoff. Clover's subsequent review addresses subtle representation and
design decisions. Deliver the report, model,
assets, acceptance matrix, and reproduction command together. Preserve originals
and export evidence before automatically deleting all machines created by the
run. The next decision is which document operations to expose for editing based
on that review, rather than another general approval to continue infrastructure.
