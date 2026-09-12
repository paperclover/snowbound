# Rust attachment authoring

`plain/candidate` and `icon/candidate` are the page writer's output for
`an_attachment_inserted_on_a_fresh_page_reads_back_and_can_be_removed` in
`crates/onestore/tests/page_attachments.rs`: on a section created in Rust, a
paragraph between two text paragraphs becomes an inserted file
(`notes 🦀.txt`, 28 bytes), stored the way OneNote stores one
(`corpus/native/cold-05-06-attachment`): the payload embedded in the
section's file-data store list, an embedded-file object declaring it by
identity and extension, and an attachment object with the file name, the
24 pt icon size and the flags every native attachment carries. `icon/` also
embeds the native fixture's 724-byte icon PNG as the preview picture;
`plain/` omits it, which OneNote accepts (it renders its own icon).
OneNote's integrity check refuses an attachment object without
`0x1c001d61` (twenty bytes: 16, 1, zeros), so the writer always stores it.

`*/cold` are fresh OneNote 2010 reads: the file reads back as an
`InsertedFile` with the preferred name and the exact payload bytes, between
the two text paragraphs. `tools/test_attachment_edit.py` checks both captures
without a VM. Regenerate with `ONESTORE_ATTACHMENT_EXPORT` set to a directory
while running the test (it writes `plain/` and `icon/`), then cold-open each
with `tools/native_runner.py OUTPUT/plain COLD --expected-pages 1
--collect-notebook`.
