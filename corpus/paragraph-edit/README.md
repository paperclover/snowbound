# Native paragraph editing

OneNote 2010 authored 13 cases, split each through keyboard input, reopened the
result in a fresh disposable VM, and joined each pair through keyboard input.
The four phases retain notebook bytes and independent native XML: `before`,
`split`, `cold-split`, and `joined`. A fourteenth control page remains unchanged.

Cases cover mixed formatting, style/start/end/empty boundaries, nested children,
bullets, numbering, completed tags, table cells, soft breaks, and splitting plain
text before a hyperlink. The manifest records storage identities and split
positions. Captured UI scripts and screenshots retain the actual native actions;
`joined/first-backspace` records the intermediate list/indentation changes.

The original paragraph stays on the left and children transfer to the new right
paragraph. Completed tags remain on the left rich-text object. Joining an empty
left paragraph retains its paragraph identity but adopts the right text object.
Bullet, numbered, and nested cases require two Backspaces because the first
changes list or indentation state. The native application also added flag
`0x880034dd` and changed timestamps on some unrelated text objects; the manifest
records those observations without assigning undocumented meaning to the flag.

`tools/native/paragraphs.ps1` authors the cases. Use `tools/native_runner.py` with
that author, `--expected-pages 14 --inspect --collect-notebook`, then navigate to
each current page/object ID and send its recorded keys. Preserve the authored
notebook before completing inspection. Cold-open the split notebook for the join
pass. Captured COM IDs belong to their originating sessions; discover fresh IDs
when regenerating the corpus.

`rust-split` retains twelve `ParagraphSplit` outputs, their cold native captures,
and a second native session typing into four empty boundary paragraphs. The
hyperlink case stays unchanged because this operation rejects fields. The Rust
test `export_native_paragraph_splits` generates candidate notebooks; its output
directory comes from `ONESTORE_PARAGRAPH_OUTPUT`. Native character/style comparisons
use both the keyboard-generated controls and the Rust-written notebooks. Empty
typing checks extend the preexisting empty run in an independent expected model.

`tools/test_paragraph_edit.py` verifies these controls without a VM. The native
join captures establish behavior for subsequent join implementation.
Identical captured files link to one canonical copy within this corpus.

`join-edges`, authored with `tools/native/paragraph-joins.ps1`, adds five native
Backspace cases for differing inherited styles, tags on either side and children
on both sides. A nonempty left paragraph keeps its own tags; right-only tags
disappear from the active result. An empty tagged left paragraph transfers its
tags onto the adopted right text. A left paragraph with children joins through
its last descendant, and the right children become that descendant's peers.
The untouched parent's text gains a timestamp and `0x880034dd`, as in the earlier
native controls; its content and other properties stay unchanged. The test fixes
these observed graph, identity and metadata effects alongside independent native
character-style comparisons.
