# Notebook management

`candidate` is the notebook `a_notebook_is_managed_as_onenote_manages_one` in
`crates/snowbound/src/manage.rs` makes through the calls the app makes: `Notebook::create`
(a table of contents in the notebook colour OneNote gave new notebooks, and "New Section 1"
holding a page with OneNote's title date and time fields); new sections in OneNote's
new-section colour order and groups; pages created with `SectionOp::Create`, titled with
`Text`, given our Ivy art as a background picture (`template_ops`) or the Teal page colour
(`PageOp::Color`), and made a subpage; a page given Informal Meeting Notes (the art and
the template's outlines, lowered to ops from the page model `meeting.rs` builds); a page deleted to the recycle bin
(`Notebook::recycle_pages`, then `SectionOp::Delete`); a page moved to another section
as a drop on its tab moves it (`notebook::session::moved`, then `SectionOp::Delete`); a section renamed and moved into a
group, the group renamed, a group and a section deleted to the recycle bin, and the root
reordered.

`cold` is its fresh OneNote 2010 read: the hierarchy lists every section, group and page
where the app put it, with its colour; the recycle bin holds "Deleted Pages"
(`isDeletedPages`) with the deleted page, and the deleted group's and section's files; the
Teal page reads back `color="#D4F9F2"`, the Ivy page one background image, the Meeting
page OneNote's four outlines with their numbering, to-do tag and bullets, and the
screenshots show the art, the colour and each title's date and time. OneNote left the
files' identities as written. `tools/test_notebook_management.py` checks this without a VM.

`native` is what OneNote 2010 did through its own interface on the VM. `new-notebook`:
File, New made "New Section 1" with an "Untitled page" titled with the date and time
(`created`); New Section named sections "New Section N"; a section dragged onto a group
moved into its folder (`moved`); deleting a page copied it into
`OneNote_RecycleBin/OneNote_DeletedPages.one` (grey) with its identity, title and
creation time, and deleting a group moved its sections into the recycle bin and removed
its folder (`final`). `section-colors`: sixteen sections made with New Section, whose
colours are the order new sections take. `page-color`: View, Page Color stores
`0x14001d2a` (COLORREF) on the page node and "No color" removes it; `colors.txt` is each
of the sixteen colours as the page XML reports it, in the menu's order. `meeting-template`
is the page XML of a page made from Business, Informal Meeting Notes.

Regenerate with `SNOWBOUND_MANAGEMENT_EXPORT` set to a new directory while running the
test, then `tools/native_runner.py OUTPUT COLD --expected-pages -1 --collect-notebook
--screenshots`.
