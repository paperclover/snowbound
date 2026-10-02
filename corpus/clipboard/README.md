# Clipboard

`onenote-2010.html` is the `HTML Format` OneNote 2010 put on the clipboard for a
whole outline holding bold, italic, coloured, resized, re-fonted, underlined and
highlighted runs, a link, nested bullets, a numbered list, a To Do tag, a 2 by 2
table with a bold cell, and a last line (lab, 2026-10-02; `tools/w7`, a page
built through COM and copied with Ctrl+A, Ctrl+C). Beside it OneNote offered
`OneNote 2010 Internal` (56 bytes naming the copy, not its content), Unicode and
ANSI text, an enhanced metafile and a device-independent bitmap, but no RTF. The
tag is absent from the HTML. `crates/canvas/src/editor/html.rs` reads it in its
tests.

`word-2010.html` is the `HTML Format` Word 2010 put on the clipboard for a
document of a bulleted list three levels deep, a plain paragraph, a numbered
list three levels deep (1., a., i.) and a last plain paragraph, typed with
AutoFormat and Tab and copied with Ctrl+A, Ctrl+C (lab, 2026-10-02). Word
writes each item as a paragraph whose `mso-list` style names its list and
level, with its marker in a `mso-list:Ignore` span inside `<![if
!supportLists]>`. Pasted into OneNote 2010, each level nested under the item
above; bullets kept Word's glyph and font (Symbol `·`, Courier New `o`,
Wingdings `§`) with no gallery index, and numbers took Word's sequence and
punctuation. OneNote also nested the numbered list under the plain paragraph
before it, from Word's indents; Snowbound nests by `mso-list` level alone, as
it reads `ul` and `ol`. `crates/canvas/src/editor/html.rs` and
`crates/canvas/tests/clip_paste.rs` read it.
