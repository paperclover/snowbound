# Table column widths through the editor

`candidate/` is the output of `typed_and_dragged_columns_store_onenote_s_widths` in
`crates/canvas/tests/table_widths.rs`, editing `corpus/table-tags/native`. On "Check a
table" a three-by-three table is typed after the text a keystroke per revision, each
storing its column's fitted width with its text; on "Edit a tagged table" a native cell
is typed into, widening its column; on "Delete and undo" a new table's first column is
dragged to 96 pt, locking it, and a line long enough to reach the outline's width is
typed in the second.

`cold/` is a fresh OneNote 2010 read: the typed table's columns are the widths OneNote
gives the same table typed in it (60.76, 37.11 and 139.09 pt), the widened native column
keeps its width, the dragged column stays 96 pt and locked, and the long line wraps in
its column at the outline's edge. `tools/test_table_widths.py` checks this without a VM.
Regenerate with `SNOWBOUND_TABLE_WIDTHS_EXPORT` set to a new absolute directory while
running the test, then cold-open it with
`tools/native_runner.py OUTPUT COLD --expected-pages 3 --collect-notebook --screenshots`.
