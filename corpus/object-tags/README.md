# Tags on pictures and files

`native/` is a notebook OneNote 2010 made in the lab: `tools/native/picture-objects.ps1`
put a picture and a file in an outline, and a picture, a file and a picture of text on the
page; each picture and file was then selected and tagged To Do with Ctrl+1 by hand.
OneNote stores each tag on the picture or file itself (NoteTagStates on jcidImageNode and
jcidEmbeddedFileNode), in an outline as on the page, though its COM read shows an outline
object's tag on its paragraph. It draws the check box in a column left of the object,
centred on it: 20.25 points out from an outline's picture, as from text, and 24.75 points
from one on the page. It recognised "GLACIER HARBOR / Invoice 4471" in the picture of text
and stored it as the picture's RichEditTextUnicode, which its search finds (the read's
`OCRText`). A Ctrl+K on a selected picture there opened no dialog; the address typed
became a link to a new page, which the section keeps.

`candidate/` is the output of `tags_on_pictures_and_files_draw_and_check` in
`crates/canvas/tests/pictures.rs`: the page picture's, the page file's and the outline
picture's check boxes clicked, one revision each. `cold/` is a fresh OneNote 2010 read of
it: those three checked, the outline file's not, and the recognised text kept.
`tools/test_object_tags.py` checks this without a VM. Regenerate with
`CANVAS_OBJECT_TAGS_EXPORT` set to a new absolute directory while running the test, then
cold-open it with `tools/native_runner.py OUTPUT COLD --expected-pages 4 --collect-notebook --screenshots`.

`restored/` is the output of `a_restored_picture_keeps_its_recognised_text`: the picture of
text deleted and brought back by undo, written with its text (RichEditTextUnicode), its
language and the undocumented layout of its words (0x1C001D61) as read, and its cold read,
whose OCRData equals the native read's, words and positions alike
(`CANVAS_RECOGNIZED_EXPORT`).
