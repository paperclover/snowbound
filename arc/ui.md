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
one joins the page with rounded inside corners. It is OneNote 2010's layout,
drawn with soft shadows, concentric corner radii and a dark appearance. Call
it half-skeuomorphic: the shapes that made OneNote's notebook metaphor
legible, without the 2010 chrome.

The motion comes from [File Pilot](https://filepilot.tech): things move
quickly and never feel like they're waiting on an animation. Animated values
ease exponentially toward their targets with a short half-life. That is
frame-rate independent, a retargeted animation continues smoothly from where
it is, and it settles fast. Popups open and close on short timed curves,
slow enough to follow: a menu grows out of the pointer or its button as it
fades in, a combo's field widens into its list, and a dialog swings up into
place over a dimmed window, as Windows opens a window. On GNOME and KDE, menus instead
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
native. That means file pickers, alerts and date pickers (AppKit on macOS,
zenity or kdialog on Linux), the caret and selection colours, each platform's
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
                  overflow is shared out by each box's strictness
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
  siblings overflow, each gives up space according to its *strictness*. That
  one knob covers most of what flexbox is usually needed for.
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

## `draw`: the renderer

`draw` is a small wgpu renderer. A frame is a list of layers, each with its
own transform and clip. A layer is a flat sequence of primitives: glyph runs,
SVG icons (tinted or in their own colours), SVG paths filled or stroked,
rounded or gradient rectangles, pen strokes and raster images.

- **Glyphs** are rasterized with Swash into an atlas, at quarter-pixel phases,
  so text placed at fractional positions stays crisp and doesn't shimmer as it
  moves. Icons and paths rasterize once per size and phase into the same
  atlas.
- **The renderer never measures text.** Text reaches it through a trait that
  visits positioned glyph runs. Line metrics stay with whoever laid the text
  out, which is how the page keeps its Windows metrics while the chrome uses
  ordinary ones.
- **Batches** merge consecutive primitives that share a texture and clip,
  without reordering paint.
- **Caches are bounded.** The glyph atlas and image cache have fixed budgets.
  Eviction keeps everything the current frame needs and rebuilds after
  pressure. Images are filtered in linear light with premultiplied alpha.

GPU readback tests pin the rasterization down: successive quarter-pixel
translations have to move the ink's centroid in quarter-pixel steps, and
repaint, cache eviction and a new renderer must all produce identical pixels.

## Testing an interface you can't click

A covered window gets no redraws, so interaction is scripted.
`SNOWBOUND_REPLAY` feeds the app a file of pointer, key, wait and snapshot
steps. `--screenshot` draws the whole window offscreen in each appearance
(the README's screenshots are made this way). `ui`'s own tests build frames
headlessly and assert on layout, routing and signals.
