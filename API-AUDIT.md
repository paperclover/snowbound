# Public API audit

The current API is reasonable for a Rust reader/canvas prototype and the supported
text-edit diagnostic tool. It is an experimental interoperability API, not yet a
stable application SDK. The workspace move preserves every public name and its
behavior; this audit adds contract documentation without introducing wrappers or
changing serialization.

## Consumer boundary

```text
owned snapshot bytes
  └─ Store::parse                 committed storage; checksum diagnostics
       └─ RevisionIndex::parse     revision identities and dependencies
            └─ Document::parse    semantic views; checksum damage rejected
                 ├─ pages()       active section pages in stored order
                 └─ text_runs()   text with inherited formatting and links

snapshot + typed identities + UTF-16 range + replacement
  └─ commit_file_text             lock → compare snapshot → append → flush
       └─ Result<(), CommitError> success or explicit publication state
```

A document represents one `.one` or `.onetoc2` file, not a notebook directory.
The directory, section ordering, native references and report assets are currently
assembled by the tooling. A second crate can use the semantic model directly and
keep its layout/cache state separate. Mutating the public model changes only the
inspection view; writers reparse the supplied snapshot and enforce their own
invariants.

## Findings and decisions

| Area | Assessment | Recommendation |
| --- | --- | --- |
| Read/write separation | Good: parsed views cannot accidentally save themselves. Snapshot comparison rejects stale writes before publication. | Keep explicit commit operations; do not add a mutable document plus generic `save()`. |
| Commit outcomes | Good: `NotCommitted`, `Unknown` and `Committed` distinguish retry behavior. An error can still mean the edit persisted. | Keep this distinction at every UI/FFI boundary. Variant documentation now states the caller action. |
| Borrowing and lifetimes | The model owns decoded strings but borrows payloads. Consumers must retain its source snapshot/store. An application object holding both bytes and borrowed views would require a different ownership design. | Let the canvas prototype determine whether its actual access pattern needs an owned model. Do not add a self-referential owner or duplicate document DTO in advance. |
| Semantic error classification | `Error` exposes diagnostic text and an offset, with no typed reason. I/O failures already have `ErrorKind`; semantic distinctions such as unsupported edits and temporarily missing contexts require text matching today. | Before adding automatic semantic recovery, choose a small typed classification based on the recovery actions it needs. A detailed variant for every parser message would enlarge the compatibility burden. |
| Editable text discovery | Readable text is broader than editable text: hidden fields, hyperlinks, equations, generated content, conflicts and protected content can be readable but rejected for writing. Currently callers can try `replace_text` and inspect its result; candidate discovery duplicates some checks in the random-edit example. | For the diagnostic tool, obtain eligibility from the writer's actual validation. If a public eligibility API is added, share that validation rather than maintain a second list of rules. |
| Identity transport | `ExGuid` is typed, ordered, hashable and serializes to its display string. It has no `FromStr` or `Deserialize`. Existing tools retain or look up typed IDs instead of parsing them. | A Rust canvas can keep typed IDs. A JSON editing endpoint should either map strings back to IDs from its snapshot or justify a canonical parser with round-trip tests. |
| Raw storage exports | `Header`, nodes, references, property arenas and both revision layers are public. They are useful for diagnostics, but expose implementation details to consumers. | Keep them available during interoperability work; have the UI depend on `onestore::document` plus edit functions. Moving them into a separate module now would create churn without an established consumer requirement. |
| Public fields and enums | Inspection fields are mutable and enums are exhaustive. Downstream code can depend on the exact shape. | Treat the current Rust and JSON shapes as experimental. Whether to restrict construction or allow future enum variants is an API-evolution tradeoff to decide with the first consumer, not an assumed stability promise. |
| Document boundaries | `pages()` excludes conflicts/history, returns no visible pages for encryption, and rejects TOC files. All referenced contexts remain in the model. | These boundaries are now documented on the method. Readers must inspect the root kind when distinguishing a locked section from an empty section. |
| Units and defaults | Coordinates are points, edit/run offsets are UTF-16 units, Time32 values use the 1980 epoch, and `Format` preserves absence separately from explicit false. | Keep the native distinctions; renderers should use resolved runs and convert offsets explicitly when crossing into UTF-8 or browser selection APIs. |
| In-memory replacements | `replace_text` and `replace_property_bytes` return complete byte images without locking or persistence. Overwriting a live file with those bytes would bypass the commit protocol. | Their documentation now directs existing-file updates to the corresponding commit functions. Raw scalar editing still requires the caller to maintain document semantics. |
| Scope and dependencies | The library stays native and synchronous, with no network runtime, renderer, process management or platform UI dependency. Internal writer helpers remain crate-private. | Keep the core boundary. Put diagnostic serving and rendering in consumers; expose a C ABI only against concrete embedding needs. |

The two decisions worth reviewing before the diagnostic tool are semantic error
classification and edit eligibility. Neither requires a broad API redesign.
The lifetime/ownership question can be informed by the separate canvas prototype.

## Verification

- Root workspace tests: `evidence/m8/workspace-tests.log`.
- Clippy and formatting: `workspace-clippy.log`, `workspace-format.log`.
- Rustdoc and compiled documentation example: `workspace-docs.log`,
  `workspace-doctests.log`.
- All eight cargo-fuzz targets build against the relocated library:
  `workspace-fuzz-build.log`.
- Python verification tools: 36 tests in `workspace-python.log`.
- An independent crate under `evidence/m8/api-consumer` uses only public APIs to
  create a section, traverse pages/runs, serialize identities, commit a Unicode
  edit, reject a stale snapshot through typed commit/I/O state, and reopen the
  exact resulting text. Output: `evidence/m8/api-consumer.log`.

The audit examined exported declarations and their implementations in storage,
revision resolution, document interpretation, creation, editing and persistence.
It does not freeze the API or turn the earlier native compatibility evidence into
a new platform guarantee.
