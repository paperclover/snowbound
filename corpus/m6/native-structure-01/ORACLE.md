# Native document structure controls

OneNote 14.0.4763.1000 authored the second page from the frozen
[scripts/author.ps1](scripts/author.ps1). The fixture includes named paragraph
styles, alignment and spacing, two associated tags, nested bullets, Roman
numbering restarted at III, and a 3-by-2 table with one locked column.

The locked column remains 120 points wide. The unlocked column contracts from
the authored 180 points to approximately 37.116 points to fit its contents.
The native PDF shows both the text formatting and table on separate sheets.
[Tests](../../../tests/document.rs) assert the model's corresponding semantics.
The [partial black highlight XML omission](../native-probes-01/ORACLE.md) also
occurs here; the PDF preserves the highlight.

The [table controls](../native-table-controls-01/expected.json) isolate a native
XML limitation: adding `shadingColor` to a cell causes `UpdatePageContent` to
return `0x80042001`. Removing that attribute succeeds with locked or unlocked
columns. This does not establish whether OneNote 2010 renders the storage-level
CellShadingColor property.

## TOC transaction fragment boundary

The Rust-created TOC began with a 44-byte transaction-log fragment containing
three list-count entries, a sentinel, and a 12-byte next-fragment reference.
OneNote appended another fragment. Its next CRC is `0x68f09d29`; including the
preceding fragment's final sentinel incorrectly produces `0x548e8f73`.

MS-ONESTORE 2.3.3.2 product behavior note 8 specifies exclusion of bytes flushed
at the end of a TOC transaction-log fragment. This fixture establishes that the
final entry is excluded from the running CRC, while its own checksum is still
validated. The storage regression also corrupts the native checksum and verifies
that the damage is detected. Writer regressions cross fragment and transaction
counter boundaries through 260 successive edits.
