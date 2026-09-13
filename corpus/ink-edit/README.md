# Rust ink authoring

`drawing/candidate` is the page writer's output for
`an_ink_drawing_is_written_erased_stroke_by_stroke_and_removed` in
`crates/onestore/tests/page_ink.rs`: on a section created in Rust, a page-level
ink object with two strokes, a 60 pt square at (300, 120) and a red diagonal,
stored the way OneNote 2010 stores a mouse drawing
(`corpus/native-ink/cold-ui-ink`): an ink container listed as a page child, its
data node listing stroke objects, each stroke's coordinates as ISF multi-byte
first differences in HIMETRIC with its index, language, identity, time and
half-inch origin, and one drawing-attribute object per distinct pen carrying
the dimension table OneNote wrote. `drawing/cold` is a fresh OneNote 2010
read: the `InkDrawing` reports the stroke extent as its position and size (one
HIMETRIC unit larger, as OneNote does) and `page-000.png` shows the square and
diagonal rendered.

`handwriting/candidate` is the same drawing held by a new paragraph of the body
outline (`handwriting_is_written_as_paragraph_content`); `handwriting/cold`
shows OneNote accepting it as an `InkDrawing` inside the outline element, the
outline grown to contain the strokes' absolute page coordinates.

`tools/test_ink_edit.py` checks both without a VM. Regenerate with
`ONESTORE_INK_EXPORT` and `ONESTORE_INK_PARAGRAPH_EXPORT` set to new absolute
directories while running the tests, then cold-open each with
`tools/native_runner.py OUTPUT COLD --expected-pages 1 --collect-notebook
--screenshots`.

`native-mouse-over-text/` is OneNote 2010 receiving mouse strokes over an
outline's text and in empty space (`tools/native_handwriting.py`): both
became page-level `InkDrawing` objects. Mouse ink never produces
handwriting paragraphs or embedded ink in text runs, so those forms stay
read-only retained content until a pen-authored fixture exists.
