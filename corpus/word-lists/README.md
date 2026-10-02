# Lists pasted from Word

`candidate/` is `crates/canvas/tests/clip_paste.rs`'s `words_lists_paste_and_store`
section: Word 2010's copy of three-level bulleted and numbered lists
(`corpus/clipboard/word-2010.html`) pasted through the editor at a caret on blank page,
stored as one edit. Bullets keep Word's glyph and font with no gallery index
(`ListMSAAIndex`), as OneNote 2010 stored the same paste (lab, 2026-10-02).
`cold/` is a fresh OneNote 2010 read: the bullets read back as the ones OneNote's own
paste gave (`bullet` 1, 0 and 13), and the numbers as 1., a., i. and 2.
`tools/test_word_lists.py` checks the capture without a VM. Regenerate with
`CANVAS_WORD_LIST_EXPORT` set to a directory while running the test, then
`tools/native_runner.py DIR COLD --expected-pages 1 --collect-notebook --screenshots`.
