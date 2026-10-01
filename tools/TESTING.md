# Library regression lanes

## The CI gate

`python3 tools/ci.py` gates `main` (or `--rev REV`, or `--working-copy` for this
checkout's `@`) in the jj workspace `../snowbound-ci`: it points that
workspace's own commit, a child of `main`, at the revision's files, so other
checkouts' edits in progress never reach the result, and a working copy is
frozen as it was when the run started. Its build cache is
`../snowbound-ci/target`, apart from every agent's `target/`. Runs wait for one
another, and the exit status is the result.

| Lane | Runs |
| --- | --- |
| `fmt` | `cargo fmt --all --check` |
| `clippy` | Clippy `-D warnings` on the workspace (all targets and features), and `snowbound` without default features |
| `test` | builds every test target, runs the executables four at a time (`--test-jobs`) from their package folders, slowest last time first, and the doctests |
| `python` | builds the examples the suite runs, then `tools/test_*.py` under `uv` with Pillow and pdfplumber |
| `windows-x86_64` | `platform/windows/cargo.sh` build of `snowbound` (nightly's win7 target) |
| `windows-aarch64`, `linux-*` | Clippy `-D warnings` on what ships (libraries and binaries but `mobile`), then the `snowbound` build; Linux through `platform/linux/cargo.sh`, which links with zig against glibc 2.17 |
| `ios` | `xcodebuild` of the simulator app, unsigned |
| `web` | Clippy `-D warnings` on `snowbound` for `wasm32-unknown-unknown`, SQLite built by nixpkgs' clang; `release_web.py` links, optimizes and publishes the static folder |
| `macos-10.6` | `platform/snow-leopard/cargo.sh` build of `snowbound`; skipped, saying why, without the SDK or nightly `rust-src` |

```sh
python3 tools/ci.py                          # every lane, on main
python3 tools/ci.py --working-copy --changed # the lanes and test packages @'s changes from main reach
python3 tools/ci.py --rev xyz --lanes test windows   # `windows` names both windows-* lanes
```

Lanes run four at a time (`--jobs`), each under its own time limit
(`--timeout MINUTES` overrides them all). The table it prints names each
failure's first errors with their files and lines, to tell whose edit broke
it; `../snowbound-ci/target/ci/runs/TIME/` keeps every lane's log and
`summary.json` (status, seconds, errors with files, each test executable's
time), for the last 20 runs. After a run over `--budget` (80 GB), it deletes
the build units this run didn't use, least recently used first, which keeps
`deps/` small for the font tests that scan it. Windows needs llvm-mingw from
`platform/windows/toolchain.sh` in this checkout's `target/windows`, or
`LLVM_MINGW`; Linux needs `zig`. `release.py` runs the gate on the commit it
publishes.

The workspace is made on first use; to drop it, `jj workspace forget ci` and
delete `../snowbound-ci`.

## Public fixtures

From a checkout with Rust and [uv](https://docs.astral.sh/uv/), which provides
Python 3.12 with Pillow and pdfplumber without installing anything system-wide:

```sh
uv run --no-project --python 3.12 --with pillow --with pdfplumber \
  python tools/check_public.py /absolute/path/to/new-results
```

The command runs formatting, all workspace features/targets, doctests, Clippy,
diagnostic/example builds, and Python regression tests. Each stage has its own
log; `results.json` records executed commands, elapsed times and exit statuses.
Only a completed run receives `status: passed`. Existing result directories are
refused. This lane clears inherited `ONESTORE_*` and `SNOWBOUND_*` overrides and
uses this checkout's binaries so personal captures or external exporters cannot
silently replace public inputs.

Repeat the same command in a fresh checkout containing only versioned files to
verify clean-checkout compatibility. Fixture symlinks must remain symlinks; their
targets are versioned in this repository. Neither `corpus/private`, ignored
`evidence`, existing `target` outputs, credentials nor running virtual machines
are required. Cargo's normal dependency cache may be reused.

For the Python tests alone, end the same `uv run` with
`python -m unittest discover -s tools -p 'test_*.py'`. Without pdfplumber they
skip the PDF oracles (`test_pdf_format`, part of `test_document_oracle`) and
say so; `check_public.py` refuses to start without it.

Rust explicitly reports ignored lab tests and fixture generators. Those cases
are **not** part of a successful public run. The Python suite tests native/lab
harness logic using retained synthetic captures and mocks; it does not claim a
new execution of OneNote or a real server interruption.

## Sweeps and soak

Seeded sweeps (random edit walks, multi-client schedules, differentials across
corpus sections) run a smoke slice by default. `SNOWBOUND_SWEEP=0` runs them in
full with the seeds as written; any other number shifts every sweep's seeds, and
a failure replays under the same value.

`tools/soak.sh [seconds per fuzz target]` soaks a dedicated machine until
stopped: each round runs the full sweeps under a random shift, in release with
debug assertions, then every `fuzz/` target for the time limit. A failing round
leaves `soak/*.log` ending in the command that replays it; crash inputs stay in
`fuzz/artifacts/<target>/`. On a Linux VM, install rustup's stable and nightly
toolchains, `cargo install cargo-fuzz`, and `build-essential pkg-config
libfontconfig-dev`; copy the checkout and run the script under `tmux`.

## Motion capture

`SNOWBOUND_FRAMES=DIRECTORY` writes every frame the app draws to
`DIRECTORY/MILLISECONDS.png`, timed from when the window opened. With a
hidden-window replay (`SNOWBOUND_REPLAY` plus `--screenshot`, see
`tools/canvas/README.md`) this records an animation at its real pace without
touching the screen: waits tick at 60 Hz, and the app draws as fast as it can
while it animates. Point it at a copy of a notebook, end the script with a
short `wait` so the last frames finish writing, and assemble strips or GIFs
with `ffmpeg`.

## Private and native verification

Private notebooks stay outside versioned fixtures. Materialize a copy before
editing and retain source hashes; never point an authoring harness at an original
notebook. Live acceptance uses explicitly owned disposable targets and preserves
the run's commands, inputs, outputs and teardown evidence.

| Boundary | Entry point | Acceptance evidence |
| --- | --- | --- |
| Native authoring and cold reopen | `native_runner.py --help` | Independent OneNote capture; exact expected page count when known; owned clone teardown |
| Password-protected sections | `native_protected.py NOTEBOOK PASSWORDS OUTPUT [SECTION]` | A fresh clone cold-opens Snowbound's protected sections, unlocks each in OneNote's dialog, types into one through COM and reads every page with the notebook it leaves (`corpus/protected-sections`) |
| Mixed native/Rust/offline writers | `native_collaboration.py --help` | Recorded intents, durable receipts, independent server state and cold native comparison |
| SMB directory pagination | `test_smb_directory.py --help` | Caller-owned Linux VM, native filesystem oracle, interrupted-page rejection |
| SMB publication and payload interruptions | Ignored tests in `notebook` (feature `smb`) | Explicit `ONESTORE_SMB_*` lab inputs, retained protocol traces and independent recovery checks |
| Application session on a share | `session_acceptance.py --help` | Page saves through `notebook::session` on the mounted Samba share, relaunch between launches, cold native reopen of the edited pages, owned VM and clone teardown |

To compare an additional **already captured** notebook hierarchy without running
OneNote:

```sh
PYTHONPATH=tools ONESTORE_NOTEBOOK_NATIVE=/absolute/path/to/capture \
  python3 -m unittest test_notebook_discovery
```

The capture contains `notebook/` and `read/hierarchy.xml`. This comparison is a
separate private lane and must retain its own log. A missing capture, unavailable
lab, compilation-only iOS result or ignored test never establishes live native
compatibility. Process exit, server/VM interruption and physical storage loss
remain distinct fault models.
