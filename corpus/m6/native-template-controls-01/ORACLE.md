# Native default page template and unaligned image container

The captured UI operations selected Hearts from the native Templates pane, selected Hearts - Decorative as the section default, and created a new page. The default background appeared automatically; `default-template.png` and `ui-operations.json` record that result. The clone was deleted.

The section property 0x2c001d62 refers to a separate page object space whose role-2 metadata has JCID 0x2003e. Property 0x1c001ca5 contains the UTF-16 template name Hearts. These native template fields are not in the published MS-ONE object list; the typed model identifies them as observed native metadata. The original personal notebook uses the same relationship with the name Ivy.

The embedded image object starts at byte 11396, four bytes past an eight-byte file boundary. Its payload is 10056 bytes; the footer begins at 21492 after four padding bytes. MS-ONESTORE 2.6.13 pads the object length, independent of its file position. This fixture exposed the reader's incorrect absolute-offset alignment. The regression relocates the object through all eight offsets and separately corrupts its footer.

The stored picture is Snowbound's rendering of Hearts (SHA-256 6a0593db927c1a34a94cf11093e5bd8700843323ed4fe8c905df0c595cb09c26), written over Microsoft's at the same length with a PNG comment as padding, so the file is otherwise native. `read/` is a cold OneNote read of that notebook, whose XML exports the same image bytes; `default-template.png` still shows Microsoft's art as OneNote drew it during the capture.
