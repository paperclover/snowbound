# Note tags on tables through the editor

`native/` is OneNote 2010 authoring `tools/native/pages.ps1` input: three pages in
`Tables.one`, each a table between two paragraphs, tagged To Do (unchecked) or
Important, the last also holding a To Do-tagged table in a cell. OneNote stores a
table's tags on the table object, and draws them in the tag column centred on it.

`candidate/` is the output of `tagged_tables_draw_check_and_survive_edits` in
`crates/canvas/tests/table_tags.rs`: on "Check a table" a click checks the table's
box; on "Edit a tagged table" text is typed and Enter pressed in a cell; "Delete and
undo" has all its content deleted and the deletion undone, each published as its own
revision.

`cold/` is a fresh OneNote 2010 read: the first table's To Do is completed, the edited
table keeps its tag, and the restored tables read as OneNote authored them.
`tools/test_table_tags.py` checks this without a VM. Regenerate with
`SNOWBOUND_TABLE_TAGS_EXPORT` set to a new absolute directory while running the test,
then cold-open it with
`tools/native_runner.py OUTPUT COLD --expected-pages 3 --collect-notebook --screenshots`.
