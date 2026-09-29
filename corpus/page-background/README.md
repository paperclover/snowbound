# Page background

`candidate` is `backgrounds_go_on_and_off_pages_that_have_content` in
`crates/snowbound/src/manage.rs` (`SNOWBOUND_BACKGROUND_EXPORT`), on a copy of
`rule-lines/native/notebook`: the Page Color menu's ops give the ruled "Standard" page
Ivy's art (`manage::art_ops`), give "Wide" the art and take it away again, and colour
"SmallGrid" `0x00c8d8e8`, a COLORREF outside OneNote's menu.

`cold` is its fresh OneNote 2010 read: "Standard" keeps its rule lines under one
background image (`backgroundImage="true"`), "Wide" keeps its lines and no image, and
"SmallGrid" reads back `color="#E8D8C8"` and shows it under its grid. `read/page-006.png`
shows OneNote drawing the art over the rule lines. `tools/test_page_background.py`
checks this without a VM.
