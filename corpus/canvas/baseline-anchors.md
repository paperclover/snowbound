# Native baseline and paragraph-spacing control

[baseline-anchors.one](baseline-anchors.one) contains only synthetic text and three generated opaque PNG anchors. OneNote 2010 14.0.4763.1000 authored and saved it in a disposable Windows 7 SP1 clone. No private notebook content or font binaries are embedded as test inputs.

[baseline_fixture.py](../../tools/canvas/baseline_fixture.py) generates its authoring input for the existing native runner:

```sh
python3 tools/canvas/baseline_fixture.py NEW_INPUT_DIRECTORY
python3 tools/native_runner.py NEW_INPUT_DIRECTORY NEW_CAPTURE_DIRECTORY --author tools/native/pages.ps1 --pdf --screenshots
```

The controls cover Arial 11/16 pt, Calibri 11/17 pt, mixed sizes, long-word wrapping, blank paragraphs and paragraph spacing. The native runner navigates to each page before PDF publication; calling `Publish` without navigation crashed the standalone capture experiment in `ONMain.DLL`.

The spacing outline contains `Spacing first`, an empty paragraph and `Spacing last`. Native XML reports 108 pt before and 144 pt after each paragraph, with total outline height 324.8671875 pt. Native first-line placement starts at the outline origin: outer spacing is omitted and only the two 144 pt gaps contribute. The editor regression verifies these boundary/gap semantics through split and undo, using the active font's text heights so it does not require redistributed Arial files. Spacing properties remain stored even when they do not contribute at the outline boundary.

The baseline comparator registers PDF images by identical decoded pixels and unique occurrence. It retains translation candidates, their spread and size ratios before comparing text baselines. It does not fit to text positions or apply a passing tolerance. Baseline comparison excludes mismatched lines. PDF pagination leaves the final spacing paragraph without image registration on page two.

The 110 pt Arial 11 long-word control wraps at UTF-16 offsets 8, 22, 42 and 59. Emergency word breaking reproduces these native offsets; ordinary word wrapping allowed the alphabet to overflow its line. The layout regression checks bounded ASCII advances, complete source coverage and caret round trips without fixing font-dependent offsets in the test.

Native font hashes:

- Arial: `001bb08e859d4db7814902119412a14713b0c45e89cbc429bb3f5e6af14815e0`
- Calibri: `436cb479a8f9eff517016868323bdfbca1a053bba4cc55c8753859b64d041c5c`

Full XML/PDF exports, repeated native geometry, display captures and comparison results remain in the session's external `baseline-anchors` evidence directory. Screen capture recorded 96×96 DPI and requested 100% zoom; PDF baseline residuals are not screen-baseline acceptance.

Separate Windows GDI calibration in the external `glyph-baseline-calibration` directory recovers screen baselines through exact glyph-prefix pixels at a known baseline. It accounts for the capture's RGB565 color depth and keeps spell-check pixels outside glyph bounds separate. Fractional-position controls distinguish rounding an outline origin and local baseline separately from rounding their sum. A wrapped-line counterexample prevents treating that first-line result as a universal native coordinate policy.

Private-page calibration in the external `album-screen-origin` evidence uses three markers from this fixture, inserted only into a disposable page copy. Their repeated pixel bounds establish the page origin independently of text; body pixels and outline geometry remain unchanged across insertion. The measured origin corrects a one-pixel comparison offset. Exact native glyph-prefix matching in `private-screen-baselines` then uses native outline bounds and unambiguous source-order assignments; canvas baseline predictions do not select matches.
