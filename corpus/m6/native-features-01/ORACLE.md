# Native feature controls

OneNote 14.0.4763.1000 authored 19 pages alongside the Rust seed page, then
exported XML, PDFs and attachment bytes. The frozen input is
[notebook/fixture.json](notebook/fixture.json); [scripts/author.ps1](scripts/author.ps1)
contains the executed author. The input generator validates every page against
the unmodified 2010 COM schema (`OneNote2010.xsd`, kept outside the repository).

The fixture covers PNG/JPEG/BMP/TIFF/GIF import, alternative text and image links,
all four background/printout flag combinations, an attachment, a WAV recording
and its timed annotation, inherited strikethrough and paragraph spacing, RTL,
collapsed nested paragraphs, repeated breaks, numbering formats 0–4, a disabled
task with identity and dates, duplicate titles, three page levels, nested section
groups, duplicate section names in different groups, and an empty section.
Native XML exports BMP, TIFF and GIF images as PNG; JPEG remains JPEG.
The stored TIFF payload remains TIFF. The report preserves it and creates a
PNG browser preview whose pixels are checked against the native XML export.

The printout controls isolate a native framing distinction. All four inputs
request 288 × 192 points. Without `isPrintOut`, stored layout maxima are exactly
288 × 192. With it, they are 286.56 × 190.56, independently of `backgroundImage`.
Native XML includes the 0.72-point border on each side; native PDF bitmap bounds
agree with the smaller stored dimensions within PDF coordinate rounding.
The corresponding stored Boolean is `0x08001d85`; the model retains both flags
and exposes the bitmap dimensions without adding the frame to them.

The recording GUID is stored in `0x1c001c97`. Its annotation stores the same GUID
in `0x1c001c98` and 500 milliseconds in `0x14001c99`, agreeing with native
`MediaIndex`. The WAV payload is checked byte-for-byte against the native cache.
COM import does not set `IRecordMedia` for this fixture; the model leaves that
property absent. The separate private video has the documented value 2.

The task's GUID, disabled/completed bits and four dates are checked against
`OutlookTask`. Numbering format 0 ends in a meaningful UTF-16 NUL code, which
must survive decoding. Rust assertions are in [tests/document.rs](../../../tests/document.rs).

Reproduce the native semantic comparison with the bundled Python runtime:

```sh
cargo build --example document
python3 tools/verify-document.py corpus/m6/native-features-01/notebook corpus/m6/native-features-01/read
```

The comparison passed 20 pages, the task association and 8,576 explicit
character-format comparisons. These results establish model and native
agreement; report visual inspection is a separate acceptance gate.
