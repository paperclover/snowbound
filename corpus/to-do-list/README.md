# Bullets into a to-do list

`native/` is OneNote 2010 on a copy of `corpus/list-edit/cold/notebook`: the
outline's paragraphs selected with Ctrl+A twice (`1-select.ahk`), then Ctrl+1
(`3-tag.ahk`). OneNote keeps every bullet and number and puts the To Do box
beside them (`3-tag.png`); the selected top-level paragraphs take the tag and
their nested children do not. Its context menu on the selection offers no
command that converts a list (`2-menu.png`).

`candidate/` is the output of `lists_become_to_do_lists_and_back_one_revision_each`
in `crates/canvas/tests/to_do_list.rs`, on the same section: Make To-Do List over
"Bullet item" through "Nested numbered" removes their bullets and numbers and
tags each To Do, then Make Bulleted List over "Second numbered" and "Nested
numbered" removes the tag and gives each OneNote's default bullet, each published
as its own revision.

`cold/` is a fresh OneNote 2010 read: the first two paragraphs are open To Do
items with no list, the next two are bulleted and untagged, and "Restarted at
three" keeps its number. `tools/test_to_do_list.py` checks both reads without a
VM. Regenerate with `SNOWBOUND_TO_DO_LIST_EXPORT` set to a new absolute directory
while running the test, then cold-open it with
`tools/native_runner.py OUTPUT COLD --expected-pages 1 --collect-notebook --screenshots`.
