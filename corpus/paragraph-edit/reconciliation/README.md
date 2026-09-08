# Native paragraph reconciliation controls

`before` contains sixteen OneNote-authored pages and one unchanged source page.
`tools/native/paragraph-reconciliation.ps1` creates these cases and applies the
separate COM replacement controls retained in `remote`. Whole-page COM updates
replace rich-text identities, including unchanged text. Every local split/join
and dependent edit therefore remains a conflict without remote publication.

`keyboard` begins from the same notebook and records actual native keystrokes,
screenshots, XML, and saved bytes. Prefix/boundary edits, children, bullets, tags,
formatting, sibling replacement, and empty-paragraph typing exercise independent
changes. Bullets alter ancestry: the first paragraph gains an outline group,
and a later bullet becomes a child of its preceding paragraph. Replacing a whole
selected sibling creates a new paragraph; the split/join targets retain identity.

`reconciled` retains Rust output and a fresh OneNote cold-open. Six cases publish
automatically; eight publish after explicit review. Each includes a dependent
text edit, with SQLite reopen between local acknowledgements and publications.
The changed empty-left survivor and nonadjacent bulleted join retain conflicts.
Public tests check all 28 receipts, exact active graph/child levels, native
characters/styles, and the saved graph's identities. COM replacement controls
also retain all sixteen local branches across reopen without publishing.

Regenerate keyboard controls with `tools/native_runner.py` on `before/notebook`,
using `--expected-pages 17 --inspect --collect-notebook`. Once inspection is
ready, run `tools/native_paragraph_reconciliation.py OUTPUT`; it records actions
and requests final capture. The runner removes its owned clone. The ignored
Rust test `paragraph::export_native_reconciliation` generates reconciled output
at a new absolute `ONESTORE_NATIVE_RECONCILIATION_OUTPUT` directory. Cold-open its
`candidate` with the same runner, without inspection. Verification lives in
`tools/test_paragraph_edit.py` and the offline crate's `tests/sync.rs`.

The original COM pass stopped at its fifth case because its tag definition
followed page settings. The recorded continuation fixes the schema order and
applies only the remaining twelve cases. Scripts, commands, and both phase
hashes retain this provenance; the before-image was preserved throughout.
Identical captured files link to their canonical corpus copy.
