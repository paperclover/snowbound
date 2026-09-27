# The page: editor and canvas

A OneNote page is a free canvas, but it doesn't feel like a drawing app. You
click anywhere and type, and a text box (an *outline*) appears and grows as you
write. Outlines wrap, lists indent, tags hang in the margin. `canvas` is the
crate that makes a page behave that way and look the way OneNote 2010 draws
it. It depends on `onestore`'s page model and on `draw`. It knows nothing
about storage, windows or the interface kit, which is why the same page runs
inside the desktop app and inside a UIKit view on iOS.

## Layers

```text
onestore::page::Page       stored identities, formatting, unknown content kept aside
        │
canvas::document           text outlines as paragraphs of rich text, UTF-16 positions
canvas::editor             CanvasEditor: edits, selection, composition, undo, the ops each edit lowers to
canvas::layout, outline    shaping (parley), Windows line metrics, outline and table geometry
        │
canvas::gpu::page          PageScene: the page as draw primitives; pictures decoded off-thread
canvas::interaction        PageView: hit layers, drags, grid, handles, key routing, scroll, zoom, AccessKit
        │
host                       translates platform events in; carries out the Requests that come back
```

A host feeds `PageView` pointer, key and text-input events. It gets back a
`Response` saying whether the page, the selection or only the view changed,
plus the occasional `Request` the page can't do itself: show a date picker,
read the clipboard, open the character palette. Everything platform-specific
stays on the host's side of that line.

## Editing emits ops

`CanvasEditor` holds the working page. When an edit changes what's stored, the
editor records the `onestore::op`s the change lowers to at that moment. The
host collects them and hands them to the section as one `Edit`. No page is
ever rebuilt or diffed to be saved, so the work of saving a keystroke follows
the size of the edit, not the size of the page.

Undo lives in the editor, not in storage. Each history entry keeps the inverse
of the change it made. Undoing applies that inverse and emits its ops as a new
edit, so the file only ever moves forward, like OneNote's. Undoing a deletion
brings back the original identities, so internal links to those paragraphs
survive an undo.

Input-method composition (marked text) stays inside the editor until it
commits, and only then becomes ops. When another client changes the open page,
the editor compares the stored page with what it last read plus the ops it has
handed out, and reloads in place.

## OneNote-faithful geometry

The goal is that a page looks the same in Snowbound as in OneNote: same wraps,
same line heights, same places. That took measuring, not guessing.

- **Stored geometry is not laid-out geometry.** An outline's stored width and
  height are constraints and hints. The displayed box comes from layout, so the
  canvas lays out as OneNote does and never treats a stored size as a clip.
- **Line boxes use Windows metrics.** OneNote measures lines with a font's
  Windows ascent and descent, not the typographic or horizontal-header values
  most text stacks pick. The difference is small per line and large per page.
  Drawing and hit-testing share these line boxes.
- **Substitutes match metrics.** Where the system lacks Calibri, Arial, Times
  New Roman or Courier New, bundled metric-compatible substitutes (Carlito,
  Arimo, Tinos, Cousine) stand in, under the stored font name.
- **OneNote's constants are OneNote's.** New outlines take OneNote's default
  width. Dragging snaps to its grid, anchored at the page's margin origin. Tags
  sit in a column to the left of the text with OneNote's spacing, and a tag's
  colour paints the whole paragraph as it does there.

These rules were established against OneNote's own output: its XML export
gives outline sizes, its PDF export gives exact line breaks, and screenshots
give placement. The comparators in `tools/canvas` (with the probes
`layout-probe` and `page-probe`) keep checking them.

Equations draw in two dimensions from the tree `onestore::page::Math` parses.
The linear text stays the editable source. Ink draws stroke by stroke in page
coordinates. Page templates' background art is recreated as vector art and
recognised by the stored picture's hash. OneNote's bitmaps aren't shipped.

## Content the editor doesn't understand

Nothing is lost for being unfamiliar. A paragraph the canvas can't draw
becomes a labelled placeholder that keeps its place in the flow, and the rest
of its outline stays editable. An object the editor can't hold draws as stored
and stays read-only. Its bytes are never touched either way. Wherever a
remaining "can't edit this" state exists, the aim is to remove it by teaching
the editor the structure, not by flattening the content.

## Pictures and memory

The page scene reads only picture headers when it's built. Pictures in and
near the view decode on a background thread at the size they're shown, within
a fixed memory budget, and pictures long out of view are let go. No number or
size of pictures can make a page fail to open. Opening a page happens on its
own thread too. The current page stays live until the new one is laid out and
the pictures it shows first are ready.

## Accessibility

The page builds an AccessKit tree. Each text outline appears as its own
editable text area, whose runs carry the canvas's real line boxes and
character positions, so a screen reader's caret and selection land where the
eye does. Native selection and replacement actions go through the editor's
history like any other edit. The tree updates only while an assistive client
is listening, and never for a caret blink. The interface kit around the page
doesn't have a tree yet.

## Search, dates, conflicts

Smaller modules follow the same pattern of reproducing OneNote's behaviour
precisely. `search` matches the way OneNote 2010 searches: word prefixes,
ignoring case and diacritics, title matches first. `date` edits the title's
date and time fields the way OneNote stores a changed page date. `conflict`
shows conflict pages with OneNote's highlight.
