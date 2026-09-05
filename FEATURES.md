# Milestone 6 feature matrix

The matrix follows MS-ONE's document families, including features absent from the
personal corpus. Each row requires an independently authored native fixture,
Rust interpretation checks, and a review-report inspection before acceptance.
A retained opaque payload is evidence of preservation, not interpreted coverage.

| Family | Required cases | Specification  Evidence |
| --- | --- | ---  --- |
| Notebook structure | Multiple notebooks, section groups, section ordering/colors, duplicate names, empty sections | 2.2.14–18, 2.2.91–96  [Native feature controls](corpus/m6/native-features-01/ORACLE.md); [Rust scale](evidence/m6/rust-scale-final-01.log) |
| Pages | Titles/alternate titles, order, subpages, duplicate text/titles, authors/timestamps, RTL | 2.2.19, 2.2.29–31, 2.2.58, 2.3.74  [Direction/origin controls](corpus/m6/native-page-direction-03); [private review](evidence/m6/private-current-report-05/qa/review.json) |
| Outline hierarchy | Positioned outlines, nesting, outline groups, indentation, collapsed/hidden content | 2.2.20–23, 2.3.8, 2.3.18–19  [Structure](corpus/m6/native-structure-01/ORACLE.md); [saved collapse](corpus/m6/rust-collapse-control-01/ORACLE.md) |
| Text | ASCII/Unicode preference, surrogate pairs, combining marks, mixed scripts, empty/long paragraphs | 2.1.4, 2.2.5, 2.2.23, 2.2.89  [100 native histories](evidence/m6/native-histories-independent-03.log); [break controls](corpus/m6/native-break-controls-02) |
| Character formatting | Mixed runs, font/size/color/highlight, bold/italic/underline/strike, super/subscript, language | 2.2.43–45, 2.2.76–77, 2.3.9–16  [Native probes](corpus/m6/native-probes-01/ORACLE.md); [private native comparison](evidence/m6/private-current-compare-01.log) |
| Paragraph formatting | Named styles, alignment, spacing, RTL, style inheritance | 2.2.44, 2.2.80, 2.2.83, 2.3.81–83  [Structure](corpus/m6/native-structure-01/ORACLE.md); [feature report inspection](evidence/m6/features-report-05/qa/review.json) |
| Lists | Bullets, numbering, restarts, mixed indentation, custom fonts/formats | 2.2.25, 2.2.57, 2.3.20, 2.3.43  [Structure](corpus/m6/native-structure-01/ORACLE.md); [numbering controls](corpus/m6/native-features-01/ORACLE.md) |
| Tables | Multiple rows/columns, nested content, widths, shading, borders, locked columns | 2.2.26–28, 2.2.66, 2.2.70, 2.2.97  [Widths/locks](corpus/m6/native-structure-01/ORACLE.md); [RTL](corpus/m6/native-page-direction-03); [shading limitation](corpus/m6/cell-shading-control-01/ORACLE.md) |
| Links | External/internal links, formatted labels, embedded text-run data | 2.2.78, 2.2.82, 2.2.90, 2.3.75–77  [Native link controls](corpus/m6/native-link-controls-01); [empty-label regression](corpus/m6/native-empty-link-01) |
| Images | Multiple formats, sizes, alt text, filenames, internal/external containers, background/printout images | 2.2.24, 2.2.36, 2.2.59, 2.2.75, 2.2.79  [Native formats/roles](corpus/m6/native-features-01/ORACLE.md); [browser inspection](evidence/m6/features-report-05/qa/review.json) |
| Files and media | Attachments, original filenames, recording identifiers, audio/video payloads and associated text | 2.2.32–33, 2.2.60–61, 2.2.71–72  [Native attachment/recording controls](corpus/m6/native-features-01/ORACLE.md) |
| Ink and embedded objects | Strokes, placement, native payload/export comparison, math and embedded text-run objects | Native ink corpus; 2.3.79–80  [Native ink](corpus/native-ink/cold-ui-ink); [math](corpus/m6/native-math-01/ORACLE.md): payload/run preservation, opaque rendering |
| Tags and tasks | Standard/custom tags, multiple tags, checked/unchecked state, task status/dates | 2.1.9, 2.2.41–42, 2.2.84–88, 2.3.85–96  [Native task controls](corpus/m6/native-features-01/ORACLE.md); [private tags](evidence/m6/private-current-compare-01.log) |
| History and recovery | Version pages/contexts, recycle-bin pages, conflicts and their source relationships | 2.1.1–2, 2.1.17, 2.2.34–40, 2.2.86  [Private history review](evidence/m6/private-current-report-05/qa/review.json); [live conflicts/recovery](corpus/m6/live-collaboration-15/ORACLE.md) |
| Protection and unknowns | Locked section identity/ciphertext, unknown JCIDs/properties, malformed structures | MS-ONESTORE encryption; MS-ONE 2.1.5, 2.1.12  [Existing corpus checks](evidence/m6/corpus-regression-01.log); [document fuzzing](evidence/m6/document-public-fuzz-08.log): ciphertext/raw data preserved |
| Scale and interactions | Rust/native large notebooks, many pages/objects/assets, repeated identities/text, mixed feature histories | All applicable rows  [Independent native histories](evidence/m6/native-histories-independent-03.log); [Rust scale](evidence/m6/rust-scale-final-01.log) |

[M6-ACCEPTANCE.md](M6-ACCEPTANCE.md) records the native builds, comparisons,
report inspections, regression campaigns and interpretation boundaries. `evidence/m6/spec-objects.json`
is the current object-definition inventory extracted from the supplied markdown.
