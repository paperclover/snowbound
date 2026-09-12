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
