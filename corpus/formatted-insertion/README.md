# Atomic formatted insertions

Synthetic Rust paragraph/outline insertions with independent OneNote 2010 cold
reads. The native paragraph and table inputs descend from the public
`native/20260905-05` corpus. `manifest.json` records stable intents and file hashes;
each capture retains its native environment and run provenance. Attachment exports
link to existing identical public payloads.

Generate new candidates with the ignored `export_native_formatted_insertions` test
and `ONESTORE_INSERT_OUTPUT` set to a new absolute directory. Each child directory
is a separate notebook: the candidates can share source section identities.
Use `tools/native_runner.py` with one child directory and `--expected-pages 1`.
`tools/native/formatted-insertion.ps1` performs an unchanged native HTML round trip
followed by a native prefix edit for the four nonempty cases.

The unchanged round trip establishes native font substitution before checking the
edit's exact character styles. Table updates omit the outline's measured `Size`:
resubmitting it made two unlocked columns oscillate in native XML, including after
a fresh cache reopen. Those unsettled captures are excluded. The retained table
reverse capture omits that measurement; paragraph and ordinary outline captures
retain it.

`native-cell/ui` records a direct keyboard prefix edit, without HTML resubmission.
Its native XML settles, preserves every prior character's style, and the resulting
file passes the same model oracle. The capture includes the keyboard script and
screenshot.

COM omits the empty outline. Its cold-reopened file retains the empty rich-text
object and style; `empty/typed-after-cold` adds text to that retained object and
verifies the 18-point italic result in another independent native cold read.
`tools/test_formatted_insertion.py` checks these captures without a live VM.
