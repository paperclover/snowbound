# Platforms

Snowbound is one core with thin hosts around it. The file format, sync, the
page editor and the renderer are identical everywhere. What changes per
platform is the window, the input plumbing, and how much of the interface is
drawn by Snowbound versus the operating system.

```text
                 macOS                 Linux                  iOS
shell            snowbound + ui        snowbound + ui         UIKit (apps/ios, Swift)
glue             snowbound/src/macos   snowbound/src/linux    crates/mobile (C ABI, staticlib)
page             canvas ─────────────────────────────────────────────────────►
paint            draw (wgpu: Metal)    draw (Vulkan or GL)    draw (Metal, CAMetalLayer)
data             notebook::session + embedded SMB client ────────────────────►
format           onestore ───────────────────────────────────────────────────►
```

## Desktop: `snowbound`

The desktop app is a winit window with the `ui` kit's chrome and a `canvas`
page inside it. `snowbound` picks a platform module at compile time
(`macos.rs` or `linux.rs`, both mounted as `platform`), and everything else
in the crate is shared: library and settings, the sidebar, menus, page and
section management, templates, and screenshot and replay support.
Accessibility for the page goes through AccessKit's winit adapter on both.
`commands.rs` is the one table of commands: each one's title, its chords on
macOS and elsewhere, when it is enabled or checked, and what it does. The
keyboard, the toolbar and the macOS menu bar all run commands from it.

### macOS

- The toolbar's row is the title bar. An empty `NSToolbar`, unified compact
  from macOS 11, with the title hidden makes the title bar the row's height,
  and AppKit places the traffic lights in it; a press in the row's gaps drags
  the window. The title is still set, for the Window menu, Mission Control and
  VoiceOver. `--screenshot` paints the lights where the hidden window's AppKit
  put them. The app draws the row as part of the same frame as the rest of the
  chrome. Under the whole
  window lies AppKit's title bar material, an `NSVisualEffectView`, and the
  chrome is drawn transparent over it, so the title bar, toolbar, tab row and
  sidebar take the system's desktop tint, appearance and focus state exactly.
- AppKit supplies the open and save panels, alerts, the date picker, date
  formatting for new page titles and conflict labels, the account's full name
  (used as the author, as OneNote uses Office's user name), and the caret and
  selection colours.
- Frames present inside Core Animation's transaction, so a resize pairs each
  frame with the window's new size instead of stretching the last one.
- A notebook on a mounted SMB share is opened through the embedded SMB client,
  signed in with the password the keychain keeps for that mount (see
  [sync](sync.md) for why the mount itself isn't enough).
- The menu bar (`menubar.rs`) is laid out as OneNote for Mac's. Its items are
  the command table's, validated from the statuses each frame publishes, and
  their key equivalents are the table's chords, so AppKit takes a chord the
  menu enables before winit sees the key. Linux has no menu bar.
- `tools/canvas/build_macos.py` builds and ad-hoc signs `target/Snowbound.app`.

### Linux

- X11 and Wayland. Title bars come from the window manager, or on Wayland from
  the compositor where it offers server-side decorations, and the toolbar's
  row lies beneath them, as in Dolphin and Kate. Elsewhere on Wayland, as on
  GNOME, the row is the title bar, as a GTK 4 header bar holding tools: winit's
  Adwaita frame keeps its shadow, corners and resize edges but has no header,
  and the row ends in the window's buttons in the order GNOME's `button-layout`
  lists them. Under that frame on GNOME the window erases its bottom
  corners to transparent pixels and the frame draws libadwaita's window edge
  round it: radius, shadows and outline. KWin rounds Breeze's corners itself.
- The toolbar and the rest of the chrome continue the title bar's fill,
  focused and not: on KDE the colour scheme's header colours from
  `kdeglobals`, as KWin paints its title bars, and on GNOME winit's Adwaita
  frame's. A settings portal signal re-reads them when the scheme changes.
- Menus and other popups take the desktop's look and motion: libadwaita's
  popover menus on GNOME, shown and hidden at once as GTK 4 does, and Breeze's
  on KDE, faded as KWin fades popups and scaled by Plasma's animation speed.
  The desktop is read once from `XDG_CURRENT_DESKTOP`; elsewhere the kit's own.
- zenity or kdialog provide the pickers and alerts. The XDG settings portal
  provides the colour scheme. Text conventions come from the C library's
  locale. Fontconfig is loaded at run time, so builds need no headers for it.
- Wayland's clipboard goes through the window's own connection, since not every
  compositor offers a clipboard to clients without a window.
- `crates/snowbound/linux` packages a tarball, plus an installer that adds the
  launcher entry the Wayland desktop needs to find the icon.

### Windows and older macOS

The readme names both as goals. Nothing platform-specific exists for them yet.
Keeping `ui` and `draw` free of platform toolkits is what keeps them within
reach.
The chrome's fill already has its seam: `platform::install_backdrop` lays a
system material under a transparent surface, as Mica or Aero glass would,
and `platform::titlebar` names opaque fills where the system has those.

## iOS: native around the canvas

On iOS the split moves. Touch text editing depends on affordances that users
know by feel and that are expensive to imitate: the loupe, selection handles,
the edit menu, autocorrect, dictation and hardware keyboard commands. So UIKit
owns everything around the page:

- **UIKit** handles navigation (a three-column split view of notebooks, pages
  and the page), scrolling and zoom (`UIScrollView`), the keyboard and text
  input (`UITextInput`), caret, selection highlight and handles, the format
  bar, and connecting to servers.
- **`canvas`** draws the page into a `CAMetalLayer` and makes every editing
  decision. The C surface exposes the active outline's text as the flat UTF-16
  model `UITextInput` speaks. That is the same unit `onestore` ops measure
  text in, so autocorrect and dictation become ordinary text ops with no
  translation layer.
- **`crates/mobile`** is that surface: a static library with a C header,
  covering libraries, shares, sections and views, built by an Xcode build
  phase (`apps/ios/build-rust.sh`). All views share one GPU device and one
  renderer, and text engines are pooled across views.

No `ui` crate ships on iOS. Everything below the view is the desktop's code:
`notebook::session` with its replica and background publishing, the embedded
SMB client (credentials kept in the Keychain, as Files keeps its own), conflict
pages, search, and page management through the same ops. Notebooks from Files
are read and written under `NSFileCoordinator`, so file providers see every
change.

## What stays shared, on purpose

- **Behaviour**: the editor, hit-testing, the placement grid, undo, conflicts
  and search all live in `canvas` or `notebook`. A host that reimplements one
  of them has made a bug.
- **Storage**: every platform writes through the same ops and the same
  replica, and publishes through the same sync step.
- **Look of the page**: the page renders through `draw` everywhere, so a page
  looks the same on a phone as on the desktop.
