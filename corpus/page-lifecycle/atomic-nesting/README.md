# Atomic page nesting

The candidate nests `child` and `grandchild` from `../03-renamed` beneath the
`Same title` page, at levels 2 and 3. Section series order, section metadata
copies and the two page metadata revisions publish in one transaction. Page
bodies and prior revisions retain their identities and contents.

`../04-nested` is the independent native control. `cold` is a fresh OneNote
reopen of the Rust candidate; all active page and section objects compare exactly.
The files differ in 48 header bytes, including file identity and version metadata;
all bytes after the header are identical. Both images are retained. The capture's
owned clone was removed; `provenance.json` records its source and teardown.

Set `ONESTORE_PAGE_BATCH_OUTPUT` to a new directory and run the core unit test
`write::tests::multi::nesting_publishes_section_order_and_page_levels_in_one_transaction`
to export another candidate. Capture it with `tools/native_runner.py INPUT OUTPUT
--expected-pages 9 --collect-notebook`. Run `tools.test_page_lifecycle` after
building workspace examples to compare native navigation and document content.

This fixture verifies the internal transaction writer. It does not define a
public page operation, offline conflict policy or implicit subpage movement.
