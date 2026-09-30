# The Draw tab's tools

`native-ui/notebook/ink.one` is OneNote 2010's own work in a lab clone, drawn with the
mouse from the Draw tab (screenshots of the tab, the pen gallery and Color & Thickness
were taken alongside):

- "Pens": the first favourite pen (black, 35 HIMETRIC wide), a second stroke drawn at once
  beside it, the red thick pen, the yellow highlighter, and shapes: rectangle, line,
  arrow and oval with the default shape pen (black, 50 wide).
- "Gallery": one stroke with each of the 14 favourite pens in the gallery's order.
- "Edits": four strokes, the second removed by the stroke eraser, the third lassoed and
  dragged 60 by 20 pixels, the fourth lassoed and deleted, then a rectangle and a line.
- "Shapes2": arrows at other angles and pen widths, a rectangle moved after drawing, and
  lines in each of Color & Thickness's nine widths.

What it shows, and `crates/onestore/tests/page_ink.rs` pins:

- Every stroke and every shape is an ink container of its own, on top of the page; OneNote
  never adds a stroke to an earlier drawing. Erasing or deleting removes the container;
  moving sets its offset and leaves the strokes as they were.
- A pen's drawing attributes ignore pressure. A highlighter's tip is a 70 by 400
  rectangle (pen tip 1) with transparency 127 and raster operation 9 (MaskPen), and its
  stroke's bias is 2. Strokes are numbered through the page.
- Pen widths are 25, 35, 50, 70, 100, 150, 200, 350 and 500 HIMETRIC. The favourites are
  black, red `0x241ced`, blue `0xbb6531`, green `0x367d17` and grey `0x808080` at 35 and
  50, and yellow, cyan, green and magenta highlighters; black is stored as no colour.
- A shape's container stores 2 in `0x14001d4e` (1 for a drawn stroke), its kind in
  `0x0c001d4f` (11 a line or arrow, 12 a closed shape) and its geometry: a line's ends in
  half inches (`0x1c001dac`), or a closed shape's transform from the unit square in half
  inches and its anchors in the unit square (`0x1c001daa`: 8 for a rectangle, 4 for an
  oval). Its strokes carry only their path and pen, whose dimension table spans the whole
  32-bit range at resolution 1000. A rectangle is drawn clockwise from its top left, an
  oval in 50 steps clockwise from its right end, and an arrow's head is a second stroke of
  two barbs at atan(1/2) to the line, 8 points plus twice the pen width long. Shape
  corners snap to the 18 pt grid.

`native-ui/read` is a fresh OneNote 2010 read of that file.

`drawn/candidate` is the page view's output for
`drawing_tools_store_each_stroke_and_undo_it_in_one_step` in
`crates/canvas/tests/ink_tools.rs` (`CANVAS_INK_EXPORT`): a pen stroke in an accent
colour, a yellow highlighter, and a rectangle, oval, arrow and line dragged out with the
shape tools, each its own edit. `drawn/cold` is a fresh OneNote 2010 read: six
`InkDrawing`s, the four shapes with their `ShapeInfo` anchors, drawn as Snowbound drew
them (`page-000.png`).

`edited/candidate` is `onenote_drawings_take_erasing_moving_and_more_strokes` in
`page_ink.rs` (`ONESTORE_INK_TOOLS_EDIT_EXPORT`): the native file with the red zigzag on
"Pens" deleted, the vertical line moved by (36, 18), and the six drawings above added
below. `edited/cold` shows OneNote reading OneNote's own drawings where they were, the
moved one offset, and Snowbound's beside them.

`tools/test_ink_tools.py` checks all three without a VM. Regenerate the candidates with
the export variables set to new absolute directories, then cold-open each with
`tools/native_runner.py OUTPUT COLD --expected-pages N --collect-notebook --screenshots`.
