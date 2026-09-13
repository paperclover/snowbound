# Editing next to a recording

`candidate/Features.one` is the writer's output for
`editing_a_recorded_page_keeps_the_recording_and_its_annotation` in
`crates/onestore/tests/page_media.rs`: the native section of
`corpus/m6/native-features-01` with one plain paragraph appended to the page
"Files and recording", where OneNote 2010 recorded `silence.wav` and
annotated a paragraph with the moment 500 ms. The recording attachment
(`page::Attachment::recording`) and the annotation
(`PageParagraph::media`) are read from the page and kept through the edit;
a model that adds an annotation or drops the recording refuses to write.

`cold/` is a fresh OneNote 2010 read: `MediaFile`, `MediaReference` and
`MediaIndex` name the same recording and moment as before the edit, next to
the added paragraph. `tools/test_media_edit.py` checks this without a VM.
Regenerate with `ONESTORE_MEDIA_EXPORT` set to a new directory while running
the test, then cold-open with `--expected-pages 13 --collect-notebook`.
