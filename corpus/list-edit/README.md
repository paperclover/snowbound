# Rust list authoring

`candidate/` is the page writer's output for
`bullets_numbering_nesting_and_restarts_publish_on_a_fresh_page` in
`crates/onestore/tests/page_lists.rs`: on a section created in Rust, the first
paragraph gains a bullet (glyph U+25CB in Courier New, bullet index 4, 11 pt),
then two numbered paragraphs, a nested numbered child, a numbered paragraph
restarting at three, a nested plain paragraph and a trailing plain paragraph.
The numbering definition is the upper-roman `##.` node copied from
`corpus/outline-edit/tree`. Every list node is written with the model's
identity after squash, so the reread model equals the saved one.

`cold/` is a fresh OneNote 2010 read: the bullet renders (reported as bullet
index 3 at 11 pt), the numbered paragraphs read `I.` and `II.`, the nested
child restarts at `I.`, the restarted paragraph reads `III.`, and the plain
paragraphs carry no list. `tools/test_list_edit.py` checks this without a VM.

A list node whose compact identity would be the null identity (sequence zero
at table index zero) fails OneNote's integrity check on open; the writer
allocates list nodes like other new objects and renames them to the model's
identities in squash. Regenerate with `ONESTORE_LIST_EXPORT` set to a new
absolute directory while running the test, then cold-open it with
`tools/native_runner.py OUTPUT COLD --expected-pages 1 --collect-notebook`.
