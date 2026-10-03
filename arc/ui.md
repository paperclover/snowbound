# The interface kit and the renderer

Snowbound draws its own interface. The section tabs, toolbar, page tabs,
sidebar, menus and command palette are all painted by the app through one GPU
renderer, the same one that paints the page. This essay explains why, and how
the `ui` and `draw` crates are built to make that pleasant rather than
heroic.

## The look: half OneNote, half now

The shell is a deliberate homage. Section tabs lean over one another at 45°
with a faint highlight along their tops. The open tab lies on top, in its
section's colour, and frames the page. That colour shades gently down the
window and becomes the accent for the whole interface, easing to the next
section's colour when you switch. Page tabs sit on the right, and the open
one joins the page with rounded inside corners; OneNote's Display options move
them to the left and the notebooks to the right. It is OneNote 2010's layout,
drawn with soft shadows, concentric corner radii and a dark appearance. Call
it half-skeuomorphic: the shapes that made OneNote's notebook metaphor
legible, without the 2010 chrome.

The motion comes from [File Pilot](https://filepilot.tech): things move
quickly and never feel like they're waiting on an animation. Animated values
ease exponentially toward their targets with a short half-life. That is
frame-rate independent, a retargeted animation continues smoothly from where
it is, and it settles fast. Popups open and close on short timed curves,
slow enough to follow: a menu swings out of the pointer or its button as it
fades in, a combo's field widens into its list, and a dialog swings up into
place near the window's top over a dimmed window, as Windows opens a window; the
command palette swings in the same way, undimmed. A filtered list shows its new
results at once. On GNOME and KDE, menus instead
look and move as the desktop's own (see [platforms](platforms.md)).

## Why not native widgets

- **The page has to be custom-drawn anyway.** OneNote-faithful layout needs
  Windows line metrics, OneNote's grid, and its tag and list geometry. No
  platform text view does that, so a renderer and a text-editing core exist
  regardless. Drawing the chrome with them costs little more.
- **One renderer, one pass.** The page and the chrome share a glyph atlas, an
  image cache and a frame. A text field in the toolbar edits with exactly the
  keys, chords, click counts and caret movement the page uses, because both go
  through `draw::edit`.
- **The shell is not a platform idiom.** Leaning section tabs and a
  colour-framed page don't exist in AppKit or GTK. The app would be fighting
  its toolkit.
- **Portability.** The same interface runs on macOS and Linux today, and the
  project aims further (Windows, and old versions of OS X).

The platform still owns what it's best at and what people expect to be
native. That means file pickers, alerts and date pickers (AppKit's sheets on
macOS; the portal on Linux, which otherwise asks with the kit's own), none of
which blocks the window, the caret and selection colours, each platform's
editing chords, the keychain, the traffic lights and window frames. On iOS the
split goes further (see [platforms](platforms.md)).

## Immediate mode, with memory

`ui` is an immediate-mode kit in the sense Casey Muratori and Ryan Fleury use
the term. The *API* is immediate: every frame, builder code declares boxes
from application state, and there's no widget tree to keep in sync with the
model. The *implementation* remembers plenty:

```text
frame N
  route input ─── against frame N-1's layout (hover, press, focus, drags, wheel)
  build ───────── app code declares boxes; each reads its Signal (clicked, dragging, events…)
  solve layout ── per axis: pixels · label size · fraction of an ancestor · sum of children
                  overflow taken from space, then by folding groups, then by strictness
  paint ───────── Layers: primitives under a clip, or a Custom box the host paints
  after paint ─── work the frame asked for (page requests, saving) runs, then asks for a frame
```

- **Identity** is a hash of the parent's id and a part the builder chooses. A
  cache keyed by those ids keeps hover, press, focus, scroll and animation
  state between frames.
- **Input is answered one layout late.** Events route against the previous
  frame's boxes before building, so a box can read what happened to it while
  it is being declared. It sounds odd and is invisible in practice.
- **Layout is solved after building**, per axis. A box can be sized in fixed
  pixels, by its label, as a fraction of an ancestor, or by its children. When
  siblings overflow, space sized from an ancestor gives way first. Then a row
  *folds* its groups, boxes with a full and a folded form (`Spec::fold`), by
  priority, reaching into its boxes sized by their children, so a group can
  fold inside another. Only then do boxes give up room by their *strictness*, the least
  strict first. That one knob covers most of what flexbox is usually needed
  for. A box filling across a parent sized by its children stretches to what
  its siblings make it, and a popup that isn't strict gives way to the window,
  so a dialog is as tall as its contents up to the window and scrolls within.
- **The toolbar is one row that never overflows.** Both forms of every group
  are built each frame and the solver picks, so a group folds in the frame the
  window narrows, with no widths remembered or worked out by the app. A folded
  group is a dropdown of the same command-table rows, and a gallery it lists
  opens beside its row as a submenu, on hover, Right or a click. The
  section tabs scroll sideways past the row's room, fading out where cut.
- **Nothing runs while nothing changes.** The kit reports whether it wants
  another frame (queued input, an animation still settling) and when a timed
  change such as a caret blink is due. The app sleeps in between.
- **Lists are virtual for free.** `ui::list` builds only the rows in view from
  the source data, and holds its place on the selection as items arrive and
  leave above it. No separate model sits between the data and the rows.

The page is a **custom box**. `ui` routes it the pointer, wheel, key and
input-method events that land on it, in order. The host hands them to
`canvas`'s `PageView` and paints the page into a clipped layer of the same
frame. The page's scrollbars are ordinary `ui` widgets; the canvas only
reports its scroll bounds.

`ui::popup` builds menus, filterable lists, a colour grid, galleries and a fuzzy command
palette on a popup layer that takes input above everything else. `ui::shell`
has the OneNote-specific controls: section tabs and compact toolbar buttons.
The kit doesn't know what a notebook is. `snowbound` assembles the window from
these parts.

## Accessibility and the keyboard

The interface presents itself to assistive technology as an AccessKit tree
built from the same boxes as the frame. A box with a `Spec::role` is a node;
the kit's widgets choose their own, and `Ui::access` adds states, values and
names to any box. A box without a role lets its children through, and its
text reads as a label.

- **Names come from what a sighted user reads.** A control without one is
  named by its text and its children's, or by its tooltip: the title names
  it, the chord becomes its shortcut, and the description its description.
  An icon button's command-table title is its name, and a split button's arrow
  is that name with "Options", so no two toolbar controls share one.
- **Popups are menus and dialogs.** A menu's highlighted row is the focus
  within it, so arrows read as they move; a palette is a dialog whose field
  keeps the focus while its list's selection moves. Opening a popup moves the
  focus into it, and closing it gives the focus back. A command menu opens at
  its top with nothing highlighted; a value picker (fonts, sizes, a combo's
  list) opens on its current value.
- **Actions are input.** A press or a new value from assistive technology
  arrives as an event and is answered next frame, as a click would be.
- **Only changes are sent**, and only while something listens.

The page keeps its own tree (see [canvas](canvas.md#accessibility)). The
host grafts it at the page's box, sending the interface's tree first, since
it holds the graft, then the page's.

```text
Window
├─ Toolbar         buttons named by their tooltips, combos with their values
├─ TabList         section tabs
├─ Group "Page"    the page's own tree, grafted
├─ TabList         page tabs
└─ Menu | Dialog   open popups
```

The keyboard reaches the same controls. Tab steps through them, within an
open popup or across the window; a toolbar, tab list or tree is one step,
entered at its selected control, and arrows move within it. F6 steps between
those groups and the page, as Windows and GTK step between panes, and on
macOS Control-F5 goes to the toolbar. Space or Enter presses, Escape closes a
popup or returns the focus to where the keyboard took it from, and the
focused control wears a ring until the pointer is used. The page keeps Tab
for itself, so F6 leaves it.

## `draw`: the renderer

`draw` is a small wgpu renderer. A frame is a list of layers, each with its
own transform and clip. A layer is a flat sequence of primitives: glyph runs,
SVG icons (tinted or in their own colours), SVG paths filled or stroked,
rounded or gradient rectangles, pen strokes and raster images.

- **Glyphs** are rasterized with Swash into an atlas, at quarter-pixel phases,
  so text placed at fractional positions stays crisp and doesn't shimmer as it
  moves. Icons and paths rasterize once per size and phase into the same
  atlas. A rounded box's soft shadow is worked out in the shader instead, so
  popups of any size, widening or not, cost the atlas nothing.
- **The renderer never measures text.** Text reaches it through a trait that
  visits positioned glyph runs. Line metrics stay with whoever laid the text
  out, which is how the page keeps its Windows metrics while the chrome uses
  ordinary ones.
- **Batches** merge consecutive primitives that share a texture and clip,
  without reordering paint.
- **A popup in motion is one picture.** Layers sharing a motion draw
  offscreen together, then fade and lean back as a whole, so nothing beneath
  or within shows through a row. A popup's contents clip to its rounded
  outline, and one opening over a combo lays its rows out at their final
  width while only the outline widens.
- **Caches are bounded.** The image cache has a fixed budget. A full glyph
  atlas is cleared and rebuilt with only what the current frame needs; where
  one frame alone needs more than half of it, it doubles, up to what the GPU
  allows. Images are filtered in linear light with premultiplied alpha.

GPU readback tests pin the rasterization down: successive quarter-pixel
translations have to move the ink's centroid in quarter-pixel steps, and
repaint, cache eviction and a new renderer must all produce identical pixels.

## Testing an interface you can't click

A covered window gets no redraws, so interaction is scripted.
`SNOWBOUND_REPLAY` feeds the app a file of pointer, key, wait and snapshot
steps, and `accessibility` steps that write the whole window's tree as text, so a
hidden window's tree can be checked without a screen reader. `--screenshot` draws the whole window offscreen in each appearance
(the README's screenshots are made this way). `ui`'s own tests build frames
headlessly and assert on layout, routing and signals.
