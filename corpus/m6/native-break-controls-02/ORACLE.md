# Native paragraph-break controls

OneNote 2010 14.0.4763.1000 authored the 24 cases in `notebook/fixture.json` through the captured `scripts/author.ps1`; `read/` contains native XML and PDF exports. The clone was deleted (`teardown.json`).

Twelve inputs appear both as styled plain text and as hyperlink labels. Literal line endings become stored carriage returns; a `<br>` followed by a newline also survives. Bare `<br>` elements without a following newline were discarded by this COM import. Native HTML export omits exactly one final carriage return while preserving preceding breaks. Hyperlink fields exclude the breaks. `comparison.json` records each stored/native observation.

The Rust test asserts the stored text and associated link boundaries. The independent Python oracle checks that removing another break causes a failure. This is evidence for the observed COM import/export behavior, not permission to trim arbitrary whitespace.
