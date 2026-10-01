# Native right-to-left page control

`scripts/author.ps1` changes PageSettings RTL to true and locks the existing
table columns to 96 and 144 points through OneNote COM. `before.xml`, `after.xml`
and `read/page-000.xml` retain the native observations. The saved store uses
visual left-to-right column order [Right cell, Left cell], with widths [144, 96].
COM exports cells in page direction [Left cell, Right cell], with widths [96, 144].
The native PDF independently confirms the physical cell order. Native x is store x -
PageMarginOriginX - 36; the native table outline measures wider than its stored suggested
maximum width and keeps its right edge, store x + LayoutMaxWidth - PageMarginOriginX - 36
equalling native x + native width (`corpus/rtl-page` has narrower outlines keep their left).
Vertical coordinates retain the 14.4-point canonical origin.

The public fixture includes images, a file and two opaque ink objects. All
supported text, object order, payloads, formats and coordinates pass
`tools/verify-document.py`. Ink stroke bounds remain distinct from the stored
container position; the comparator checks ink count and excludes stroke geometry.
`teardown.json` records deletion of the authoring clone.
