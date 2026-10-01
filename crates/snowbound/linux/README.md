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

If Snowbound stops unexpectedly, `~/.local/state/snowbound/snowbound.log` says
why; please attach it when you report a problem.

**Install Snowbound**, shown while no notebook is open, adds Snowbound to your app menu and
to the apps that open `.one` and `.onetoc2` files, and copies it to
`~/.local/bin/snowbound`. Options, next to the update settings, uninstalls it
again; your notebooks stay where they are.

The window's title bar is your desktop's own: KDE's on KDE, and on GNOME and
other desktops without server-side decorations an Adwaita-style one drawn by
Snowbound.

## Requirements

- x86_64 or aarch64 Linux with glibc 2.17 or newer (RHEL 7, Debian 8,
  Ubuntu 14.04 and later). NixOS needs nothing extra: Snowbound finds the
  libraries below among the system's packages or your profile's.
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
