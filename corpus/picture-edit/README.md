# Rust picture authoring

`inserted/candidate` is the page writer's output for
`a_picture_inserted_on_a_fresh_page_reads_back_and_can_be_removed` in
`crates/onestore/tests/page_images.rs`: on a section created in Rust, a
paragraph between two text paragraphs becomes a picture, stored the way
OneNote stores an inserted picture (`corpus/native/cold-05-05-image`): the
PNG embedded in the section's file-data store list as a file-data store
object, a picture-container file object declaring it by identity and
extension, and a picture object with the displayed size that the paragraph
holds as content. OneNote's own picture objects also carry a DPAPI-protected
blob (`0x1c001dfb`) that only the authoring Windows user can decrypt; the
writer omits it. `inserted/cold` is a fresh OneNote 2010 read: the image
reads back as `format="png"` with the exact payload bytes, between the two
text paragraphs.

`native-resize` is OneNote 2010 editing that Rust-written picture
(`tools/native/picture-edit.ps1` on `inserted/candidate`): the picture gets
a 144 by 108 point user-set size and alternative text through the COM API
(`before.xml`, `update.xml`), and `read/` is the read after the edit. In
`notebook/`, OneNote stored the size as the picture object's layout width
and height with the user flag (`0x14001c1b`, `0x14001c1c`, `0x08001cbd`) and
left the intrinsic size (`0x140034cd/ce`) alone; it also rewrote the picture
container under a native identity with its DPAPI blob and hash.

`resized/candidate` is the writer doing the same on the native picture of
`cold-05-05-image` (`a_native_picture_is_resized_and_described_then_reset`):
layout width, height, user flag and alternative text on the picture object.
`resized/cold` is its cold read with the size and description.

`tools/test_picture_edit.py` checks all three without a VM. Regenerate the
candidates with `ONESTORE_IMAGE_EXPORT` and `ONESTORE_IMAGE_RESIZE_EXPORT`
set to new absolute directories while running the tests, then cold-open
them with `tools/native_runner.py OUTPUT COLD --expected-pages 1
--collect-notebook`; regenerate `native-resize` with `--author
tools/native/picture-edit.ps1` on `inserted/candidate`.
