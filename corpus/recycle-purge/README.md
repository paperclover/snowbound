# Recycle bin purge

OneNote 2010 deletes for good what its Notebook Recycle Bin has held "after 60
days" (`DaysToKeepRecycledItems`), a few minutes after it starts. Observed in a lab
clone on 2026-09-30, with the clock moved to age pages and sections:

- it judges by when the content last changed, not when it was deleted: pages
  created 70 days back and deleted a minute before were purged, a page deleted
  that day but changed that day was kept;
- a page goes from `OneNote_DeletedPages.one` in one appended revision;
- a binned section goes as a file once none of its pages changed in 60 days, its
  file's own times aside; the bin's TOC keeps listing it, unchanged.

`candidate/` is the notebook
`the_recycle_bin_forgets_pages_and_sections_unchanged_for_sixty_days` in
`crates/notebook/tests/structure.rs` exports with `NOTEBOOK_PURGE_EXPORT`: a page
and a section deleted into the bin, then purged as 61 days on. `cold/` is a fresh
OneNote 2010's read: Deleted Pages empty, the section gone, no repair, every file
left as written. `tools/test_recycle_purge.py` checks it without a VM.
