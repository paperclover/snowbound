# Native automatic-title fixtures

OneNote 2010 authored these synthetic sections in disposable, isolated Windows
profiles. `provenance.json` records their capture paths and hashes.

- `rtl-and-attachments.one`: RTL outline order, RTL table cells, and attachments
  preceding text. RTL outlines choose descending x position; RTL rows use the
  last stored cell first. Attachments do not supply automatic text titles.
- `widths-and-limits.one`: differing outline widths establish that ordering uses
  the x anchor, not the right edge. Automatic titles trim the truncated result
  and count UTF-16 units without requiring grapheme boundaries.

`tests/edit.rs` edits each relevant body candidate and checks the native-derived
navigation label while preserving unrelated properties and historical objects.
