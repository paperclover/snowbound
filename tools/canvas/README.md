# Canvas experiments

The design record that scopes this work is kept outside the repository.

## macOS text canvas

The host opens nonempty UTF-8 text as paragraphs in one temporary text outline. Its editor reuses `onestore::document::Format`, retains paragraph layout across selection changes, and keeps notebook storage outside the editing boundary. The window title and close/quit guard expose the temporary lifetime. Escape and Return dismiss the discard dialog and keep the page open.

```sh
python3 tools/canvas/build_macos.py
"target/Snowbound.app/Contents/MacOS/Snowbound"
cargo test -p canvas --features interaction
cargo test -p draw -- --ignored
cargo test -p canvas --features gpu gpu:: -- --ignored
```

The builder signs and verifies the local bundle: with team `9R7DPNW28H`'s Developer ID Application identity (found in the keychain by team, chosen by SHA-1, or `--sign-identity SHA1`) and a Developer ID provisioning profile for `net.paperclover.snowbound` naming `iCloud.net.paperclover.snowbound` (found in Xcode's profile folders, or `--profile PATH`), with hardened runtime and the iCloud container, which iCloud notebooks need; without both, ad hoc. `--sign developer-id` fails instead of falling back, and `--sign ad-hoc` skips Developer ID. To preserve an existing app during review, provide a new bundle path and a distinct identifier together:

```sh
python3 tools/canvas/build_macos.py --release --output '/PATH/Snowbound Review.app' --bundle-id net.paperclover.snowbound.review
```

The explicit output path must not exist. Its filename supplies the bundle's display name; the default build continues to update `target/Snowbound.app`. A separate identifier is signed ad hoc, without the container. Nothing here notarizes.

Control-Command-Space opens the macOS character picker for the active text field.

Omit both arguments for a blank page with provisional caret placement, or provide a UTF-8 file and optional document width (default 480 pt). Typing, single/double/triple-click selection, drag selection, arrow/word/line/paragraph/document navigation, Backspace/Delete, word/line deletion, indentation, clipboard shortcuts, Cmd-Z/Shift-Cmd-Z and IME use the shared editor. Tab/Shift-Tab indent plain paragraphs; Enter retains their indentation. Cmd-Up/Down reach document boundaries, Option-Up/Down reach paragraph boundaries, and Home/End scroll without moving the caret. Mid-text Tab currently indents; OneNote’s table creation is not implemented. Scrolling is bounded by page objects, including negative coordinates; Cmd-wheel or Cmd-plus/minus zooms without changing document width, and Cmd-0 returns to 100%. The GPU test requires an available native adapter and compares readback pixels before/after repaint and forced glyph/image-cache eviction.

```text
onestore::Format
       ↓
canvas       text edits · inverse history · composition · paragraph layout · structural edits
       ↓
canvas::gpu     page scene · text runs, tag icons, pictures and ink as draw primitives
       ↓
draw            Swash glyph rasterization · custom wgpu quads · bounded glyph/image caches · layers
                · draw::edit: key chords, click counts and caret steps within one layout
       ↓
canvas::interaction   hit layers · drags · placement grid · picture handles · chrome · key routing
                      · scroll bounds and zoom · caret blink · AccessKit tree
       ↓
ui          immediate-mode boxes · keyed state · autolayout · event routing · labels · widgets
            · shell: section tabs, toolbar buttons and drop-downs
       ↓
snowbound   title bar · toolbar · section frame · page tabs · the page as a custom box · Winit window and events
            · AppKit pickers and quit guard · clipboard · saving
```

## Desktop shell

`--notebook FOLDER` opens a notebook laid out as OneNote lays one out. The title bar shows the application's icon and the notebook's name; below it two rows of toolbar buttons follow the Home ribbon's Basic Text group, the first nine tags and zoom (only zoom acts yet). Readable top-level sections are tabs in the notebook's order, each leaning over the next at 45° with a faint highlight along its top, and the open one on top, casting a soft shadow and framing the page and the page tabs on the right in its colour, which shades slightly down the window; a tab that opens rises to meet the frame as its outline fades in. The frame's top corners and the page's corners are rounded concentric with the window's. The open page's tab is the page's colour and joins it with rounded inside corners; subpages are indented. The interface follows the system's light or dark appearance, and in the dark one the page's paper darkens and text left at OneNote's automatic colour turns light (except on a highlight), resolved when painting so nothing re-lays out; explicit colours, highlights, pictures and ink keep theirs, except that the background art of OneNote 2010's page templates, recognised by the picture's hash, paints from bundled vector recreations drawn for the paper. Sections take their stored colour's hue at a saturation and lightness chosen for each appearance. The section's colour is also the interface accent, easing to the next section's when another opens. The search box, New Page and the page-tab toggle sit above the page tabs. Command-F searches the page titles; Enter opens the first match and Escape clears it. A page that also changed on another computer shows Keep mine / Keep theirs above it. `--section FILE TITLE` opens one section the same way at that page. Section files must be regular files; discovery rejects symbolic links.

Launched without a file, the app reopens the notebooks open when it last closed, at the one shown last, or shows "No notebooks open" with New Notebook and Open Existing. Settings (the open notebooks, the sidebar, recent fonts and the user name edits are stored under, which Settings… (⌘,, Ctrl+, on Linux, or Options in the sidebar) changes and which starts as the account's full name) are JSON in `~/Library/Application Support/Snowbound/settings.json` or `$XDG_CONFIG_HOME/snowbound`; `--settings FILE` uses another file, `--notebook FOLDER` adds a notebook to them, and `--screenshot` never writes them. Open Existing takes a notebook folder, its `Open Notebook.onetoc2` or a section file, which opens in the notebook its folders make up. A notebook on a mounted SMB share opens through the embedded SMB client, signed in with the password the system keychain holds, and through the mount only when that fails. The notebook button left of the section tabs opens OneNote 2010's navigation bar: the open notebooks with their sections and section groups, the open section marked; dragging a row reorders a folder or, onto a group, moves into it, and right-clicking a row, a section tab or a page tab offers OneNote's commands (Rename, Delete to the notebook's recycle bin, New Section, New Section Group, Close This Notebook; Delete, New Page, Make Subpage, Promote Subpage). Page tabs drag to reorder. The `+` above the page tabs adds a page at the end of the section, titled with its date and time as OneNote titles one, and puts the caret in its title; until its body is typed in, drawn on or clicked, templates offer our recreations of OneNote 2010's art (Ivy first), its page colours and Informal Meeting Notes' content, written as ops (`corpus/notebook-management`).

As in OneNote 2010, Notebook Recycle Bin (File menu, a notebook's context menu, or the palette) shows the notebook's recycle bin as its own row of tabs, read-only under a bar: a page's context menu restores it to a section, keeping its identity, copies it out, or deletes it for good, a section's restores it to a folder, and Empty Recycle Bin deletes what it holds (`corpus/recycle-bin-view`). Pages another author changed since they were viewed are bold in the page list, and so are their section's tab and row and the groups and notebook holding them, until left or marked read (Mark as Read, Ctrl+Q off macOS; Mark Notebook as Read; Next Unread; Show Unread Changes in This Notebook); what was read is kept in the cache folder's `read.json`, as OneNote keeps it in its cache. Save As (⇧⌘S) writes the page or section as a OneNote section, the notebook as a OneNote package, or any of them as a PDF; opening a `.onepkg` asks, as Unpack Notebook does, for its name, colour and folder (`corpus/notebook-package`).

The `ui` crate builds every frame from the application's state. A cache keyed by stable box ids keeps hover, press, focus, scroll and animation values, and the previous frame's layout routes the frame's input before building, so a frame answers input one layout late and paints with its own. Sizes are solved per axis after building: fixed, label-sized, a fraction of an ancestor, or the sum of children, with overflow shared out by each box's strictness. The page is a custom box: `ui` routes it the pointer, wheel, key and input-method events that land on it, in order, and the host hands them to `PageView` and paints the page in a clipped layer of the same frame. Page scrollbars are `ui` widgets over that box; the canvas reports only its scroll bounds. Text fields edit through `draw::edit`, as the page does, so keys, clicks and drags select and move the same way in both. Work the frame asks for (page requests, saving) runs after the frame is painted and requests the frame that shows it. Opening a section or page reads and lays it out on a thread of its own while the current page stays live; the newest replaces the page once the pictures it shows first are drawn, or after 200 ms. Its tab is selected at once, and an open that takes over 80 ms shows a page's outline in place of the page leaving, which takes no input. The pages beside the open one, the sections beside the open section and a hovered page or section tab are read ahead on a thread of their own. A notebook keeps the last three sections left or read ahead open, which `Library::open` hands out, and the last 32 pages shown or read ahead keep their scenes, with up to 128 MiB of decoded pictures, so a page shown again is laid out afresh around pictures already drawn.

A covered window receives no redraws, so interaction can be scripted: `SNOWBOUND_REPLAY` names a file of `move X Y`, `press [right]`, `release [right]`, `wheel DX DY`, `pinch FACTOR`, `key NAME`, `type TEXT`, `modifiers [shift] [command]`, `wait MS`, `snapshot PNG_PATH`, `accessibility TEXT_PATH` (the window's accessibility tree, a node a line, as the platform shows it), `appearance light|dark`, `resize WIDTH HEIGHT` and `quit` lines in logical pixels, and each step and every 16 ms of a wait draws a frame. With `--screenshot` as well, the window stays hidden and only the replay's snapshots capture it. Snapshots wait for the page's pictures, file icons and background art to finish rasterizing; `SNOWBOUND_FRAMES` frames never wait, so art appears in the first frame drawn after it is ready. Snapshots omit the window's traffic lights, which AppKit draws. Replays edit and save like a user, so point them at a copy of a notebook. Their edits carry the settings' user name, so give fixture runs a `--settings` file holding `{"user_name": "Snowbound Test"}`. A replay or `--screenshot` run never reads the account's own settings or iCloud: without `--settings` it starts from none, and its iCloud folder is only the one `SNOWBOUND_ICLOUD_FOLDER` names.

```sh
cp -RL SOURCE_NOTEBOOK /tmp/notebook-copy && chmod u+w /tmp/notebook-copy/*.one
SNOWBOUND_REPLAY=SCRIPT "target/Snowbound.app/Contents/MacOS/Snowbound" --notebook /tmp/notebook-copy --cache /tmp/notebook-cache
cargo test -p ui
```

The renderer allocates a 2048² RGBA glyph atlas and a 3.5 MiB vertex buffer, limits combined text-glyph and tag-icon entries to 8192, and rebuilds a frame after atlas eviction. Font scaling uses 32 cache entries. These limits bound those caches; they are not a process-memory or worst-case rasterization budget. Drawing and hit-testing share Windows-metric line boxes. First-baseline fidelity and substitute-font metric selection remain experimental; the `windows_*` probe values expose that policy independently from Parley's original metrics.

The `draw` crate owns the renderer so the page and application chrome share one pass, atlas and image cache. It paints layers, each a transform and device clip over a flat sequence of glyph runs, SVG icons (in their own colours or tinted), SVG paths filled or stroked, rectangles (rounded, or shaded top to bottom), pen strokes and immutable raster images. Icons and paths rasterize once per size and quarter-pixel phase into the glyph atlas. The renderer is the default `render` feature; `canvas` without `gpu` uses only `draw::edit`. Text reaches it through the `Glyphs` trait, which visits positioned glyph runs, so the page's Windows-metric line boxes stay in `canvas` and the renderer never measures text. Consecutive primitives with the same texture and clip share a batch without changing paint order. The host emits selection, composition and caret rectangles through that same interface; repainting multiple placements reuses the retained layout and glyph cache.

Glyph coverage uses quarter-pixel phases with the vertical sign converted to Swash's Y-up coordinates. The `draw` GPU readback test checks nine successive quarter-pixel translations by their linear-light ink centroids, then requires identical pixels on repaint, cache eviction and renderer reconstruction on the same device. The native caret experiment exposed the previous inverted vertical phase; correcting raster placement does not change line metrics or wrapping.

Image textures have a separate 64 MiB / 256-entry cache. Offscreen images do not upload or consume the active-frame budget; a frame whose visible images exceed that budget returns an error. Under pressure, eviction preserves every image needed by the current frame, and a texture goes when every copy of its image has. PNG/JPEG decoding reads the size from the header first, checks encoded size and dimensions against a 256 MiB decoding limit with a best-effort codec allocation limit, and shrinks the picture to the size asked for, filtering premultiplied values in sRGB encoding. These limits do not bound all CPU buffers or driver allocations. Images use linear filtering of premultiplied, linear-light colors; glyphs retain their own sampling/blending policy. Decoded color values are currently interpreted as sRGB; ICC and orientation metadata are not transformed by this experiment.

## Note tags

Paragraph layout retains checkbox (shape 3), question mark (15), exclamation (17), red, yellow and blue square (100–102) and musical note (121) tags from paragraph and rich-text nodes. Tags are stored newest first and drawn oldest first: an outline has one tag column, as wide as its most-tagged paragraph needs (12 pt per tag), whose left edge sits 20.25 pt plus 12 pt per extra tag before the list marker or text origin, and each paragraph's tags start at that edge. Tags do not change text advances, wrapping or line height. A tag's font colour or highlight paints the whole paragraph, the newest such tag winning; shape 0 draws no icon. Positions, order and colours match OneNote 2010 on the notebook's lyric pages. Hand-authored vector paths use Swash's existing rasterizer and the text atlas, including its subpixel phases, offscreen culling and eviction. No Office bitmap assets or tag fonts are bundled.

Checkboxes display completion; noncheckable tags ignore the completion bit required by the file format. Disabled tags dim. Accessibility descriptions expose source labels and checkbox/disabled state without changing selectable text. Tag state is read-only; text edits and undo retain it. Clicking a tag gutter focuses its outline. Unsupported icon shapes and task tags return `UnsupportedContent` during layout.

## Run formatting

Superscript and subscript runs draw at two thirds of their size, a third of the run's size above the baseline or 8% below it, with their underline and strikethrough; line metrics and hit-testing keep the paragraph's baseline. Links draw blue and underlined unless coloured. Paragraph alignment offsets lines within the wrap width.

## Equations

A paragraph stored as an equation draws in two dimensions from the tree `onestore::page::Math` parses: sub- and superscripts at 70%, fractions over a rule on the math axis, radicals drawn with the pen, n-ary operators at display size with limits above and below (integrals take them as scripts), stretched fences, accents and bars above, boxes, matrices and equation arrays. Latin letters draw in mathematical italic. Cambria Math falls back to STIX Two Math on macOS, and atoms are placed by their glyph ink rather than the math font's tall line box; an equation line is at least as tall as a line of text. The linear text stays the editable source, laid out in the body font so the caret keeps a text height; editing equations in two dimensions is a follow-up.

## Font substitution

`TextEngine::default()` lays out Calibri, Arial, Times New Roman and Courier New in the bundled Carlito 1.104, Arimo 1.341, Tinos 1.340 and Cousine 1.241 (`crates/canvas/assets/fonts`, SIL Open Font License) wherever the system lacks them. The font box names such a family with its substitute, as "Carlito (Calibri)"; stored `Format.font` values keep the document's name.

The host and `layout-probe` also accept repeated `--substitute-font FONT_FILE` options for another Arimo, Carlito, Caladea, Tinos or Cousine file. Each file is registered only in this process, as its family's counterpart; this explicitly replaces that family even when a system copy exists. No font files are installed.

```sh
"target/Snowbound.app/Contents/MacOS/Snowbound" TEXT_FILE 240 --substitute-font ARIMO_FILE
cargo run -p canvas --bin layout-probe -- corpus/canvas/text-cases.json --substitute-font ARIMO_FILE --substitute-font CARLITO_FILE
```

Arimo's horizontal-header ascent/descent match the measured Arial Windows extents; its larger OS/2 Windows extents do not. The substitute uses those horizontal-header extents without line gap, scoped to the registered font data. Carlito uses its OS/2 Windows extents. Other text fallbacks retain their own metrics. Color emoji use the paragraph’s text baseline and scale within its line box; an explicit font-size change still changes line height. The comparator reports actual `canvas_height` and `canvas_height_residual` separately from the raw Windows-table hypothesis.

Arimo 1.341 and Carlito 1.104 match all 14 recoverable native ASCII wrap controls, with maximum outline-height residual 0.000054 pt. This does not establish matching coverage or advances for every script: the mixed Hebrew/Arabic control rewraps under substitution and resolves Arabic to Geeza Pro on this Mac. The 500-case Unicode source-coverage test passes with these substitutes across 1804 visual lines. Font files, hashes, native comparisons and host paste/undo captures remain in the external evidence bundle.

## Structural text editing

`TextDocument` owns a nonempty sequence of `PageParagraph` nodes, each with one rich-text object. Paragraph and rich-text identities survive edits; split/join preserves surviving identities and allocates new ones only for new objects. Construction and replacement reject duplicate identities, missing or forward parent links, and children at the same or shallower indentation level as their parent. In-paragraph replacement retains modeled hierarchy, lists, tags, collapse state and paragraph formatting. Structural replacement requires flat, untagged content. Document changes are internal to the canvas crate; callers edit through `CanvasEditor`.

`CanvasEditor::from_page` owns the working page: text outlines, date fields, image payloads and positions, and retained unsupported objects. GPU support is optional. Text has one visible projection and layout per paragraph, with source UTF-16 selections and affinity at wrap boundaries. Title growth shifts eligible body objects across columns in the same undo transaction; shrinking keeps their positions, while undo and IME cancellation restore them. Background images remain fixed. `CanvasEditor::from_outlines` takes owned outlines and their list/tag definitions, preserving source coordinates, widths, indentation and identities. Rendering and editing share `ParagraphLayout` shaping and outline arrangement, including list-marker height and adjacent spacing. Paragraph spacing contributes only between visible paragraphs; leading and trailing spacing properties remain stored without expanding the outline boundary. Collapsed descendants retain source nodes while navigation, hit-testing and selection geometry use visible paragraphs. LF insertion splits flat paragraphs; boundary Backspace/Delete joins them. Replacements stage every affected paragraph's layout before publishing the edit. Undo restores full affected nodes, including their identities, empty styles and reversed selections; untouched paragraphs retain their layouts. Resizing keeps source selection and history while recomputing geometry at the new width.

The macOS host uses this same editor for typing, pointer/keyboard selection, clipboard text, IME, split/join and undo. Click blank canvas or press Cmd-Shift-N to place a provisional caret with an automatic width capped at 468 pt; the keyboard command places it beside the focused outline or read-only object. The first committed input creates an outline in one undo step; placement alone preserves history and does not dirty the page. Drag an outline's top bar to move it on the page's 18 pt grid, which OneNote anchors at the page's margin origin. Hold Option for free pointer placement or dragging; Escape cancels a drag. Use Cmd-Option-arrows (1 pt; Shift makes it 10 pt). Ctrl-Tab and Ctrl-Shift-Tab cycle text outlines and read-only placeholders. A drag previews placement and commits one undo step on release; Escape or loss of window focus cancels the preview. Creation, movement and text edits share one history, while untouched outlines retain their text layouts. Composition can provisionally replace a selection spanning paragraphs; cancellation restores the original paragraphs, selection and redo, while commitment creates one undo step. Vertical navigation retains its preferred column across short or empty paragraphs. Horizontal navigation enters the neighboring paragraph at its visual edge, including when text direction changes.

Core tests compare 220 Unicode range/replacement combinations with plain-text replacement, and compare incremental layout with full recomputation through a 64-edit structural history and an 80-step multi-outline history followed by complete undo/redo. They also exercise hidden-field mapping, paragraph-separator selection, grapheme deletion, multiline IME and failed-layout atomicity. Live macOS captures verify Enter, Backspace join, undo/redo, Option-E dead-key commit and the close guard. Native Pinyin input verifies Escape cancellation over single- and cross-paragraph selections: original text, selection and redo survive. Accepting a candidate across paragraphs commits one undo step. Input-event logs distinguish empty preedit cancellation from candidate commitment. Escape commits the spacing acute accent in both this host and a native TextEdit control; that dead-key case is not evidence of composition cancellation.

`cargo +nightly fuzz run canvas-editor /absolute/corpus-directory -- -max_total_time=60 -max_len=256 -artifact_prefix=/absolute/artifact-directory/` runs the canvas state machine under AddressSanitizer. Create both external directories first. Each input applies up to 32 Unicode range replacements with mixed formatting, optional provisional composition/cancellation, undo/redo and width changes. Plain-text replacement is checked independently; paragraph metadata and selections must survive undo/redo, and retained line geometry must match a freshly constructed editor. The target uses installed fonts and a retained text engine; it does not read notebooks or exercise GPU drawing. Keep generated corpus and failure artifacts outside the repository.

The private import check edits and restores each body and title paragraph after dropping the source file bytes. It compares complete modeled nodes before/after undo and redo, including metadata on nested and tagged content. It exercises the document boundary without launching or modifying the live app:

```sh
CANVAS_TEST_SECTION=PRIVATE_SECTION_COPY CANVAS_TEST_PAGE=EXACT_PAGE_TITLE cargo test -p canvas imported_nodes_preserve_identity_through_edit_and_undo -- --ignored --nocapture
```

The editor integration check additionally edits every visible body paragraph, compares incremental geometry with fresh outline layout, and requires undo/redo to restore the original/edited geometry exactly. All 269 body paragraphs in the three frozen references pass. Set `CANVAS_TEST_SUBSTITUTE` to the optional Carlito font file when reproducing that check:

```sh
CANVAS_TEST_SECTION=PRIVATE_SECTION_COPY CANVAS_TEST_PAGE=EXACT_PAGE_TITLE CANVAS_TEST_SUBSTITUTE=CARLITO_REGULAR_FILE cargo test -p canvas imported_editor_reflows_and_restores_native_outline_geometry -- --ignored --nocapture
```

## Native text accessibility

AccessKit exposes each text outline as an independent editable macOS text area. Logical text runs carry the canvas line boxes, cluster advances and source positions, including blank paragraphs and hidden-field projection. They use the editor's retained projections and explicit source indices; separators connect the next visible paragraph across collapsed children. Native selection, whole-field replacement and keyboard edits share the editor history. Focus transfers retire native AppKit preedit when the canvas commits or cancels composition, preventing dead-key state from transferring to another outline. Selection/navigation reveals the caret; pan and zoom keep their own viewport behavior. Accessibility updates run only when the adapter is active and the host changes, not for caret-blink redraws. Cached paragraph runs reuse text and character geometry when the shaped-layout identity, source index, next visible paragraph and local origin match; reflow or changed source mappings invalidate them. The cache retains only the current page’s paragraph ranges and clears on adapter deactivation. IDs survive geometry/selection updates; changed run text or source mappings receive fresh IDs so obsolete action positions are rejected.

```sh
cargo test -p canvas --features interaction accessibility
SNOWBOUND_TRACE_INPUT=1 "target/Snowbound.app/Contents/MacOS/Snowbound" SYNTHETIC_TEXT_FILE
```

Input tracing logs Winit keyboard/IME, pointer, modifier and focus events, AccessKit action payloads and applied accessibility selections to stderr, including text. Consumer tests check Unicode/hidden-field mappings, stale actions, undo, exact document text and transformed caret geometry in both affinities at wraps and bidirectional boundaries; native captures exercise AX value/selection setters. Those checks do not substitute for a VoiceOver walkthrough.

## Reference pages in the macOS host

```sh
"target/Snowbound.app/Contents/MacOS/Snowbound" --reference PRIVATE_SECTION_COPY EXACT_PAGE_TITLE --substitute-font CARLITO_REGULAR_FILE
```

This integration mode keeps imported objects read-only and opens a separate editable canvas to their left. The initial notes width defaults to 240 pt in this mode, and the reference starts 24 pt to its right. Existing text-file and width arguments also work. Typing, outline creation/movement, paste and undo operate on the temporary notes. The close guard protects those notes. Reference text is rendered but is not exposed through the host’s editable accessibility fields.

The host and offscreen renderer both use `canvas::gpu::page::PageScene`. The live scene reads current geometry from `CanvasEditor`; the static reference scene owns its imported content. Layout happens on construction and drawing reuses it. Pictures only read their headers then: before each frame, `update_pictures` decodes those in view, and half a view around it, on the scene's picture thread at their shown size in device pixels (in half-power-of-two zoom steps, 150 ms after the zoom settles), and paints each once it lands; one that fails to decode paints as a placeholder. Pictures in view shrink together to fit 32 MiB, those around it decode with what room is left, and those unseen longest are let go past it, so no number or size of pictures fails a page. `settle` waits for the whole page, for offscreen drawing. The stored bytes stay shared with the page model and save unchanged. Scene coordinates stay in source document points, while the comparison example applies its captured margin normalization through the viewport. All three shared-scene outputs are pixel-identical to their preceding title-rendering captures. Decoded pictures are capped at 32 MiB per scene, in addition to the renderer’s GPU cache budget; encoded source payloads and temporary decoder allocations are separate.

The host sleeps its caret timer while macOS reports the window occluded and requests a redraw when visibility returns. Input tracing includes draw attempts, surface occlusion/timeouts and submitted presents; submission is not a measurement of display completion. Startup failures propagate a nonzero exit code after the event loop exits.

## Editing an imported page

```sh
"target/Snowbound.app/Contents/MacOS/Snowbound" --page PRIVATE_SECTION_COPY EXACT_PAGE_TITLE --substitute-font CARLITO_REGULAR_FILE
```

`--page` opens supported body outlines and title text as temporary editable objects in their source positions. It accepts a section copy and exact page title, without the separate text-file/width arguments. Title text supports editing, wrapping, IME and undo, and remains present when empty. Date text follows its height; clicking the date or time opens the macOS date control and commits into the same undo history. Clicking a picture selects it with OneNote's dashed border, tint and eight handles. Dragging it moves it on the page grid (Option places it freely); edge handles stretch one axis and corner handles keep the aspect ratio, anchored at the opposite side. Background pictures pass clicks through. Moves and resizes share the undo history. Character editing, selection, IME and undo use the same outline model as new pages. Existing structured or tagged paragraphs accept in-paragraph edits; their structural split/join remains restricted. Newly created flat outlines support the full structural workflow. The app never writes the section file.

`PageScene::from_page` constructs the editor-owned page and reads its pictures' headers. Imported paint order stays with the editor; each text slot invokes the host’s outline painter. Movement reads the current model geometry and keeps each picture's raster. New outlines draw after the imported page. The close guard compares all object layouts, editable text and the creation timestamp with the opening state, including displacement that remains after a title is shortened.

The app's private integration test exercises the actual outline painter and accessibility consumer for each imported outline, then verifies text restoration through undo:

```sh
CANVAS_TEST_SECTION=PRIVATE_SECTION_COPY CANVAS_TEST_PAGE=EXACT_PAGE_TITLE CANVAS_TEST_SUBSTITUTE=CARLITO_REGULAR_FILE cargo test -p canvas --features interaction imported_page_widgets_and_accessibility_follow_edits -- --ignored --nocapture
```

## Unsupported page content

Pictures, files and handwriting that occupy a paragraph lay out in the outline's flow, at the paragraph indent with its spacing. A picture draws at its user-set layout size, else its intrinsic size. A file draws as OneNote does: the icon OneNote stored for it (24 pt when unsized) centered in a 54 pt column, with its name, less the extension, wrapped and centered below; a file without a stored icon keeps the empty slot. Handwriting strokes are relative to the paragraph's top-left, which the paragraph extends to reach. All three are part of the editable outline, and a picture there selects like a page picture, resizing and deleting in place through the outline's history (it keeps its place when dragged; Left and Right move the caret to the text beside it): text navigation passes over them, edits reflow around them, a selection spanning one deletes it, and Backspace or Delete does not join text across one.

A paragraph of content the canvas cannot draw becomes a labelled placeholder box in the flow (at least 160 pt wide, its stored size when larger), and the rest of its outline stays editable. An outline the editor cannot hold, such as one with no text for the caret, draws as stored without editing. Unsupported top-level objects, titles, or outlines that cannot be drawn render as read-only placeholders in source order. Missing image payloads or dimensions produce an image-unavailable placeholder. The editor owns the imported source values for those objects; the importer’s `Unsupported` record contains identity, class and layout metadata, not an opaque copy of unparsed notebook bytes. Supported body outlines still enter the editor. If none exist, a provisional caret appears to the right of the placeholders; typing creates an annotation outline.

Placeholders use source positions and at least 160 pt width, expanding their height to contain the status text. This is an explicit substitute for unavailable rendering, not a geometric reconstruction. Pointer presses focus placeholders, hide the previous text caret/selection and retire its IME composition. Typing, deletion, paste, text-selection commands, undo/redo and outline movement are suppressed while a placeholder has focus. Escape restores the previous text focus; Ctrl-Tab cycles through text outlines and placeholders. Zoom remains available, and Cmd-Shift-N creates an editable annotation beside the focused object. AccessKit exposes placeholder focus, status and bounds without text-edit actions; identities persist across editor updates and viewport changes. Focus reveals offscreen placeholders and suspends caret-blink scheduling. Hit-testing follows paint order for overlapping editable outlines and placeholders: new annotations draw last and receive hits first, while imported objects resolve from front to back. Each body outline’s header, width grips and text participate at its own position in that order before any outline’s padding or tag gutters, so a width handle stays reachable under the left padding of the outline above it. Supported titles expose editable text without move/resize handles. Pictures other than backgrounds take hits at their paint position, and a selected picture's handles take hits above everything else. Invalid geometry, malformed image data and resource-budget failures remain errors.

Page-level ink draws every stroke, nested groups included, at its stored page coordinates: each segment is a capsule (round pen tip) or a square-ended bar (rectangle tip) at the pen width, never thinner than one device pixel, in the stroke colour with its transparency. Ink extends the scroll bounds by its painted extent and saves unchanged; it takes no hits and does not move with the outlines around it.

Document and layout values are read-only outside the editor; edits update their retained geometry together. `CanvasEditor::from_outlines` remains available for text-only construction.

## Owned page import

`Page::from_revision` copies one page's text, outline hierarchy, list/tag definitions and image payloads into canvas-owned values. The source revision and file bytes can then be dropped. Hidden text has a separate visible projection with UTF-16 mappings on both sides of a hidden field; the full source paragraph remains available. This import path does not write a notebook.

```sh
cargo run -p canvas --bin page-probe -- PRIVATE_SECTION_COPY EXACT_PAGE_TITLE > PRIVATE_PAGE_JSON
python3 tools/canvas/compare_page.py PRIVATE_PAGE_JSON NATIVE_PAGE_XML > PRIVATE_COMPARISON_JSON
```

The page probe uses retained outline layout: cumulative indentation, collapsed descendants, adjacent paragraph spacing, minimum line spacing and bullet layout. Title containers retain their IDs, parent geometry and ordered child outlines. Children wrap at 468 pt when no width is stored; each following child starts after the larger of the preceding child’s content height and stored suggested height. The probe emits title parent geometry once in `title_areas`, linked to the flattened outline diagnostics by ID. The comparator identifies outlines by their complete paragraph text and rejects ambiguous matches. `--allow-native-empty-nbsp` explicitly records the observed native conversion of a sole nonbreaking space to an empty paragraph; it does not report those paragraphs as exact text matches.

The PDF companion verifies the matching native XML first, then recovers paragraph wraps from its adjacent PDF export:

```sh
python3 tools/canvas/compare_page_pdf.py PRIVATE_PAGE_JSON NATIVE_PAGE_XML --allow-native-empty-nbsp > PRIVATE_LINE_COMPARISON_JSON
```

Complete-outline text identifies repeated paragraphs. Otherwise, a paragraph needs unique source context; repeated PDF instances must agree on every recovered break. Glyph boxes group visual lines even when bold/regular PDF runs have slightly different baselines. The report retains each instance's original baseline range and print-scale observation. Current source probes match all 159 nonblank body paragraphs across the three references (214 visible lines); 110 whitespace-only paragraphs have no PDF glyph-position evidence. This establishes body wrap agreement for these captures, not screen baselines, empty-line placement or title/tag rendering.

`--include-titles` also compares title/date/time paragraphs against PDF glyphs. All eight such paragraphs in the three references match with Carlito explicitly substituted for Calibri. Both `page-probe` and `render_page` accept repeated `--substitute-font FONT_FILE` arguments after their positional arguments, using the same substitution path as the app. The title comparison screenshot uses the existing image-derived viewport translations; no title-specific translation is fitted. Wrap agreement does not establish title screen baselines or general title alignment behavior.

All three private reference pages pass owned import after the source document is dropped. All 15 body outline heights agree with native XML within 0.002 pt. Album's Courier New bullet height participates in the whole paragraph box; expanding the first text line instead incorrectly adds space to wrapped paragraphs. The importer retains note tags attached to rich-text objects as well as paragraph nodes. These are total-height measurements, not proof of matching line breaks or first baselines. Full reports remain in the external evidence bundle.

```sh
cargo run -p canvas --features gpu --example render_page -- PRIVATE_SECTION_COPY EXACT_PAGE_TITLE NEW_PNG_PATH
```

This comparison renderer draws body text, title text, date/time fields, bullet glyphs, supported tag icons, highlights and images in source object order at 100% / 96 DPI into the frozen viewport size, writing only a new PNG. It normalizes the three measured page margin origins to 36 pt / 14.4 pt. The title container is placed relative to that normalized margin. The optional fourth argument is the physical Y position of the document origin (default zero). Image-region comparison against the frozen viewport finds native translations of +6 px for Video and +30 px for Lore. Those are measured capture transforms, not paragraph-baseline corrections or a general scrolling rule; the native screenshots and geometry reports accompany these images.

## Text and native-reference probes

`corpus/canvas/text-cases.json` is the input shared by the Rust/Parley probe, the Swift/Core Text probe and the synthetic native-page generator. Probe coordinates are logical points, without pixel quantization. Both probes accept additional font-file paths after the JSON path and register those fonts only within the process.

```sh
cargo run -p canvas --bin layout-probe -- corpus/canvas/text-cases.json > PARLEY_JSON
swift tools/canvas/core_text_probe.swift corpus/canvas/text-cases.json > CORE_TEXT_JSON
python3 tools/canvas/native_fixture.py corpus/canvas/text-cases.json NEW_NOTEBOOK_DIRECTORY
python3 tools/native_runner.py NEW_NOTEBOOK_DIRECTORY NEW_CAPTURE_DIRECTORY --author tools/native/pages.ps1 --pdf --screenshots
python3 tools/canvas/compare.py corpus/canvas/text-cases.json PARLEY_JSON CORE_TEXT_JSON NEW_CAPTURE_DIRECTORY/read > COMPARISON_JSON
python3 -m unittest discover -s tools/canvas -p 'test_*.py'
python3 tools/canvas/range_stress.py NEW_STRESS_DIRECTORY target/debug/layout-probe
python3 tools/canvas/range_stress.py NEW_CORE_TEXT_STRESS_DIRECTORY swift tools/canvas/core_text_probe.swift
```

The comparison and its tests require `pdfplumber`. The native runner uses the existing disposable Windows lab. Choose new artifact destinations outside version control. Original notebooks are never capture destinations. Font substitutes can be tested using a derived case file with just the requested font names changed; the comparator records those overrides and rejects changed text, sizes, styles or widths. It also verifies the native capture's input fixture against the canonical cases.

Native PDF ranges are supplementary evidence: the comparator accepts only unambiguous ASCII content and groups characters by intersecting vertical glyph boxes, preserving mixed-style lines without fitting to expected canvas wraps. Unmapped PDF glyphs and ambiguous matches do not produce passing expectations. Screen captures still require a recorded zoom/DPI before becoming the frozen raster reference.

For controlled screen captures, run `native_runner.py` with `--inspect --pdf`. Once it reports the clone is ready for inspection, run `capture_view.py CLONE CAPTURE/read/page-NNN.xml NEW_VIEW_DIRECTORY --scrolls 2` for a long page, or omit the scroll option for a short page. The tool targets OneNote's main frame, fixes 1600×900/100%, records measured DPI and captures overlapping views. The zoom screenshot verifies the visible setting. Creating `CAPTURE/finish` asks the runner to collect final artifacts and delete the clone; the captured view's source XML is then under `before-read`. Inspect the images before accepting them as references.

`compare_scroll.py` measures downward integer scrolling between native captures from identical foreground rows, then scores the overlapping foreground pixels. It reports all candidates and leaves the offset unset when the best score is ambiguous. Blank or unrelated captures cannot establish an offset. The comparison requires unchanged horizontal position and at least 64 pixels of overlap; it does not use canvas geometry. The selected 1600×900 captures use this recorded content crop:

```sh
python3 tools/canvas/compare_scroll.py NATIVE_TOP.png NATIVE_SCROLLED.png --crop 48 83 1434 842
```

Use the measured offset to render the corresponding viewport with `render_page` by subtracting it from the page's top-view translation. Keep top-view registration separate: the Video and Lore comparisons establish their translations from image regions, independently of text layout. Pixel-color bounds alone cannot establish a text baseline when caret, outline-border or background pixels overlap the text region.

The [native baseline fixture](../../corpus/canvas/baseline-anchors.md) adds three opaque image anchors and paragraph-spacing controls. `compare_baselines.py PROBE_JSON NATIVE_XML NATIVE_PDF` matches decoded image pixels, rejects ambiguous or masked images, and reports baseline residuals under independently measured image translations. It preserves wrap mismatches and unregistered pages; no passing tolerance is inferred from the canvas output.

The initial native experiment establishes three useful controls:

- Ordinary Arial outline heights, including mixed sizes, agree with sums of Windows ascent/descent metrics within 0.00004 pt; first-baseline placement is a separate measurement.
- The tested COM imports clamp requested outline widths below 72 pt and remove wholly empty outlines. Those outcomes must not be mistaken for paragraph-engine failures or generalized into an editor-wide width restriction.
- This Mac lacks Calibri: both engines initially resolve it to Helvetica. Resolved faces accompany results so matching a few wrap offsets cannot conceal substitution.

Carlito matches the initial native Calibri wrap cases, including bold. Arimo matches the tested Arial advances, but its Windows metric tables have larger extents than Arial's; blindly selecting those tables would add vertical drift. Its horizontal-header ascent/descent match Arial in these probes. Font substitution therefore needs a measured metric policy as well as matching advances.

The [Parley dependency](../../Cargo.toml) pins upstream source for the experiment. Published 0.11.1 returned a line boundary inside an emoji's UTF-8 encoding in the deterministic stress cases. An isolated two-location correction removed that failure but still lost a leading space in another case; those local changes were not adopted. The unmodified upstream commit in the manifest passes all 500 source-coverage cases, as does Core Text, and retains the measured native ASCII break agreement. This establishes a tested candidate, not whole-canvas compatibility.

Full native captures, candidate font binaries/licenses and probe outputs remain in the external evidence bundle linked from the research session. The current tools are experiments, not the finished editor.

## CPU pipeline profiling

The opt-in release probe measures engine construction, document baseline cloning, editing, undo, the host’s production page primitive collection and accessibility construction. Synthetic cases contain 100, 1,000 and 5,000 paragraphs. Each runs 32 insertion/undo samples alternating between the first and last paragraph, timing the accessibility update after each insertion, undo and a 40-pixel scroll; assertions require one changed layout per insertion, retained layouts through drawing/accessibility, and exact document restoration after undo. The accessibility regression applies incremental updates to an AccessKit consumer tree and compares it with a full rebuild through 160 mixed editing operations and complete undo/redo, including composition, source index changes, resize and viewport transforms.

```sh
cargo test --release -p canvas --features interaction canvas_pipeline_cost -- --ignored --nocapture --test-threads=1
CANVAS_TEST_SECTION=PRIVATE_SECTION_COPY CANVAS_TEST_PAGE=EXACT_PAGE_TITLE CANVAS_TEST_SUBSTITUTE=CARLITO_REGULAR_FILE cargo test --release -p canvas --features interaction canvas_pipeline_cost -- --ignored --nocapture --test-threads=1
```

Private-page mode reads the section once and measures 16 parse/open/close cycles with one retained text engine, recording first construction separately from repeated cycles. Set `CANVAS_PROFILE_CYCLES` to a positive count for longer retention runs. It samples this process’s resident memory through macOS `ps` with each page open and closed. Tab-separated `canvas_profile` records report nanoseconds; `canvas_count` records report object counts; `canvas_rss_kib` records report KiB. The test harness can prefix the first record on a line. Times exclude log output, layout identity scans and memory sampling. Primitive collection includes its temporary allocation and destruction but excludes GPU preparation, submission and presentation. File reads can hit the OS cache. Resident memory includes allocator retention and is neither live allocation accounting nor GPU occupancy; these measurements do not establish input-to-present latency or a process memory limit.

The GPU probe uses a 1386×759 offscreen target and runs 32 samples each for repeated drawing, vertical pan, eight zoom levels, forced eviction, horizontal/vertical offscreen placement, and native page reconstruction. Without private inputs it draws 2,000 synthetic text lines. The first repeated-draw sample includes initial pipeline and atlas work; later samples reuse the renderer. Native reconstruction preserves the text engine and renderer while replacing the decoded scene. Parsing, layout, primitive collection and logging are outside the measured draw interval.

```sh
cargo test --release -p canvas --features gpu renderer_cost -- --ignored --nocapture
CANVAS_TEST_SECTION=PRIVATE_SECTION_COPY CANVAS_TEST_PAGE=EXACT_PAGE_TITLE CANVAS_TEST_SUBSTITUTE=CARLITO_FILE cargo test --release -p canvas --features gpu renderer_cost -- --ignored --nocapture
```

Each `canvas_gpu_sample` row contains phase, sample, draw/submit nanoseconds, elapsed nanoseconds through queue completion, glyph/tag entries, occupied atlas texels, image entries, image texture bytes, vertices, batches, CPU vertex capacity bytes, CPU batch capacity bytes, and glyph/image table capacities. GPU resources are reported at their requested sizes; driver overhead, staging buffers, allocator overhead and the font scaler's internal cache are excluded. Queue completion includes CPU preparation and waiting; it is neither GPU execution time nor input-to-display latency. Each sample waits for the queue, so this experiment does not measure display frame pacing. Assertions enforce the existing entry, vertex and image-byte budgets throughout every phase.

## Native interaction captures

`capture_interaction.py` records one saved AutoHotkey script, a settled screenshot and a subsequent COM page export while `native_runner.py --inspect` holds a disposable clone. It checks that the source page belongs to the supplied target and refuses an existing output directory. Script bytes stay at their original path; the input record includes their hash. Finish and remove the clone through the owning runner after collecting the sequence.

```sh
python3 tools/canvas/capture_interaction.py CLONE NATIVE_RUN/read/page-000.xml ACTION.ahk NEW_OUTPUT_DIRECTORY
```
