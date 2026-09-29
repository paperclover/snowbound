# Rule lines

`native` is OneNote 2010's View, Rule Lines, set through its menu on the VM on eleven
pages of a notebook the COM API made, each titled for what it was given: None; Narrow
(ruled, then set back to None); College, Standard and Wide Ruled; Small, Medium, Large and
Very Large Grid; ColorRed (Standard, then Rule Line Color, Red); Hidden (Standard, then
Rule Line Color, `<none>`, on a Teal page). MediumGrid was then given Rule Line Color,
Purple. `read` is its cold read in a fresh clone.

The page node stores six properties MS-ONE does not document, in this order: the
horizontal lines' colour (`0x14001cd5`), kind (`0x14001cd3`: 2 ruled, 1 grid) and spacing
(`0x14001cd4`, half inches), then the vertical lines' kind (`0x14001cd6`: 3 a margin line,
1 grid), spacing (`0x14001cd7`, 0 for a margin line) and colour (`0x14001cd8`). Colours
are 0xAARRGGBB with alpha 0xFF, unlike the page colour's COLORREF; the menu's `<none>`
colour is white. None removes all six. OneNote draws lines 1/96 inch wide: horizontal ones
every spacing from the margin origin (36, 14.4) down across the page, grid lines through
it either way down the whole page, and a margin line 1/96 inch left of it.
`colors.txt` is each Rule Line Color as the page XML reports it, in the menu's order.

`candidate` is `rule_lines_store_what_onenote_stores` in `crates/onestore/src/op/tests.rs`
(`ONESTORE_RULE_LINES_EXPORT`): on OneNote's file, the `RuleLines` op gives None Wide
Ruled, removes College's, turns Standard into a Small Grid and MediumGrid into Narrow
Ruled in Red, and gives a created page, "Created", a Very Large Grid. `cold` is its fresh
OneNote 2010 read: every page reads back with the lines written, and `read/page-*.png`
shows them. `tools/test_rule_lines.py` checks both without a VM.
