# Attaching files from the editor

`candidate/` is `crates/canvas/tests/attachment_insert.rs`'s section: the editor
attaches `notes 🦀.txt` (30 bytes, with the icon OneNote stored for a text file)
at the end of "Before the file", "After the file" is typed where the caret
lands, and `large.bin` (2 MiB of noise, with the canvas's blank-page icon, as
a host without system icons stores it) is attached after that. Each step is its
own edit of the ops the editor recorded, sealed into one appended revision.
`cold/` is a fresh OneNote 2010 read: both files in place, and the bytes OneNote
materializes to open them (`read/*.attachment`) hash as written. Opened by
double click and saved by Save As in a lab clone, both matched too.
`tools/test_attachment_insert.py` checks the capture without a VM. Regenerate
with `CANVAS_ATTACHMENT_EXPORT` set to a directory while running the test, then
`tools/native_runner.py DIR COLD --expected-pages 1 --collect-notebook
--screenshots`.

`without-icon.png` is OneNote opening a file stored without the icon: it draws
a broken picture, so the editor always stores one.

## What OneNote 2010 does

Observed in a lab clone on 2026-09-29:

- **Storage.** Every attachment goes into the section's own file data store,
  whatever its size (1 KiB to 300 MiB, through COM and through Insert > Attach
  File, on a local folder and on a share). OneNote never wrote a `_onefiles`
  folder. Identical bytes are stored once per section: a second attachment of
  the same file, or the same icon, references the first payload. Snowbound
  stores each attachment's bytes anew, which OneNote reads the same.
- **Icon.** Beside each file it stores a 32-pixel PNG of the file type's
  system icon, drawn at 24 points, over the name without its extension.
- **Attach File and drops.** At the caret, the paragraph splits there: the file
  follows the text before the caret (or replaces an empty first half) and the
  text after it follows the file with the caret, as an empty paragraph at the
  end of one. A drop on text does the same at the drop point, after asking
  whether to attach a copy (the default) or link to the original. Attached or
  dropped on blank page, the file becomes a page-level object at that point.
- **Open.** A double click or Open warns once ("Opening attachments could harm
  your computer and data") and opens a copy from
  `%TEMP%\OneNote\14.0\NT\N\` under the file's own name. The context menu
  offers Open, Save As (in the source's folder, under its name), Insert as
  Printout, Open Original and Copy Link to Original.
