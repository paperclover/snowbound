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
read the clipboard, open a link. Everything platform-specific stays on the
host's side of that line, shortcuts included: the page takes editing keys, and
the host's command table runs the chords for formatting, tags, zoom and the
clipboard through the page's own methods.

## Editing emits ops

`CanvasEditor` holds the working page. When an edit changes what's stored, the
editor records the `onestore::op`s the change lowers to at that moment. The
host collects them and hands them to the section as one `Edit`. No page is
ever rebuilt or diffed to be saved, so the work of saving a keystroke follows
the size of the edit, not the size of the page.

Undo lives in the editor, not in storage. Each history entry keeps the inverse
of the change it made. Undoing applies that inverse and emits its ops as a new
edit, so the file only ever moves forward, like OneNote's. Undo steps are
OneNote 2010's too: typing and backspaces at one caret are a single step until
the caret moves or another kind of edit comes between, though each keystroke's
ops still reach storage as it is typed. Undoing a deletion
brings back the original identities, so internal links to those paragraphs
survive an undo.

Each page keeps its history while the window is open, as in OneNote 2010: the
desktop app parks a page's editor when it is left and takes it up again, through
the same reload in place, when the page opens. Between those edits the app keeps
what was done to pages and sections (new, deleted, moved and indented pages, page
and section names, sections moved), and Undo takes back whichever came last: the
open page's own step, or the last such action in its section or notebook,
showing the page it changes. Unlike OneNote, an edit doesn't end what actions Undo
can reach, so New Page, a typed title, Undo, Undo takes the title and then the
page. Each step back is computed against the section as it is then, one edit of
its own; a step whose page is gone, or holds work it didn't make, is passed over.

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
  width. Dragging snaps to its grid, anchored at the page's margin origin, unless
  Snap To Grid (the shape gallery's last item, kept between launches) is off. Tags
  sit in a column to the left of the text with OneNote's spacing, and a tag's
  colour paints the whole paragraph as it does there.
- **A right-to-left page is the same frame, seen from its right.** OneNote 2010
  keeps its positions left to right like any page's, from a margin origin some
  10,800 pt out, and opens it scrolled to the right end of its content, keeping
  that edge as the window resizes. The canvas does the same and stores clicks,
  strokes and drags as on any page (`corpus/rtl-page`).

These rules were established against OneNote's own output: its XML export
gives outline sizes, its PDF export gives exact line breaks, and screenshots
give placement. The comparators in `tools/canvas` (with the probes
`layout-probe` and `page-probe`) keep checking them.

Equations draw in two dimensions from the tree `onestore::page::Math` parses,
each in a space kept in its line of text, so text and links around them keep
their look and the line grows to hold them. They are edited as OneNote's equation editor edits them: Alt+= starts one,
typing is its linear format (UnicodeMath), and a space builds up what it ends.
Linear and Professional switch an equation between the forms, and OneNote
stores both. Links follow OneNote too: a typed URL links itself when a space or
Enter ends it, the Link dialog stores its address in a hidden field code before
the label, and a click or Enter opens a link. URL text shows as a link, as
OneNote links it when it opens a page, without being stored as one. Ink draws stroke by stroke in page
coordinates. Page templates' background art is recreated as vector art and
recognised by the stored picture's hash. OneNote's bitmaps aren't shipped.

## Attached files

A file sits in an outline's flow as OneNote draws it: its icon over its name,
extension hidden, in a 54-point column. Attaching splits the caret's paragraph
around the file, and the text after the caret follows it with the caret, as
OneNote 2010 attaches and drops files. Every new file stores the icon OneNote
stores beside it, because OneNote draws a broken picture for a file without
one: the host's system icon where it has one, the canvas's blank page
otherwise. A double click or the context menu's Open asks the host to open a
copy; Save As writes the bytes where the user chooses.

Attached or dropped where a click on blank page left the caret, the file goes
on the page itself, as OneNote 2010 places it: the same column at the caret's
grid point, outside any outline. It selects, drags on the grid, deletes and
moves with Insert Space as a picture does, and opens and saves as above.

A tag on a picture or file is stored on the object itself, as OneNote 2010 stores
the tag Ctrl+1 gives a selected one, and drawn in a column left of it, centred on
it; a click checks its box (`corpus/object-tags`). A picture's link follows on
Ctrl+click, Command+click on macOS, while a click selects it, as OneNote's
tooltip says (`corpus/picture-link`).

OneNote 2010's Ctrl+1 leaves a bulleted or numbered paragraph's list beside its
new To Do box, and it has no command that converts a list. Make To-Do List, in
the page's context menu and the palette, trades the selected paragraphs' bullets
and numbers for the first check box tag in the user's list as one undo step;
Make Bulleted List trades the tag back for a bullet (`corpus/to-do-list`).

An outline holding only pictures or files takes a paragraph after them where a
click beside them lands, as OneNote 2010 adds one when typing there. It is
stored with the first edit that reaches it, and undoing back to it empty takes it
out again (`corpus/object-outline`).

A file printout's pages are pictures whose stored data is the printout's XPS
package. The page draws the PNG OneNote rendered of each
(WebPictureContainer14), framed in grey as OneNote frames them, and moves them as
pictures. A printout page deleted and brought back by undo returns as the page it
was: its XPS package, the PNG and the properties tying it to the printout, written
back as read (`corpus/printout`).

Pictures go in as OneNote 2010 pastes and inserts them. At a caret in text the
paragraph splits around the picture, as around a file. On blank page the picture
lies on the page at the caret, and the caret moves to the grid row below it.
From the title it joins the outline where the body starts, or lies two grid rows
below the page's content when none starts there. A picture takes the size its
resolution gives it, or 96 dpi without one. Paste takes files first (a picture
file as its picture, as a dropped one goes in), then Snowbound's own copy, then
a web page, its formatted text, lists, tables and pictures in order as one undo
step, then text, then a picture: Finder offers a copied file's name and icon
beside it, and other apps a picture of copied text.

Copy offers what OneNote 2010 does beside text, HTML with each run's formatting
inline, lists as `ul` and `ol` and tables bordered, which OneNote and Word paste
as copied (`corpus/clipboard`). Snowbound's own format, a `Clip` of paragraphs
and the definitions they name as JSON, carries styles, tags and pictures too.
Several paragraphs go between the halves of the caret's paragraph, an empty
half dropped; a title takes the text alone.

## Recordings

Record Audio and Record Video work as OneNote 2010's do. The caret's paragraph
splits around a line saying when recording started, in OneNote's grey `cite`
style, and the caret goes on below it. Text written while recording links to
the moment it was written; text written while paused links to nothing, and the
pause is left out of later moments. On Stop, the file goes in above that line,
named after the page. Hovering a linked note or a recording shows OneNote's blue
play button in the margin, and a click asks the host to play from five seconds
before that moment, OneNote's default rewind. While a recording plays, See
Playback highlights the note linked last at or before the moment playing,
scrolling it into view when it changes.

The host records audio as 16 kHz WAV and stores it as IMA ADPCM, and video as
AVI of Motion JPEG at 320 by 240 and 15 pictures a second with PCM sound, both
of which OneNote plays (`corpus/recording/video`); `canvas::recording` makes both,
so every host stores the same bytes. GStreamer writes that AVI
directly; on macOS a capture session records a movie that `AVAssetReader` reads
back into it off the main thread after Stop, the transport saying so and the page
editable meanwhile. Mac OS X 10.6, without AVFoundation, opens recordings in the
system's player and does not record. Snowbound plays both itself: the sound through the platform's
player, decoded from IMA ADPCM first, and a video's pictures in the transport
over the page, which also carries OneNote's Pause, Stop, ten-second and
ten-minute skips, Seek To and See Playback.

## Drawing

The Draw tab's tools work as OneNote 2010's do with a mouse (`corpus/ink-tools`).
Every stroke and every shape is a drawing of its own, added on top of the page
when the pen lifts: one edit and one undo step, published as one appended
revision, or in one with the strokes queued behind it during a sync round trip,
as keystrokes are. Shapes are ink too. Their corners snap to the placement grid, and the
drawing keeps the shape's kind and anchors beside its strokes, which OneNote
edits it by. A highlighter's rectangular tip multiplies what lies beneath, so
text under it stays dark. The stroke eraser takes whole drawings, or only the
strokes it touches in an older drawing of several. The lasso picks the drawings
with most of their points inside it. They move by their offset, as OneNote
moves ink, and Delete removes them. Escape returns to Select & Type, where a
click on ink picks it. The default pen draws in the open section's accent at
the light theme's shade, so a stroke stores one real colour and shows it in both
themes.

A pen that reports pressure (a tablet on macOS, the Apple Pencil) draws and stores it as
OneNote 2010 does with pressure sensitivity on: each point's width is the pen's times
0.25 plus 1.5 times the pressure, and the stroke keeps NormalPressure beside X and Y
(`corpus/ink-pressure`). A mouse, trackpad or finger draws at the pen's width, and so
does every pen with OneNote's "Use pen pressure sensitivity" turned off (Options >
Advanced on the desktop, the pen's colour menu on iOS; on by default). winit reports no
tablet pressure on Linux.

## Tables and selections across them

Columns size as OneNote 2010 sizes them (`corpus/table-widths`). An unlocked
column fits its widest line plus 4.347 pt, never under a new column's 37.11 pt,
widening and narrowing with each edit, which stores the width in the same
revision as its text. A table stops at the outline's width and its cells wrap
from there. Dragging a column's right border resizes that column alone, the
columns after it moving with it, down to 37.11 pt; on release it is one edit
and one undo step, and the column is locked, so typing no longer fits it.

Note tags on a table sit in the tag column centred on it, and a click checks its
box (`corpus/table-tags`). A selection crossing a table's edge deletes as
OneNote 2010 deletes one: nothing joins across the edge, cells inside are
emptied, and where the selection runs on past the table its rows inside go, the
table with them when all do. What replaces the selection goes in at its start
(`corpus/cross-container`). Tab and Link leave such a selection alone, and
Shift+Enter a caret inside a link, as OneNote's do; Alt+= across paragraphs
makes each paragraph's part an equation, and a deletion between two equations
joins them where neither seam lies inside an object (`corpus/equation-join`).
An edit replacing a selection with an end inside an equation's object (a
fraction, a script, a root) takes the whole object first, as OneNote's equation
editor selects, and across paragraphs an object so taken at its paragraph's end
takes that end too; a placeholder ("Type equation here.") is taken whole
(`corpus/equation-select`).

## Styles and themes

The Styles gallery is OneNote 2010's: Heading 1 to 6, Page Title, Citation, Quote, Code
and Normal, stored under OneNote's names (`h1`, `PageTitle`, `cite`, `blockquote`, `code`,
`p`). Applying one gives the paragraph the page's style object of that definition and
clears its character formatting but links, fields and language, as OneNote's does; Enter
at a paragraph's end takes its style's NextStyle, so a heading is followed by Normal.
Ctrl+Alt+1 to 6 apply the headings, and Clear Formatting at a caret applies Normal.

A theme gives the eleven styles a look, and a page wears it as its style objects: OneNote
draws a page by its own style objects, so OneNote users see the theme under the same
names. Changing a theme moves each styled paragraph to a new style object of the same name
(style objects are read-only) in one revision per page, and a page is brought to its
theme when it opens, so a notebook-wide change costs each page only when someone looks at
it. Which theme a notebook, section or page wears lives in the notebook's `.snowbound`
folder, beside the tags' art (`corpus/styles`).

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
is listening, and never for a caret blink. The interface's own tree holds it at
the page's box (see [the interface kit](ui.md#accessibility-and-the-keyboard)).

## Spelling

Spelling is checked as OneNote 2010 checks it with its default proofing
options. Words in capitals, words with digits, and Internet and file addresses
go unchecked, and a word repeating the one before it is marked as repeated.
Each run is checked in the language it stores, so a French paragraph takes the
French dictionary and an equation none. The host supplies the dictionary;
`canvas::spelling` checks each paragraph on a thread of its own when it first
comes into view, and keeps the result by the paragraph's text, so drawing never
waits on it. Marked words carry OneNote's red zigzag, one pixel thick at any
zoom and laid on the device's pixel grid so a 1× screen shows OneNote's exact
pixels, except the word still being typed. The context menu (the edit menu on
iOS) offers the dictionary's corrections, Ignore and Add to Dictionary, and the
Spelling pane (F7) walks the page's marked words. A correction is an ordinary
edit, one revision. Nothing about spelling is stored in the page.

## Printing and PDF

`canvas::print` lays pages on paper as OneNote 2010 prints them (`corpus/print`): between
half-inch top and bottom margins with the margin origin an inch in, the whole page shrunk
when its content reaches past the paper's right edge, and each sheet after the first
starting at the line of text or picture the one before would have cut. Rule lines and
template art print across the paper; the page colour does not. The footer names the
section and numbers the sheets. `draw::pdf` writes the same primitives the screen paints
as PDF: text in subset fonts whose ToUnicode maps come from the laid-out text, so it
selects and searches, ligatures included; ink and shapes as paths; pictures and tag
icons as images. On the desktop, Print and Export as PDF first show OneNote's Print Preview
and Settings, less the preview: the range (page, page group, section, and for a PDF the
notebook), paper, orientation, fitting to the paper's width and the footer. Print then hands
the PDF to AppKit's print panel, the XDG print portal or the shell's print verb for PDFs.
On iOS the page menu prints the page through the print sheet or shares its PDF.

## Search, dates, conflicts

Smaller modules follow the same pattern of reproducing OneNote's behaviour
precisely. `search` matches the way OneNote 2010 searches: word prefixes,
ignoring case and diacritics, title matches first, and the text OneNote recognised
in pictures, which it stores in them, as OneNote's search finds it. `date` edits the title's
date and time fields the way OneNote stores a changed page date. `conflict`
shows conflict pages with OneNote's highlight.
