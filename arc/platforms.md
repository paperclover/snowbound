# Platforms

Snowbound is one core with thin hosts around it. The file format, sync, the
page editor and the renderer are identical everywhere. What changes per
platform is the window, the input plumbing, and how much of the interface is
drawn by Snowbound versus the operating system.

```text
         macOS                Linux                Windows                  iOS
shell    snowbound + ui       snowbound + ui       snowbound + ui           UIKit (apps/ios)
glue     snowbound/src/macos  snowbound/src/linux  snowbound/src/windows    crates/mobile (C ABI)
page     canvas ─────────────────────────────────────────────────────────────────────►
paint    draw (wgpu: Metal)   draw (Vulkan or GL)  draw (D3D12; D3D11 on 7) draw (Metal)
data     notebook::session + embedded SMB client ────────────────────────────────────►
format   onestore ───────────────────────────────────────────────────────────────────►
```

## Desktop: `snowbound`

The desktop app is a winit window with the `ui` kit's chrome and a `canvas`
page inside it. `snowbound` picks a platform module at compile time
(`macos.rs`, `linux.rs` or `windows.rs`, each mounted as `platform`), and
everything else in the crate is shared: library and settings, the sidebar,
menus, page and section management, templates, and screenshot and replay
support. Accessibility goes through AccessKit's winit adapter on all three:
the interface's tree, with the page's grafted into it.
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
  (used as the author, as OneNote uses Office's user name), the caret and
  selection colours, and the spell checker, `NSSpellChecker`, which the
  spelling thread calls on the main thread (10.6 has it too).
- Frames present inside Core Animation's transaction, so a resize pairs each
  frame with the window's new size instead of stretching the last one.
- A notebook on a mounted SMB share is opened through the embedded SMB client,
  signed in with the password the keychain keeps for that mount (see
  [sync](sync.md) for why the mount itself isn't enough). Open Notebook from Server… reaches one
  by address instead, and keeps a password it asks to remember as the Finder does, an
  SMB internet password.
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
- The desktop portal provides the file pickers, zenity or kdialog the alerts,
  and where neither can, Snowbound asks with the kit's own dialogs in the
  window. No dialog waits on the event loop's thread. The XDG settings portal
  provides the colour scheme. Text conventions come from the C library's
  locale. Fontconfig is loaded at run time, so builds need no headers for it,
  and so is Enchant, which checks spelling with whatever dictionaries its
  providers have; without it, words go unmarked.
- Wayland's clipboard goes through the window's own connection, since not every
  compositor offers a clipboard to clients without a window.
- The executable carries its desktop entry and icon (`desktop_linux.rs`). The
  window is `net.paperclover.snowbound` to Wayland and X11 alike. Where a
  Wayland compositor lacks xdg-toplevel-icon, as GNOME's does, it finds the
  icon only through an entry of that name, so a run that isn't installed
  writes a hidden one pointing at an icon in the runtime folder, marked with
  its process ID, and removes both on exit or at the next start after a crash.
  KWin takes the icon from the window. Install on the welcome copies the
  executable to `~/.local/bin` and writes the visible entry under the same
  name, so the menu never lists two; Uninstall in Options removes it.

### Windows

- One executable runs on Windows 7 SP1 through 11. It is built from macOS
  with llvm-mingw against `msvcrt.dll`, which every Windows has
  (`platform/windows/cargo.sh`): x86_64 on nightly's tier-3
  `x86_64-win7-windows-gnu`, whose standard library avoids Windows 8's
  imports, and aarch64 for Windows 11 on Arm. The few imports of Windows 8
  and later that dependencies still name are answered by
  `platform/windows/rt/shims.c`; everything newer is looked up at run time.
- `draw` builds three backends on Windows and the surface picks one:
  Direct3D 12 through wgpu where Windows has it (10 and 11), otherwise
  Direct3D 11, as on Windows 7, on WARP where no device reaches feature
  level 10_0, and OpenGL 2.1 on a WGL context only where neither starts
  (`surface_windows.rs`). Direct3D 11 presents through a blit-model DXGI
  swap chain, as 7 has no flip model, and through DirectComposition on 10,
  whose window has no redirection surface; OpenGL drivers such as Intel's
  place their frames below the caption the system would draw even where the
  row takes its place.
- The toolbar's row is the title bar wherever the desktop composes windows.
  On Windows 7 with Aero and on 11, the system's frame keeps its sides,
  its caption gives way to the row, its material (Aero glass, Mica) lies
  under the whole window, and the system hit-tests and runs its own
  caption buttons over the row, which brings 11's snap layouts. 7 draws
  them over the glass; 11 doesn't over the Direct3D surface, so the row
  draws them as 11 does, lit where the system reports the pointer. On 8
  and 10, whose frames are opaque, the window has no system frame and the
  row draws and runs caption buttons as 10 does, over acrylic on 10. Over
  7's glass, which takes any colour and whatever lies behind the window,
  the toolbar keeps opaque faces under its icons: by default the strip is
  opaque, as Explorer's command bar is under its glass, stopping short of the
  caption buttons 7 draws beneath the window's pixels, and
  `SNOWBOUND_W7_CHROME` tries the alternatives, `pills` for a face per group
  of tools over the glass, `frost` for a strip the glass tints through, and
  `tint` and `tint-tiles` for one panel or a tile per group in the glass's
  colour at a lightness text keeps its contrast on, as Mica tints.
  The theme and material follow the system's colour mode as it changes.
  With Windows 7's basic or classic theme the system draws the title bar
  and the row lies beneath it, as on KDE.
- A notebook on a share opens by its UNC path through Windows' own SMB
  client, which takes OneNote's opens and locks natively: `onestore` opens a
  section as OneNote does (a reader shares it with everyone, a writer denies
  other writers) and takes OneNote's coordination bytes with byte-range
  locks, so both apps can have a section open on one machine.
  ReadDirectoryChangesW reports changes below a notebook, including other
  clients' on a share. Open Notebook from Server… still uses the embedded
  client, its passwords in the Credential Manager.
- The common file dialogs, task dialogs, the date and time picker, and the
  Spell Checking API (from Windows 8) are the system's; audio plays and
  records through MCI. Dates follow the user's locale as OneNote's do.
  Windows has no menu bar: the toolbar and the command palette run the
  command table, with OneNote 2010's Ctrl chords.

### The browser

- The same `snowbound`, built for `wasm32-unknown-unknown`: `web.rs` is its platform
  module, and `web/index.html` and `web/glue.js` are the page around it. `glue.js` brings the
  canvas's pointer, wheel and touch, and a hidden text area's keys, composition and paste, as
  the `ui::Event`s winit would; `web.rs` stands in for winit's window and event loop and runs
  `State` a turn per animation frame. `tools/release_web.py` builds and publishes it.
- The page has one thread. The notebook's section thread, sync worker and background run as
  tasks on its event loop (`notebook::task`), and work the desktop gives a thread runs once
  the frame is done (`spawn`).
- Files are `notebook::fs`'s: std's elsewhere, here held in memory, with SQLite's VFS over
  the same files, so replicas and notebooks live side by side under `/Notebooks` and
  `/Cache`. A storage worker keeps them in the origin's private file system, writing the
  byte ranges each burst changed through OPFS's synchronous handles, which only workers get.
  One tab at a time holds them.
- Open Notebook, where the browser has the File System Access API (Chromium), opens a folder
  of the user's: mirrored under `/Folders`, its handle kept in IndexedDB, other apps' writes
  read every few seconds. A browser takes no locks, so a commit there stands only once the
  page has written the file and found nothing else wrote it since it was read; until then
  it is uncertain, as one whose answer was lost, and it goes again on top of another app's
  write. The sync popup says the folder isn't locked.
- Menus are the kit's own, as on Linux, with the PC's chords and ⌘ for Ctrl on a Mac; the
  browser keeps its own window and tab chords. Dialogs are the browser's; Insert, and Open
  without a folder to give, ask for files to copy in; printing downloads the PDF. Servers and
  recording are still to come.
- Spelling is Hunspell's American English dictionary through `spellbook`, fetched beside the
  module; Add to Dictionary keeps its words in the browser's files. Text the bundled faces
  lack takes Noto's, fetched by script the first time a page holds it (a CJK face cut to
  the national standards' characters by `tools/web/subset_cjk.py`).
- AccessKit has no web adapter, so once a screen reader asks for it (a visually hidden
  button, then on every visit) the trees AccessKit would get are mirrored as hidden
  elements with ARIA roles; acting on one sends AccessKit's action back.

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
pages, search, spelling (through `UITextChecker`), and page management through
the same ops. Notebooks from Files
are read and written under `NSFileCoordinator`, so file providers see every
change.

Record Audio and Record Video record through AVFoundation and hand `crates/mobile` the
WAV file, or the camera's pictures and sound, which it stores as the desktop does. Audio
keeps recording in the background and with the screen locked, under the `audio`
background mode, as a lecture outlasts the screen's timeout; a call pauses it until the
system says to go on. The camera stops in the background, which ends a video recording
and saves it. A tap on a recording plays it with `AVPlayer` in a bar over the page's foot.

## What stays shared, on purpose

- **Behaviour**: the editor, hit-testing, the placement grid, undo, conflicts
  and search all live in `canvas` or `notebook`. A host that reimplements one
  of them has made a bug.
- **Storage**: every platform writes through the same ops and the same
  replica, and publishes through the same sync step.
- **Look of the page**: the page renders through `draw` everywhere, so a page
  looks the same on a phone as on the desktop.
