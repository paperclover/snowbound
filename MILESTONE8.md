# Workspace, crash recovery and diagnostic editing

Goal resumed after the user accepted the workspace split and public API audit.
API boundary changes remain authorized when justified by implementation needs.

## Acceptance gates

1. **Workspace:** move the existing library, tests and examples into
   `crates/onestore`, preserve the root corpus and example executable paths,
   and pass Rust, Python and fuzz build checks. A sibling crate can consume
   the library without reaching into its source directory.
2. **Public API audit:** review exported types, ownership, validation, edit
   contracts and consumer ergonomics; verify a separate crate can read and edit
   through public APIs; present [API-AUDIT.md](API-AUDIT.md) before continuing.
3. **Storage interruption:** verify durable images after every write/flush
   boundary, including dropped unflushed writes, partial persistence, counter
   rollover and revision checkpoints. Reopen the persisted image, continue
   editing it, retain reproducible failures, and independently validate a
   representative image matrix with OneNote.
4. **Abrupt VM stops:** stop disposable Windows clients and the Samba server
   without guest shutdown during normal concurrent editing. Preserve disks,
   restart the same machines, compare acknowledged operations and reachable
   conflicts, and cold-open the recovered server notebook in fresh OneNote.
   Track native cache acceptance separately from server durability.
5. **Diagnostic editor:** extend the HTML reading report with supported text
   edits on a task-owned notebook copy. Edits use the Rust library, reject stale
   snapshots, preserve unrelated content and expose ambiguous commit outcomes
   without automatic replay. Verify browser interaction, two-reader stale-edit
   handling, Unicode and native round trips. This is a diagnostic interface;
   canvas rendering and product UI remain separate work.
6. **Closure:** rerun affected checks, review the implementation for unnecessary
   state and abstractions, verify source hashes, retain evidence and replay
   commands, delete owned VMs, and provide the running diagnostic tool.

Abrupt VM stops discard guest memory while the host remains powered. Simulated
storage loss exercises the library's ordered-flush contract; neither establishes
the physical drive's behavior during actual host power loss.

## Evidence

Evidence belongs under `evidence/m8/`. Original notebooks are never edited.

### Workspace and API review checkpoint

The workspace gate passed: 56 Rust integration tests (one intentional local
fuzz-seed generator ignored), 36 Python tests, all example builds, all eight fuzz
target builds, Clippy, formatting, rustdoc and its compiled example. The external
API consumer also passes. Logs are listed in [API-AUDIT.md](API-AUDIT.md).

The user accepted the public API audit and authorized continuation. The checkpoint
changed source locations and documented existing contracts, preserving signatures
and runtime behavior. No VMs were started for that checkpoint.

### Storage interruption campaign

`power-loss-02` passed 2,508 persisted-image checks and subsequent edits across
native Unicode text, 255→256 and 65535→65536 counter rollover, an attachment page,
and a pending 512-revision checkpoint. Each write/flush boundary is exercised with
six persistence policies, including complete loss of unflushed writes and partial,
reordered persistence. Comparisons cover every resolved current object, including
raw properties and payloads. Acknowledged publication must select the complete
new state; earlier cuts may select only the complete old or new state.

`power-loss-03` repeats the same checks with exact source and failure-image
retention added to the harness. `edit-text-fuzz-01.log` records 4,327 stateful
inputs over 302 seconds without failure. The 22 retained boundary images from
`power-loss-02` are undergoing fresh OneNote captures in `power-native-01`.

Replay:

```sh
cargo run --release -p onestore --example power_loss -- /new/matrix-directory
python3 tools/power_loss_native.py /new/matrix-directory /new/native-directory
cargo +nightly fuzz run edit_text -- -max_total_time=300 -timeout=60 -max_len=128
```

The TOC extension (`power-toc-01`) passes another 918 persisted images and
subsequent color edits, including counter rollover and a pending checkpoint.
`power-loss-04` passes the combined 3,426-image matrix with exact source/failure
retention after the harness refactor. The native gate uses the 22 retained images
from `power-loss-02` and 13 from `power-toc-01`; captures verify those exact source
hashes. All 22 section captures in `power-native-01` passed. The TOC captures in
`power-toc-native-01` also compare OneNote's notebook color to the persisted value
and are still running at this checkpoint.
