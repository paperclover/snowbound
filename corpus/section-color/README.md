# Section and notebook colour

`native` is what OneNote 2010 did on the VM (2026-09-30). A section tab's context menu
ends with Section Color, sixteen named colours and None (`section-color-menu.png`);
`colors.txt` is each as the hierarchy XML reports it, in the menu's order. Picking one
appends a revision to the section's own file setting its metadata's `0x14001cbe`
(COLORREF; `teal.one`), and None sets `0xFFFFFFFF` (`none.one`); the table of contents is
left byte for byte. Notebook Properties (`notebook-properties.png`, from the notebook's
Rename or Properties) offers the same sixteen without None (`notebook-colors.png`) and
appends a revision to the root `Open Notebook.onetoc2` setting its root's `0x14001cbe`
(`before.onetoc2`, `after.onetoc2`). Its Display name is written to neither file: OneNote
keeps it in its local `OneNoteOfflineCache.onecache`, so the hierarchy's `nickname`
changes and `name`, the folder, stays.

`candidate` is `crates/snowbound/src/menus.rs`'s notebook, exported with
`SNOWBOUND_SECTION_COLOR_EXPORT`: a section per colour and None, each made then coloured
with `Notebook::set_section_color`, and the notebook coloured Teal with
`Notebook::set_color`. `cold` is a fresh OneNote 2010's read: every section and the
notebook in the colour it was given. `tools/test_section_color.py` checks it without a VM.
