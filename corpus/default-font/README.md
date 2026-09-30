# Default font

OneNote 2010 keeps Options > General > Default font (Font, Size, Font color) in
`HKCU\Software\Microsoft\Office\14.0\OneNote\Options\Editing` as `DefaultFontFace`,
`DefaultFontSize` and `DefaultFontColor` (a COLORREF DWORD; its
picker is Office's theme and standard colours, `options-font-color.png`). Observed in a lab clone on 2026-09-30, it never writes run
formatting for it: a new page's `PageTitle` quick style takes the face at 17 pt
whatever the size, and new text takes a `p` quick style in the face and size
(plain, in the font colour, no highlight or paragraph spacing). On a page whose `p` is in
another font, new text gets a second `p` beside it, the old text keeping its own.
`native/Lab.one` is that section: "Title text" (Georgia 14), "T8" (Arial 8),
"C14B14" (Calibri 14), then "Later" typed on "Title text" with Calibri 14, its
second `p`. `native/Color.one` is a page made after picking Blue: `PageTitle` and `p`
both take `#0070C0`, runs no colour of their own (`color.png`).

`candidate/` is `crates/canvas/tests/default_font.rs`'s section, exported with
`CANVAS_DEFAULT_FONT_EXPORT`: a page created titled in Georgia and Blue, and two outlines
the editor started with the Default font Georgia 14 in Blue, each its own appended
revision. `cold/page.xml` and `cold/page.png` are a fresh OneNote 2010's read:
`PageTitle` Georgia 17 and one `p` Georgia 14, both `#0070C0`, as OneNote writes them.
`tools/test_default_font.py` checks it without a VM.
