# Table growth

`candidate/notebook` is `native/20260905-05` after `a_text_edit_stores_the_table_entries_it_names`
(`ONESTORE_TABLE_EXPORT`) appended words to a paragraph of every page. Each rewritten
object group stores only the global id table entries its objects name, so the stored
indices have gaps where the source revision's table named GUIDs the edit does not use.
`cold` is OneNote 2010's cold read of it from a fresh clone; OneNote left the file as
written.
