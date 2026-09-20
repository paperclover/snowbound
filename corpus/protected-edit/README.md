# Protected page edit

`candidate/notebook` is `native-encrypted/encrypted-01` after
`Notebook::save_unlocked` appended text to the first paragraph and added a
paragraph, exported by `an_unlocked_page_is_saved_under_the_section_key`
(`ONESTORE_PROTECTED_EXPORT`). `candidate/read` is OneNote 2010's COM read from a
fresh clone with an empty cache after unlocking the section in its UI with the
fixture password (`../native-encrypted/manifest.json`).

`native-after/synthetic.one` is the same section after OneNote then typed
" Native edit after Rust." into the positioned outline and saved. Its revisions
that depend on an earlier revision carry no key node (`0x7c`); only revisions
without a dependency name the key.
