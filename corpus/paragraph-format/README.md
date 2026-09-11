# Native paragraph formatting controls

`tools/native/paragraph-formatting.ps1` authored one page, "Paragraph
formatting", with six paragraphs of mixed bold/italic/plain runs: native
defaults, explicit zero values, centred spacing, right-aligned RTL, a named
style (`h1`, Georgia 18 bold with spacing) inherited, and the same style with
paragraph overrides. `before/` holds the authored notebook and its native read
(`before/read`, captured with `--expected-pages 2 --collect-notebook`;
`before-capture/` retains that run's records). OneNote reports the style's
inherited `spaceBefore="8"`/`spaceAfter="4"` as 288/144 in the read, while
explicit paragraph values read back in points.

`candidate/` is the page writer's output for the model edit in
`crates/onestore/tests/paragraph_formatting.rs`: every paragraph set to centre
alignment, zero space before, 2 pt after and 16 pt line spacing, with runs,
direction and quick styles untouched. `cold/` is its fresh OneNote 2010 read:
each paragraph is centred with `spaceAfter="2.0"` and `spaceBetween="16.0"`,
the zero space before is omitted, `RTL="true"` survives on the right-aligned
case, `quickStyleIndex` and the bold/italic runs match `before/read`, and
`tools/verify-document.py` finds no differences. `tools/test_paragraph_format.py`
checks these without a VM.

Regenerate the candidate with `ONESTORE_PARAGRAPH_FORMAT_EXPORT` set to a new
absolute directory while running
`paragraph_properties_preserve_native_mixed_runs_and_shared_styles`, then
cold-open it with `tools/native_runner.py OUTPUT COLD --expected-pages 2
--collect-notebook`. Regenerate `before/` by running the runner on
`corpus/create-notebook` style input with `--author
tools/native/paragraph-formatting.ps1 --expected-pages 2 --collect-notebook`.
