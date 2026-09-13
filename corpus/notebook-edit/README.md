# Notebook structure

`native-structure/` is OneNote 2010 building structure through the COM API on
a Rust-created notebook (`tools/native/notebook-structure.ps1`): it created
two sections and a section group with a section inside, renamed the notebook,
reopened it, and deleted a section; `structure-*.xml` are the hierarchy reads
after each step, `notebook-renamed/` the files before the delete. OneNote
keeps the documented table of contents (MS-ONE 2.2.14): one root object with
an entry array whose entries carry the file identity, order, filename and
colour (`0xffffffff` for sections, absent for groups); a section's colour
lives in its own metadata. Deleting moves the file into `OneNote_RecycleBin`,
a group with its own TOC that the root lists. OneNote also names every file
for its place in the header: `guidAncestor` is the parent TOC's file
identity and `crcName` the CRC of the section file name or the group folder
name. The Rust section `links.one`, created with a zero ancestor, was
re-identified on open and listed a second time under its new identity.

`native-reorder/` is the COM API asked to move the last root section first
through `UpdateHierarchy` (`tools/native/notebook-reorder.ps1`): the
in-session read shows the new order, but after `SyncHierarchy`, close and
reopen the order is back and the TOC unchanged, so section order cannot be
authored through COM; the order rows below are proved by cold reads of Rust
candidates alone. Section display order is the entry's ordering number
ascending, sections before groups, which the owner's own notebook confirms
(array order there differs from the numbers, and OneNote lists by number).

`structured/` is the notebook `crates/notebook/tests/structure.rs` builds
through `notebook::session::Notebook` (create sections and a group, rename
both, colour a section, order the root as group, renamed, first), placed the
same way; `deleted/` is the same notebook after deleting a section. Each
`cold/` is a fresh OneNote 2010 read: the hierarchy lists the entries in the
written order with the written colours, the recycle bin carries
`isRecycleBin`, and every file keeps the identity Rust wrote.
Every TOC revision the writer appends carries one global id table; with one
table per object group, as the section writer emits, OneNote applied a
reordered entry's value to the wrong entry. `tools/test_notebook_edit.py`
checks this without a VM. Regenerate with
`NOTEBOOK_STRUCTURE_EXPORT` and `NOTEBOOK_STRUCTURE_EXPORT_DELETED` set to
new directories while running the test, then cold-open each with
`tools/native_runner.py OUTPUT COLD --expected-pages -1 --collect-notebook`.
