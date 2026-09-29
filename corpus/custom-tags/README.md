# Customized note tags

`candidate/` is the editor's output for `custom_tags_store_what_customize_tags_stores`
in `crates/canvas/tests/custom_tags.rs`: on a section created in Rust, four tags of a
customized list are applied through `Formatting::Tag`, each definition storing the tag's
place in the list as its action type: "Snow check" (Green Star, 61, dark red text on Sky
Blue), "Snow task" (Green Check Box 1, 48, once open and once checked), "Snow circle"
(Blue Circle 2, 31) and "Snow ink" (no symbol, blue text). Definitions carry the
NoteTagPropertyStatus bits OneNote's Customize Tags writes: hasLabel, hasFontColor and
hasHighlightColor where set, hasIcon for a symbol.

`cold/` is a fresh OneNote 2010 read: the four `TagDef` entries keep their type, symbol,
name and colours, and the tags their completion state. `tools/test_custom_tags.py`
checks this without a VM. Regenerate with `ONESTORE_CUSTOM_TAGS_EXPORT` set to a new
absolute directory while running the test, then cold-open it with
`tools/native_runner.py OUTPUT COLD --expected-pages 1 --collect-notebook --screenshots`.

`native/shapes.one` is OneNote 2010's own work in a lab clone:

- "Shapes 1-72" and "Shapes 73-143" tag a paragraph with every NoteTagShape, set through
  COM (its definitions store NoteTagPropertyStatus 0).
- "Custom tags" holds a tag made with New Tag (Snow check, as above), applied at the top
  of the list (action type 0), then after moving it below To Do (a second definition,
  action type 1), with To Do applied from both places. The UI stores status 15 and 9.
- "Cells" tags a paragraph `cR-C` with the symbol picked from row R, column C of Modify
  Tag's symbol gallery, which `crates/snowbound/src/tags.rs` lays out.

OneNote keeps the tag list in `%APPDATA%\Microsoft\OneNote\14.0\Preferences.dat`, written
when it exits; Snowbound keeps its own in its settings.
