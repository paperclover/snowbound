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

### macOS

- The title bar is transparent. The app draws its own around AppKit's traffic
  lights, as part of the same frame as the rest of the chrome. Under the whole
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
- `tools/canvas/build_macos.py` builds and ad-hoc signs `target/Snowbound.app`.

### Linux

- X11 and Wayland. Title bars come from the window manager, or on Wayland from
  the compositor where it offers server-side decorations and a client-side
  frame otherwise. The app draws its own window controls only where no frame
  could be made.
- The toolbar and the rest of the chrome continue the title bar's fill,
  focused and not: on KDE the colour scheme's header colours from
  `kdeglobals`, as KWin paints its title bars, and on GNOME winit's Adwaita
  frame's. A settings portal signal re-reads them when the scheme changes.
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
