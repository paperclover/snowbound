# Pages copied between sections

`candidate/copies.one` is the destination section
`pages_copy_into_another_section_with_their_content_and_fresh_identities` in
`crates/notebook/tests/copy.rs` builds: a section created in Rust into which
`Section::import_page` copied one page from each of eight earlier rows (text
with an attachment and its icon, a table, an inserted picture, a page-level
ink drawing, note tags, bullets and numbering, equations, paragraph
formatting) and the `table-edit/nested` pages whose indent levels the model
can express (nested tables, cell subtrees, mixed languages; four outdent
pages need outline groups and refuse), each as a page creation and a save
queued through the replica
and published to the file; the last copy (paragraph formatting) was then deleted permanently
through `Section::delete_pages` (`expected-count.txt` is the page count
left). Every object carries a fresh identity (`Page::copy`), paragraph
styles become quick-style objects the writer creates from the copied
definitions (OneNote exports their font, size and colour through
`QuickStyleDef`, not on the runs), and payloads travelled inside the queued
intents.

`cold/` is a fresh OneNote 2010 read with a screenshot per page
(`read/page-NNN.png`). `tools/test_page_copy.py` checks the content oracle
and the page set without a VM. Regenerate with `NOTEBOOK_COPY_EXPORT` set to
a new directory while running the test, then cold-open with
`tools/native_runner.py OUTPUT COLD --expected-pages 9 --collect-notebook
--screenshots`.
