# Rust note tag authoring

`candidate/` is the page writer's output for
`new_definitions_and_tags_publish_on_a_fresh_page` in
`crates/onestore/tests/page_tags.rs`: on a section created in Rust, two tag
definitions are added ("Rust task", type 0, symbol 3; "Important", type 1,
symbol 13, red text on a yellow highlight), the first paragraph gains an open task, a second paragraph a
completed task with creation and completion dates, a third paragraph both
tags, and a fourth paragraph none. Tags are property-set arrays on the text
object referencing definition objects (jcid 0x120043) that carry the model's
identities after squash; an element holds one tag per action type, as OneNote
requires.

`cold/` is a fresh OneNote 2010 read: both `TagDef` entries appear with their
type, symbol, name and colours (`fontColor="#FF0000"`,
`highlightColor="#FFFF00"` for the second), the tags read back with their completion state and
dates, and the untagged paragraph has none. `tools/test_tag_edit.py` checks
this without a VM. Regenerate with `ONESTORE_TAG_EXPORT` set to a new absolute
directory while running the test, then cold-open it with
`tools/native_runner.py OUTPUT COLD --expected-pages 1 --collect-notebook`.
