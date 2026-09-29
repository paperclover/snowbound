# Page background

`candidate` is `backgrounds_go_on_and_off_pages_that_have_content` in
`crates/snowbound/src/manage.rs` (`SNOWBOUND_BACKGROUND_EXPORT`), on a copy of
`rule-lines/native/notebook`: the Page Color menu's steps through the editor
(`CanvasEditor::set_paper`), each stored as one edit. "Standard" takes Ivy's art; "Wide"
takes it and has it taken away; "College" takes it and undoes; "MediumGrid" takes it,
undoes and redoes; "LargeGrid" takes Ivy, then Tulips in its place, and undoes; "SmallGrid"
is coloured `0x00c8d8e8`, a COLORREF outside OneNote's menu; "VeryLargeGrid" is coloured
Lemon and takes Sparks' art.

`cold` is its fresh OneNote 2010 read: every ruled page keeps its lines; "Standard",
"MediumGrid" and "LargeGrid" show one Ivy background image (`backgroundImage="true"`),
"Wide" and "College" none; "SmallGrid" reads back `color="#E8D8C8"` under its grid, and
"VeryLargeGrid" `#FDFDDD` under Sparks. `read/page-003.png` shows OneNote's opaque art
hiding the rule lines, and `read/page-005.png` its white hiding the Lemon page colour.
`tools/test_page_background.py` checks this without a VM.
