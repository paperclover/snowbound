# Cross-run text replacements

Six edits of the public formatted-insertion notebooks, independently opened by
OneNote 2010 in disposable cold-cache machines. Cases cover partial and complete
replacement, a native table cell, deletion, typing after deletion, and insertion
at a style boundary. Each directory is a separate notebook because the source
section identities can be shared.

The inserted characters inherit the style at the start of the replaced range;
surviving characters retain their styles. Empty final runs retain their typing
style. These are library edit semantics; the native captures check that OneNote
reads the resulting content and formatting correctly.

Generate candidates with the ignored `export_native_cross_run_edits` test and
`ONESTORE_CROSS_RUN_OUTPUT` set to a new absolute directory. Capture each candidate
with `tools/native_runner.py --expected-pages 1 --collect-notebook` and compare
with `tools/verify-document.py`. The manifest records source paths, target IDs,
UTF-16 ranges and file hashes. The attachment export links to its canonical copy.
`tools/test_cross_run_edit.py` checks the retained
native oracles without starting a VM.
