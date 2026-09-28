# Recycle bin repair

`candidate` is the notebook
`a_page_delete_lists_the_recycle_bin_it_finds_or_makes_and_leaves_an_unreadable_one` in
`crates/notebook/tests/structure.rs` exports through `NOTEBOOK_RECYCLE_EXPORT`: a page
deleted into a new recycle bin whose name the root TOC still listed for a bin gone from the
folder (the stale entry goes, the new bin is listed), then deleted again into a bin whose
TOC OneNote might name `OneNote Table Of Contents.onetoc2` and which did not list
`OneNote_DeletedPages.one` (the bin's TOC is kept, the deleted pages placed under it and
listed).

`cold` is its fresh OneNote 2010 read: the recycle bin (`isRecycleBin`) holds "Deleted
Pages" (`isDeletedPages`) with both deleted pages, and OneNote left every file byte for
byte as written, so it repaired nothing. `tools/test_recycle_bin_repair.py` checks this
without a VM.

Regenerate with `NOTEBOOK_RECYCLE_EXPORT` set to a new directory while running the test,
then `tools/native_runner.py OUTPUT COLD --expected-pages -1 --collect-notebook
--screenshots`.
