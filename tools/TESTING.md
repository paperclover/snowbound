# Library regression lanes

## Public fixtures

From a checkout with Rust and Python 3 plus Pillow installed:

```sh
python3 tools/check_public.py /absolute/path/to/new-results
```

The command runs formatting, all workspace features/targets, doctests, Clippy,
diagnostic/example builds, and Python regression tests. Each stage has its own
log; `results.json` records executed commands, elapsed times and exit statuses.
Only a completed run receives `status: passed`. Existing result directories are
refused. This lane clears inherited `ONESTORE_*` overrides and uses this
checkout's binaries so personal captures or external exporters cannot silently
replace public inputs.

Repeat the same command in a fresh checkout containing only versioned files to
verify clean-checkout compatibility. Fixture symlinks must remain symlinks; their
targets are versioned in this repository. Neither `corpus/private`, ignored
`evidence`, existing `target` outputs, credentials nor running virtual machines
are required. Cargo's normal dependency cache may be reused.

Rust explicitly reports ignored lab tests and fixture generators. Those cases
are **not** part of a successful public run. The Python suite tests native/lab
harness logic using retained synthetic captures and mocks; it does not claim a
new execution of OneNote or a real server interruption.

## Private and native verification

Private notebooks stay outside versioned fixtures. Materialize a copy before
editing and retain source hashes; never point an authoring harness at an original
notebook. Live acceptance uses explicitly owned disposable targets and preserves
the run's commands, inputs, outputs and teardown evidence.

| Boundary | Entry point | Acceptance evidence |
| --- | --- | --- |
| Native authoring and cold reopen | `native_runner.py --help` | Independent OneNote capture; exact expected page count when known; owned clone teardown |
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
