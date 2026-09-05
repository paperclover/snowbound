# Live collaboration and interruption matrix

`tools/native_collaboration.py` ran two fresh Windows 7 / OneNote
14.0.4763.1000 clients against the disposable Linux Samba server. `linux.json`,
`server.jsonl`, `run.json`, each client's native captures, and checkpoint notebooks
record the environment and operations. `result.json` contains seven passing cases:

- A Rust scalar edit and a native edit to another outline converged.
- Concurrent edits to the same text retained both versions as a native conflict.
- Samba restart preserved the converged state.
- Disconnecting one client's lab NIC allowed independent offline/online edits;
  both converged after reconnection.
- An exclusive macOS file lock prevented native changes to the shared bytes;
  the queued native edit synchronized after release.
- Terminating and restarting OneNote preserved already synchronized edits.
- Abruptly terminating QEMU and reopening the same overlay preserved those edits.

`retention.json` validates the current page's explicit conflict-space reference,
not merely the presence of old text in historical revisions. The final notebook
was reopened from a separate fresh cache in `../live-collaboration-cold-15`.
Its native comparison passed, and `conflict.png` shows the competing edit in
OneNote's read-only conflict view. Both Windows clients, the verifier, and the
Linux VM were deleted; teardown records accompany each run.

Process/VM termination covers already server-persisted data. It does not claim
unsynchronized cache durability or physical host power-loss safety. The separate
stage-5 corpus covers lost successful SMB FLUSH responses.

The controller submits only the edited top-level content container. Submitting
all cached page containers in an offline update replayed stale content over an
unrelated online edit; failure evidence is `evidence/m6/live-collaboration-13`.
Mac SMB mounts were explicitly reconnected after Samba restarts because the OS
revoked the original mount. Failed setup runs remain separate from acceptance.
