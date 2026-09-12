# Rust picture authoring

`candidate/` is the page writer's output for
`a_picture_inserted_on_a_fresh_page_reads_back_and_can_be_removed` in
`crates/onestore/tests/page_images.rs`: on a section created in Rust, a
paragraph between two text paragraphs becomes a picture, stored the way
OneNote stores an inserted picture (`corpus/native/cold-05-05-image`): the
PNG embedded in the section's file-data store list as a file-data store
object, a picture-container file object declaring it by identity and
extension, and a picture object with the displayed size that the paragraph
holds as content. OneNote's own picture objects also carry a DPAPI-protected
blob (`0x1c001dfb`) that only the authoring Windows user can decrypt; the
writer omits it.

`cold/` is a fresh OneNote 2010 read: the image reads back as `format="png"`
with the exact payload bytes, between the two text paragraphs.
`tools/test_picture_edit.py` checks this without a VM. Regenerate with
`ONESTORE_IMAGE_EXPORT` set to a new absolute directory while running the
test, then cold-open it with `tools/native_runner.py OUTPUT COLD
--expected-pages 1 --collect-notebook`.
