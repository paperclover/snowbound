# OneNote 2010 structural-edit probe

Native captures from the Windows 7 lab (VM `enterprobe`, 2026-09-25), driven by
AutoHotkey through Enter, Backspace, Delete, Tab, Shift+Tab and paste on
synthetic pages. Every page starts with a plain paragraph `Above`.

- `probe.one`: the whole probe section after the run, closed by OneNote; one
  page per case (`c1-bullet`, `c8-split`, ...).
- `tags.one`: a section holding one page with the nine Ctrl+1..9 default tags,
  closed and flushed by OneNote.
- `tag-gallery.one`: one page with each of the 29 default tags of the Home tab's
  Tags gallery applied from the gallery in order, one per paragraph (VM `tbar`,
  2026-09-28); two paragraphs also carry Call back. Copied while OneNote was open.
- `summaries/<case>-<step>.txt`: the page after each step, from COM
  `GetPageContent`, one line per paragraph:
  `L<level>[+indentN] [tags; [x]=completed] [list] qsN text`.

Case families: `c1` Enter at the end of `Target text`, `c2`/`c2s` Enter in the
middle and at the start, `c3` Enter on an empty tagged or bulleted paragraph,
`c4`/`c4m` Backspace at the start of a second line, `c5b` Delete at the end of
the first of a pair, `c6` Tab and Shift+Tab, `c7` a two-line paste, `c8` a
numbered list, `c9` character formatting, headings and multiple tags, `c10`
paragraphs with children, `pb`/`pbe` bullets applied to plain paragraphs.

`crates/canvas/src/editor/evidence.rs` replays the summaries against the
editor; `crates/canvas/tests/structural_roundtrip.rs` and
`crates/onestore/src/op/tests.rs` edit `probe.one`.
