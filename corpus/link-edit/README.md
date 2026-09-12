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
