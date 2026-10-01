# Recycle bin view

OneNote 2010's Notebook Recycle Bin (Share, observed in a lab clone on 2026-10-01): the bin
opens as its own row of tabs, Deleted Pages then each deleted section, headed `Recycle Bin for
"Notebook":`, read-only under a bar ("To restore a page or section, right-click and move it out
of this Recycle Bin. Content here is deleted after 60 days."). A page's Move or Copy takes it to
any section; a section's moves it into the notebook or a group, its file moving there. Empty
Recycle Bin asks "Are you sure you want to empty the Recycle Bin for this notebook?", deletes
Deleted Pages' pages in one revision and each binned section's file, and leaves the bin's TOC
byte for byte, its entries naming files no longer there.

`candidate/before` is the notebook
`pages_and_sections_leave_the_recycle_bin_as_onenote_restores_them` in
`crates/snowbound/src/recycle.rs` exports with `SNOWBOUND_RECYCLE_EXPORT`: three pages and five
sections deleted into the bin. `candidate/after` is it once Snowbound restored a page (its
identity kept), copied one out (a new identity), deleted one for good, restored a section to the
notebook and emptied the bin. `cold/before` and `cold/after` are fresh OneNote 2010 reads of
each: everything where it was meant to be, every file left as written.
`tools/test_recycle_bin_view.py` checks them without a VM.
