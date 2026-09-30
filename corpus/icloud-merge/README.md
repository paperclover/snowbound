# A conflict version merged

`candidate` is `a_version_of_a_onenote_file_merges_into_it` in `crates/notebook/tests/cloud.rs`
(`ONESTORE_ICLOUD_EXPORT`): two devices edit `conflict-page/native/initial` through a fake
iCloud Drive. The Mac types "Mac: " at the start of the first paragraph; the iPhone, offline,
types "iPhone: " at the same place, appends to the positioned outline and adds a page "From the
iPhone" with a body. The iPhone's upload becomes a conflict version, which the Mac merges: the
appended text and the new page replay, and the clashing edit becomes a conflict page.

`cold` is OneNote 2010's fresh read in a disposable clone (`tools/native_runner.py`,
`--screenshots`): both pages list, the first with its conflict mark and the yellow bar "This page
has changes that could not be merged during synchronization", its positioned outline ending
"(typed on the iPhone)", and "From the iPhone" holding "Written offline on the iPhone."
