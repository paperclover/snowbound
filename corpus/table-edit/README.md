# Rust table authoring

`created/candidate` is the page writer's output for
`a_new_table_is_created_on_a_fresh_page` in
`crates/onestore/tests/page_tables.rs`: on a section created in Rust, a
paragraph between two text paragraphs becomes a two-by-two table with locked
144 pt and 216 pt columns, visible borders and a text paragraph in every
cell. `created/cold` is a fresh OneNote 2010 read showing both columns with
their widths and locks, both rows and every cell text.

`edited/candidate` is the writer's output for
`a_row_and_a_column_are_added_to_a_native_table_and_removed_again` on
`corpus/outline-edit/tree/before`: the one-row, two-column native table on
"Delete cell subtree" gains an unlocked 120 pt third column and a second row
of three cells. `edited/cold` is its cold read: the native columns keep their
widths and locks, OneNote sizes the unlocked column to its content, and the
new row and cells read back with their text.

`nested/candidate` is the writer's output for
`a_table_nests_inside_a_cell_and_is_removed_again` on the same tree fixture:
the second cell of the native table gains a paragraph holding a one-column,
two-row table with locked 72 pt column and visible borders. `nested/cold` is
its cold read with both tables.

Native rows and cells are plain containers; the writer creates them with the
modification time, the indent array copied from a sibling cell, and the flags
every native cell carries, rebuilds each table's row and cell order to the
model's, and writes the column count, widths, locks and border flag. New
tables start as an empty paragraph whose content the structure pass replaces.
An emptied cell keeps a replacement paragraph, as the tree writer provides.
`tools/test_table_edit.py` checks all three captures without a VM. Regenerate
with `ONESTORE_TABLE_EXPORT`, `ONESTORE_TABLE_EDIT_EXPORT` and
`ONESTORE_NESTED_TABLE_EXPORT` set to new absolute directories while running
the tests, then cold-open them with `tools/native_runner.py OUTPUT COLD
--expected-pages 1|12 --collect-notebook`.
