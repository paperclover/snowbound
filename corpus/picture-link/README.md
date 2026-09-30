# Pictures with a link

`lab/` holds screenshots of OneNote 2010 in a disposable clone of
`corpus/m6/native-features-01`, on the page "Image png", whose picture stores a link
(WzHyperlinkUrl, the COM read's `hyperlink`). Hovering it shows the address and "Ctrl+click
to follow link" (`hover.png`); a click selects the picture (`click-selects.png`); Ctrl+click
opens the address, here an unreachable one (`ctrl-click.png`). Snowbound follows the same
link on Ctrl+click, Command+click on macOS.

`candidate/` is the output of `a_linked_picture_keeps_its_link_through_undo` in
`crates/canvas/tests/pictures.rs`: the picture deleted and brought back by undo, published
as one revision. `cold/` is a fresh OneNote 2010 read, whose picture still links there.
`tools/test_picture_link.py` checks this without a VM. Regenerate with
`CANVAS_PICTURE_LINK_EXPORT` set to a new absolute directory while running the test, then
cold-open it with `tools/native_runner.py OUTPUT COLD --collect-notebook`.
