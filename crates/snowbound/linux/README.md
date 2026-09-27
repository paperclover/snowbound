# Snowbound for Linux

An early, lightly tested build of Snowbound, a note-taking app that reads and
writes OneNote 2010 notebooks. Expect rough edges; please report what breaks.

## Run

```sh
tar xzf snowbound-linux-x86_64.tar.gz
cd snowbound-linux-x86_64
./bin/snowbound
```

Choose **Open Notebook…** in the notebook list and pick the notebook folder,
the one holding the `.one` section files and `Open Notebook.onetoc2`.
Snowbound remembers the notebooks you open. `--notebook FOLDER` opens one
straight away.

Edits save to the notebook folder as you type. Settings live in
`~/.config/snowbound` and sync state in `~/.cache/snowbound`; `--cache DIR`
puts the latter elsewhere.

Run `./install.sh` to add Snowbound to your app launcher; it copies the app
into `~/.local`. On Wayland the taskbar, dock and KDE's title bar find
Snowbound's icon only through this launcher entry, so without it they show a
generic icon.

The window's title bar is your desktop's own: KDE's on KDE, and on GNOME and
other desktops without server-side decorations an Adwaita-style one drawn by
Snowbound.
`./install.sh ~/Notebooks/MyNotebook` makes the launcher open that notebook as
well.

## Requirements

- x86_64 or aarch64 Linux with glibc 2.31 or newer (Debian 11, Ubuntu 20.04,
  Fedora 32 and later).
- A Vulkan driver (Mesa's are standard) or, failing that, OpenGL ES 3 through
  EGL. `WGPU_BACKEND=gl ./bin/snowbound ...` forces OpenGL.
- fontconfig, and X11 or Wayland with libxkbcommon.
- zenity or kdialog for the folder picker, the page date and time dialogs and
  alerts.

## Known limits

- No New Notebook or menus yet.
- The dark or light look follows the desktop's setting when the app starts.
- The character palette (Control-Command-Space on macOS) has no Linux
  counterpart.
- The window stops drawing while the folder picker or a date dialog is open.
- Input methods work through winit and are barely tested.
