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
