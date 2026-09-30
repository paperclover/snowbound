# Typing beside a picture or file alone in its outline

`native/` is OneNote 2010 authoring `tools/native/pages.ps1` input: two pages in
`Alone.one`, one an outline holding only a text file, the other only a picture.

`keys/` is `tools/native_object_outline.py` clicking beside each object inside its
outline in OneNote 2010 and typing "abc": OneNote adds a paragraph after the object
holding the text, and undoing removes that paragraph again.

`candidate/` is the output of `typing_beside_an_outlines_only_object_adds_a_paragraph`
in `crates/canvas/tests/object_outlines.rs`: the editor shows each outline with a
paragraph after its object that is stored only once typed in, and the typing, its undo
and its redo are published as revisions; `cold/` is a fresh OneNote 2010 read of it,
whose pages read as `keys/` does. `tools/test_object_outline.py` checks this without a
VM. Regenerate with `SNOWBOUND_OBJECT_OUTLINE_EXPORT` set to a new absolute directory
while running the test, then cold-open it with
`tools/native_runner.py OUTPUT COLD --expected-pages 2 --collect-notebook --screenshots`.
