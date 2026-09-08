# Native outline editing controls

OneNote 2010 authored fourteen cases plus an unchanged source page. `before`,
`after`, and `cold` retain notebook bytes and independent native XML. The final
phase uses a fresh application cache. Cases preserve Unicode, hyperlinks, tags,
nested descendants, siblings, and a separate outline.

The controls cover leaf/subtree reordering, subtree indentation/outdentation,
stored collapse/expansion, deletion of a leaf/subtree/sole paragraph/outline,
outline position, fixed width, and automatic width. Keyboard subtree operations
explicitly select descendants. Deleting the sole paragraph removes its empty
outline. The native application sometimes advances unchanged text timestamps
and adds `0x880034dd`; the test permits only those observed bookkeeping changes.

COM outline replacement reallocates contained rich-text identities. Keyboard
movement retains them. Native XML can reorder outlines by their positions while
the file's page child order remains unchanged. Explicit width sets its native
user-size flag; automatic width clears that flag. OneNote regenerates height
from content instead of retaining the requested height. These observations
constrain future typed operations; this corpus is not itself a writer feature.

Persisted collapse values survive a cold open. Native keyboard collapse/expand
can instead change only the client's cached view, so warm COM XML alone does
not prove a stored-property edit. The controls here explicitly change the
persisted default through COM.

To regenerate, run `tools/native_runner.py` on
`corpus/formatted-insertion/native-paragraph/candidate` with a new output directory,
`--author tools/native/outline-editing.ps1 --expected-pages 15 --inspect
--collect-notebook`. After inspection is ready, run
`tools/native_outline_editing.py OUTPUT`. It freezes the before-image, records
the actual keyboard/XML inputs, and requests final capture; the runner removes
its clone. Move the returned `before-read` directory into `before/read`.
Cold-open `OUTPUT/notebook` into another new output directory with the runner,
`--expected-pages 15 --collect-notebook`. Public verification lives in
`tools/test_outline_edit.py`.

The controller selects the target again after activating the native window and
transfers XML as UTF-8 data. Captured authoring scripts retain their exact source;
identical immutable artifacts link to one canonical corpus copy.
