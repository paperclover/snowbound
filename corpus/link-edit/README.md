# Rust hyperlink authoring

`candidate/` is the page writer's output for
`a_link_is_added_to_a_fresh_page_and_reads_back` in
`crates/onestore/tests/page_links.rs`: on a section created in Rust, the
paragraph "Read about Rust" gains a hyperlink the way OneNote stores one, a
hidden field-code run `U+FDDF HYPERLINK "https://example.invalid/rust"` and
the visible label "the Rust site", both flagged as hyperlink runs. The text
and formatting writers treat such runs as ordinary text with flags; equations,
embedded objects and runs with associated data stay refused.

`cold/` is a fresh OneNote 2010 read: the paragraph text is
`Read about Rust <a href="https://example.invalid/rust">the Rust site</a>`.
`tools/test_link_edit.py` checks this without a VM. Regenerate with
`ONESTORE_LINK_EXPORT` set to a new absolute directory while running the test,
then cold-open it with `tools/native_runner.py OUTPUT COLD --expected-pages 1
--collect-notebook`.

## Internal links

`native-links/` is OneNote 2010 adding links to a page, to a paragraph on it
and to the section on the Rust-authored page above through the COM API
(`tools/native/page-link.ps1`; `links.json` holds the `onenote:///…` URLs
`GetHyperlinkToObject` returned, `update.xml` the submitted page). In
`notebook/`, OneNote stored each as a `HYPERLINK` field code with the
relative form `onenote:#Link%20target&section-id={section file identity}
&page-id={page notebook-management identity}&end&base-path=<section path>`;
a paragraph link ends with `&object-id={paragraph identity}&n` instead of
`&end`, and a section link has neither title nor page. OneNote rewrote the
target outline after linking, so its own paragraph link names an identity
(`n` 28) the current outline no longer holds.

`internal/candidate` is the writer's output for
`a_page_links_to_another_page_and_its_paragraph` in
`crates/onestore/tests/page_links.rs`: a section created in Rust with a
second page, whose first page links to that page and to its first paragraph
with URLs built by `onestore::page::link::internal_link`. `internal/cold` is
its cold read with both links. Regenerate with
`ONESTORE_INTERNAL_LINK_EXPORT` and cold-open with `--expected-pages 2`.

## Typed links

`native-typed/` is OneNote 2010 typing on the Rust-authored page above, driven by
`tools/native_links.py` (`links.ahk` the AutoHotkey session, `links.png` the
desktop afterwards). Each URL typed and ended by a space or Enter became a link
of its own text, a hyperlink run with no field code and no label flag: `http`,
`https`, `ftp`, `file`, `mailto`, `news` and `onenote` schemes, `www.` and
`\\server\share` paths. Trailing punctuation and an unmatched closing bracket
stay outside; `me@example.com` and `example.com` stay text. The Link dialog
(Ctrl+K) on the word at the caret, on a selection and on nothing stored the
address as typed (`example.net/x`, which the COM read shows as
`http://example.net/x`) in a hidden field code before a label flagged as one,
the address itself when the text was empty; text typed after a label is plain.
Remove Link (Shift+F10, R) left the URL as plain text, its runs' link flag
false. An interactive session on a copy showed the rest: a click on a link opens
it at once, typing straight after a link of its own text extends it and its
address, and Copy Link to Page puts
`onenote:///<section path>#<title>&section-id={…}&page-id={…}&end` on the
clipboard (`&object-id={…}&n` in place of `&end` for Copy Link to Paragraph),
which pastes as a labelled link. `crates/canvas/src/editor/link.rs` replays
the session through the editor and compares every paragraph's link runs.

## Links and equations from the editor

`editor/candidate` is the canvas editor's output for
`crates/canvas/tests/links_equations.rs` on the internal-link section above:
URLs typed and ended by a space or Enter, the Link dialog on a selection, on the
word at the caret and on nothing, links to the other page and to a paragraph,
Remove Link, and equations typed after Alt+= (one switched to Linear, one after
text in its paragraph), all saved as the ops the editor recorded.
`editor/cold` is its cold OneNote 2010 read: `tools/test_link_edit.py`
compares it with the candidate and checks the links and MathML in
`editor/expected.json`. OneNote's read opens an address without a scheme over
http and links URL text again when it opens a page; its own section does the
same after Remove Link (`native-typed/cold`, its cold read, shows `Gone
ftp://h.example/f` linked again). Regenerate with
`CANVAS_LINKS_EQUATIONS_EXPORT` set to a new directory, move `expected.json`
beside the candidate, and cold-open with `--expected-pages 2 --screenshots`.
