# Tags with Snowbound's art

`candidate/` is the notebook `tags_with_art_keep_it_beside_the_page` in
`crates/snowbound/src/tags.rs` writes: one page whose paragraphs carry "Launch" (a PNG
chosen for it, OneNote's Yellow Star, 13), "Ship it" (Snowbound's Rocket, OneNote's Plane,
127), "Done" (the Rocket over a Blue Check Box, 3, checked) and OneNote's own "Important".
The page stores OneNote definitions only; `.snowbound/tags.json` maps each tag's name and
symbol to its picture in `.snowbound/tags/`, named by SHA-256.

`cold/` is `tools/native_custom_art.py` in a fresh OneNote 2010 clone: its author script
gives `.snowbound` the hidden attribute Snowbound sets (the transfer archive drops it),
`before-read/` is the cold read, which shows each fallback symbol under the tag's name and
no `.snowbound` section group (`before-read/page-000.png`). OneNote then edits the page
through COM (`scripts/custom-art-edit.ps1`: new text on "Launch day", "Done" unchecked)
and syncs; `read/` is the page after, `edited.png` its render, and `sidecar.json` what
OneNote left of the folder: still hidden, every file as written, no table of contents.
`tools/test_custom_art.py` and `art_outlasts_onenote_editing_the_page` check this without
a VM. Regenerate with `SNOWBOUND_CUSTOM_ART_EXPORT` set to a new absolute directory while
running the test, then `tools/native_custom_art.py OUTPUT COLD`.
