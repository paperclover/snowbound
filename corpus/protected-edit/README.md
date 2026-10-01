# Protected page edit

`candidate/notebook` is `native-encrypted/encrypted-01` after
a Rust edit of the unlocked page appended text to the first paragraph and added a
paragraph, exported by the protected writer that `Section::unlock` has since replaced
(`corpus/protected-sections` gates the current one). `candidate/read` is OneNote 2010's COM read from a
fresh clone with an empty cache after unlocking the section in its UI with the
fixture password (`../native-encrypted/manifest.json`).

`native-after/synthetic.one` is the same section after OneNote then typed
" Native edit after Rust." into the positioned outline and saved. Its revisions
that depend on an earlier revision carry no key node (`0x7c`); only revisions
without a dependency name the key.
