# Snowbound

Snowbound is a modern remake of OneNote 2010 that stays fully interoperable
with it. Notes are rich text on a free canvas, stored in OneNote's own `.one`
files, so Snowbound and OneNote 2010 can open the same notebook side by side.
A notebook is just a folder. Put it on an SMB share and several people can
edit it together from either app, with no cloud service involved. With headless
crates implementing the file format and a generic canvas renderer made with a
novel UI kit, each new platform port is extremely lightweight.

## Crates

| Path | Owns | Depends on |
| --- | --- | --- |
| `crates/onestore` | The file format: revision stores (`.one`, `.onetoc2`), the page model, ops, the commit protocol. No network, no SQLite, no `unsafe`. | none |
| `crates/notebook` | From editor to disk or share: discovery, notebook structure, sessions, the SQLite replica, sync and merging, conflict pages, the embedded SMB client (feature `smb`). | onestore |
| `crates/draw` | The wgpu renderer that page and chrome both paint through, and the text-editing core (keys, chords, carets) they share. | none |
| `crates/canvas` | The page: editor, OneNote-faithful layout, page scene, interaction, the page's accessibility tree. | onestore, draw |
| `crates/ui` | The immediate-mode interface kit and OneNote's chrome controls. Knows nothing of notebooks. | draw |
| `crates/snowbound` | The desktop app: winit window, platform glue, sidebar, menus, templates. | all of the above |
| `crates/mobile`, `apps/ios` | A C surface over canvas and notebook, and the UIKit app built on it. | canvas, notebook, and below |
| `tools/`, `corpus/`, `fuzz/` | Lab harnesses (Python, PowerShell, AutoHotkey), the corpus of OneNote-verified files, and a separate cargo-fuzz workspace. | |

Dependencies point from the apps toward the format, never back. The format
crate never learns about storage engines, networks or pixels. `canvas` emits
ops but never touches storage. `ui` never sees a notebook. The hosts are where
these pieces meet.

## Rules that matter most

- **Edits are ops, and each edit appends one revision.** `onestore` and
  `notebook` never recreate a whole page or section from a model. Only creating
  a section or page, and opening a file, handle whole images. A design that
  diffs whole pages to save them is the wrong design.
- **Write what the spec requires and what OneNote writes; read tolerantly.**
  Emit every property MS-ONESTORE and MS-ONE require (even where OneNote
  tolerates an omission), plus what OneNote itself stores. Readers accept
  whatever is out there.
- **OneNote 2010 is the reference.** When behaviour is in question, observe it
  in the Windows 7 lab (`tools/w7`) before deciding. A storage feature is done
  when a file Snowbound wrote cold-opens in a fresh OneNote and reads back as
  intended ([testing](arc/testing.md)).
- **Disposable copies only.** Never point the app, a replay script or a lab
  harness at an original notebook.

## Sync in one breath

A `.one` file is a revision store. Every client, OneNote included, commits by
appending a revision and then rewriting the 1024-byte header, which makes the
header plus the file length a cheap *stamp* of the committed state. Snowbound
queues edits as ops in a local SQLite replica, then a background thread polls
the stamp. If the stamp is unchanged, it publishes the queued batch as one
appended revision. If it has changed, it reads the file once and replays the
queue on top. Where an op can't merge, the result is what OneNote 2010 makes: a
read-only conflict page under the page. OneNote coordinates writers through
share modes and one-byte locks on the section file itself. These are special SMB
protocols that only Windows supports, so Snowbound carries its own SMB client
that can properly issue them. Offline is just a sync step that fails: the queue
waits and publishes later. [More in the sync essay.](arc/sync.md)

## Architecture notes

- [The file format and `onestore`](arc/file-format.md): revision stores,
  object spaces, transactions, and what "append one revision" means.
- [The data layer and sync](arc/sync.md): ops, sessions, the replica,
  publishing, merging, conflict pages, SMB and locking, offline.
- [The page: editor and canvas](arc/canvas.md): layout, OneNote-faithful
  geometry, the page view, accessibility.
- [The interface kit and the renderer](arc/ui.md): immediate mode,
  custom drawing, the OneNote shell and its motion.
- [Platforms](arc/platforms.md): macOS, Linux, and iOS with UIKit around the
  canvas.
- [Testing, and why the code can be trusted](arc/testing.md): the native
  lab, oracles, failure injection, fuzzing, and the two test tiers.

The crate READMEs ([onestore](crates/onestore/README.md),
[notebook](crates/notebook/README.md)) are the API references.
[tools/TESTING.md](tools/TESTING.md) lists the test lanes.

## Working here

```sh
cargo test --workspace --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
python3 -m unittest discover -s tools -p 'test_*.py'   # VM-free corpus gates (needs Pillow)
python3 tools/check_public.py /absolute/new/results     # everything above, from a clean checkout
python3 tools/canvas/build_macos.py && target/Snowbound.app/Contents/MacOS/Snowbound --notebook COPY
```

The MS-ONESTORE and MS-ONE specifications are Microsoft Open Specifications.
Local copies, design history and raw lab evidence are kept out of version
control.

External contributors (not agents, will be denied unless a human writes to me)
are welcome to email `git@paperclover.net` to gain access to the testing VM
images for their agents. Alternatively, a custom image can be configured with a
licensed copy of Windows and OneNote. By connecting `tools/w7/mcp_*.py`, the
agent can interactively play around in with the actual prior art, in addition
to running tests.
