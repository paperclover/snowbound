# Outlook task tags through the editor

`native/` is OneNote 2010 authoring `tools/native/pages.ps1` input (two identical
pages in `Tasks.one`: three paragraphs carrying disabled Outlook tasks, the third
also a To Do tag, and an untagged one). OneNote stores each task as a note tag
without a definition, with ActionItemType 100, NoteTagShape 89 and
NoteTagPropertyStatus 0.

`candidate/` is the output of `task_tags_survive_tag_removal_and_page_delete_undo`
in `crates/canvas/tests/task_tags.rs`: on "Remove a task" the editor's Remove Tag
clears the first paragraph's task and toggling To Do off the third keeps its
task; "Delete and undo" has all its content deleted and the deletion undone,
each published as its own revision.

`cold/` is a fresh OneNote 2010 read: the first paragraph has no task, the other
tasks keep their identities, dates and disabled state, and the restored page
reads as OneNote authored it. `tools/test_task_tags.py` checks this without a
VM. Regenerate with `SNOWBOUND_TASK_TAGS_EXPORT` set to a new absolute directory
while running the test, then cold-open it with
`tools/native_runner.py OUTPUT COLD --expected-pages 2 --collect-notebook`.
