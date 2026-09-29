# Files on the page

`native/files.one` is OneNote 2010's own section, made in a lab clone on
2026-09-29. Its second page holds two copies of `float.txt` (38 bytes) outside
any outline: one inserted through COM `UpdatePageContent` as a page-level
`InsertedFile` at (216, 180) and then dragged to (288, 140.4), the other
attached with Insert > Attach File after a click on blank page, at (342, 284.4).

`candidate/` is `crates/canvas/tests/attachment_insert.rs`'s second section:
the editor places the caret on blank page at (342, 284.4) and attaches
`float 🦀.txt` (37 bytes, with the icon OneNote stored for a text file), then
at (126, 410.4) attaches `large.bin` (2 MiB of noise, with the canvas's
blank-page icon) and drags it to (288, 140.4). Each step is its own edit of the
ops the editor recorded, sealed into one appended revision. `cold/` is a fresh
OneNote 2010 read: both files are page-level `InsertedFile`s at those
positions, and the bytes OneNote materializes to open them
(`read/*.attachment`) hash as written. `tools/test_attachment_floating.py`
checks the capture without a VM. Regenerate with `CANVAS_FLOATING_EXPORT` set to
a directory while running the test, then `tools/native_runner.py DIR COLD
--expected-pages 1 --collect-notebook --screenshots`.

## What OneNote 2010 does

- **Placement.** Attached or dropped on blank page, the file becomes a child of
  the page at the point a click there puts the caret: on the 18 pt grid from
  the margin origin, 7 px above the pointer. It draws as a file in an outline
  does, its 32-pixel icon at 24 points centred in a 54-point column over its
  name without the extension, and selects as a shaded column framed in dashes,
  without handles.
- **Storage.** The attachment object (jcid 0x60035) carries what a paragraph's
  file does, except LayoutAlignmentInParent; its position (0x14001c14/15),
  LayoutAlignmentSelf 9, and the column's LayoutMaxWidth/Height (54 by about 63
  points) with the size not set by the user. A file inserted through COM has
  neither alignment nor size.
- **Moves.** Dragging rewrites the position alone, on the grid; the
  modification time stays.
- **Payloads.** Both copies' embedded-file objects, and both icons, name one
  payload in the file data store.
