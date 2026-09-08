# Native outline editing controls

OneNote 2010 authored fourteen cases plus an unchanged source page. `before`,
`after`, and `cold` retain notebook bytes and independent native XML. The final
phase uses a fresh application cache. Cases preserve Unicode, hyperlinks, tags,
nested descendants, siblings, and a separate outline.

The controls cover leaf/subtree reordering, subtree indentation/outdentation,
stored collapse/expansion, deletion of a leaf/subtree/sole paragraph/outline,
outline position, fixed width, and automatic width. Keyboard subtree operations
explicitly select descendants. Deleting the sole paragraph removes its empty
outline. The native application sometimes advances unchanged text timestamps
and adds `0x880034dd`; the test permits only those observed bookkeeping changes.

COM outline replacement reallocates contained rich-text identities. Keyboard
movement retains them. Native XML can reorder outlines by their positions while
the file's page child order remains unchanged. Explicit width sets its native
user-size flag; automatic width clears that flag. OneNote regenerates height
from content instead of retaining the requested height. These observations
constrain typed operations independently of the native COM replacement behavior.

Persisted collapse values survive a cold open. Native keyboard collapse/expand
can instead change only the client's cached view, so warm COM XML alone does
not prove a stored-property edit. The controls here explicitly change the
persisted default through COM.

To regenerate, run `tools/native_runner.py` on
`corpus/formatted-insertion/native-paragraph/candidate` with a new output directory,
`--author tools/native/outline-editing.ps1 --expected-pages 15 --inspect
--collect-notebook`. After inspection is ready, run
`tools/native_outline_editing.py OUTPUT`. It freezes the before-image, records
the actual keyboard/XML inputs, and requests final capture; the runner removes
its clone. Move the returned `before-read` directory into `before/read`.
Cold-open `OUTPUT/notebook` into another new output directory with the runner,
`--expected-pages 15 --collect-notebook`. Public verification lives in
`tools/test_outline_edit.py`.

The controller selects the target again after activating the native window and
transfers XML as UTF-8 data. Captured authoring scripts retain their exact source;
identical immutable artifacts link to one canonical corpus copy.

`layout/candidate` applies `PreparedEdit::outline` to the five geometry and saved
collapse cases in `before`. `layout/cold` captures that Rust output through a fresh
OneNote cache. Every active identity, child/content/structure reference, child level,
and collapse default survives; text, fields, tags and explicit formatting compare
against native XML. Fixed width renders at 144 points. The automatic 360-point hint
renders narrower for this content. Stored height remains a hint; native XML reports
the content-derived height. All fourteen associated tags and 9,577 explicit format
comparisons pass for both candidate and native-saved images.

Regenerate the layout candidate with the ignored `export_native_outline_candidates`
test in `crates/onestore/tests/outline.rs`, setting `ONESTORE_OUTLINE_OUTPUT` to a
new absolute directory. Cold-open that directory with `tools/native_runner.py`,
`--expected-pages 15 --collect-notebook`. The captured clone is removed by the runner.

`reserved-width` starts from native `after`, whose moved outline has a reserved
wrapping width. Rust replaces it with an explicit 144-point width and clears the
obsolete reservation. Native renders at 144 points after a cold reopen, retaining
all 304 active graph objects, ten tags and 8,457 explicit formatting comparisons.
To regenerate, run `resizing_a_native_reserved_width_preserves_content` with
`ONESTORE_RESERVED_WIDTH_OUTPUT` set to a new absolute directory, then cold-open
it with the same runner arguments.

`offline` reconciles local layout/default intents and later text edits against all
fourteen native changes. Five merge automatically, five require explicit review,
and four deleted targets retain their local branches and dependent queues. The ten
successful cases produce eighteen publications and twenty durable receipts; the
already-converged collapse defaults require confirmation without another append.
The final cold capture preserves all 304 active graph objects, ten tags, and 8,937
explicit formatting comparisons. The `cases.json` records stable targets and the
retained/dependent outcomes used by the public test.

Regenerate by running the offline sync test
`native_moves_deletions_and_layout_changes_merge_or_retain_explicit_conflicts`
with `ONESTORE_OFFLINE_OUTLINE_OUTPUT` set to a new absolute directory, then use
the same cold runner arguments. The test also retains a JSON record next to that
directory. Cache migration, uncertain revision retention, stale review, and twelve
offline writers are exercised in `crates/onestore-offline/tests`.
