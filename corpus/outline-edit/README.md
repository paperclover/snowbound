# Native outline editing controls

`empty-children` retains a mixed-client deletion regression: `before.one` exposes
transaction 22 of the retained local image; `remote.one` was observed after native
OneNote removed an empty child-list property and coalesced equivalent immutable
styles. The retained intent addresses the same subtree in both files. The offline
test requires deletion and its dependent edit to publish after reopening the
cache, while an actual remote text change retains a content conflict.

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

`tree` extends the native controls with eleven cases involving outline groups,
indentation gaps, numbered/bulleted subtrees, and table cells. The stored graphs,
identities, tags, fields, and formatting survive a fresh-cache reopen. Deleting
a group's last subtree removes the empty group. Removing the last unindented
sibling unwraps the preceding group and transfers its indentation to the outline.
Deleting a cell's only paragraph creates new empty paragraph/text identities;
the cell and table remain intact. Outdenting across a two-level gap reduces the
gap and reparents a following sibling beneath the outdented target, preserving
that sibling's indentation. Native bullet indentation additionally substitutes
its list marker; it is distinct from moving a subtree with unchanged formatting.

These keyboard controls retain rich-text identities. Eight edits also establish
a 423.75-point reserved wrapping width on the selected outline in the captured
800×600 desktop. The test permits that specific property separately from
timestamp/bookkeeping changes and verifies all other retained object fields.
Before/after/cold native comparisons cover twelve pages, eleven/six/six tags,
and 8,321/7,081/7,081 explicit character-format values.

Regenerate using the same source notebook and runner, with
`--author corpus/outline-edit/tree/scripts/author.ps1 --expected-pages 12
--inspect --collect-notebook`. Run the shared `tools/native_outline_editing.py`
controller at inspection readiness, move `before-read` to `before/read`, then
cold-open the returned notebook with `--expected-pages 12 --collect-notebook`.
The exact authoring script has one home in the reference corpus.

`rust-tree` retains four Rust-generated notebooks and their fresh-cache OneNote
captures. `ordinary` and `groups-cells` apply eighteen moves/deletions from the
native controls. Explicit bullet-subtree movement preserves its original marker.
`cross-container` moves content between cells and outlines, moves a whole table,
and moves a sibling beneath a paragraph while its enclosing group is normalized.
`unequal-groups` deletes a trailing paragraph after two differently indented
groups; native XML reports the surviving paragraphs at indentation levels three
and two. Every surviving content identity, field, tag and table remains intact;
an emptied cell receives new empty paragraph/text identities.

The four captures cover forty pages. Public tests compare native text and
formatting, the complete active graphs before/after cold reopen, metadata, and
list objects. Core tests additionally compare eighteen native transformations,
every historical object's property bytes, selected move attribution, protected
content, reused intents, and interrupted publication. The shared stateful model
in `crates/onestore/tests/support/tree_model.rs` checks paragraph order,
indentation, content, automatic titles and old/new publication outcomes across
twelve cached clients; `fuzz/fuzz_targets/tree.rs` runs that oracle under ASan.

Set `ONESTORE_TREE_OUTPUT` to a new absolute directory and run the
`native_subtree_controls_match_with_preserved_fields_and_history` and
`cross_container_moves_keep_tables_and_replace_emptied_cells` integration tests,
plus the `tree::tests::group_normalization_preserves_unequal_indentation_and_overlapping_moves`
unit test. Cold-open the resulting `ordinary`, `groups-cells`, `cross-container`
and `unequal-groups` notebooks with expected page counts 15, 12, 12 and 1,
respectively, using `tools/native_runner.py --collect-notebook`.

`offline-tree` reconciles moves/deletions and dependent text edits against the
fourteen ordinary native controls. Three already-converged moves confirm without
publication, seven competing structure/content changes require explicit review,
and four deleted targets retain the complete local branch. The cold capture
preserves all 247 active graph objects across fifteen pages, three tags, and
6,497 explicit character-format comparisons per image. Its `cases.json` records
the native input cases and outcomes. Regenerate with the offline sync test
`native_tree_and_layout_changes_reconcile_without_discarding_unreviewed_content`,
setting `ONESTORE_OFFLINE_TREE_OUTPUT` to a new absolute directory, then cold-open
its `candidate` with `--expected-pages 15 --collect-notebook`.

Its `cell-delete` and `cell-move` captures additionally verify local edits to an
emptied cell's replacement paragraph while preserving remote text in the other
cell. They retain 272 and 274 active objects, respectively, with 8,201 and 8,377
explicit format comparisons per image. Regenerate with the offline sync test
`emptied_cell_replacement_is_durable_and_cannot_be_silently_omitted_on_replay`,
using the same output variable; cold-open each candidate with twelve expected pages.
