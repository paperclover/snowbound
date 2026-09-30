# Deleting across a table's edge

`native/` is OneNote 2010 authoring `tools/native/pages.ps1` input: four pages in
`Cross.one`, each "Before text", a two-by-two table and "After text".

`keys/` is `tools/native_cross_container.py` pressing the same keys in OneNote 2010 on
that notebook: a selection from "Before" into the first cell deleted, one from the
second cell out to "After" deleted and "X" typed, one from "Before" over the first row
into "Beta one" typed over with "Y", and one from "Before" over the whole table deleted.
OneNote joins nothing across the table's edge, empties cells inside the selection, and
where the selection runs on past the table removes its rows inside, and the table when
all of them are.

`candidate/` is the output of `deleting_across_a_tables_edge_keeps_each_side` in
`crates/canvas/tests/cross_container.rs`, the same edits through the editor, each
published as its own revision; `cold/` is a fresh OneNote 2010 read of it, whose pages
read as `keys/` does. `tools/test_cross_container.py` checks this without a VM.
Regenerate with `SNOWBOUND_CROSS_CONTAINER_EXPORT` set to a new absolute directory while
running the test, then cold-open it with
`tools/native_runner.py OUTPUT COLD --expected-pages 4 --collect-notebook --screenshots`.
