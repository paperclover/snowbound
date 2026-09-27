# Testing, and why the code can be trusted

Snowbound writes into other people's notebooks, often while their copy of
OneNote is writing to the same file. A bug doesn't just crash an app. It can
corrupt a shared notebook, or silently drop a paragraph someone else typed.
The testing strategy follows from that. **Never grade the code with itself.**
Every important claim is checked against something the code under test didn't
produce: a real OneNote, a separate model, a second path through the system,
or a disk that loses bytes on purpose.

This essay explains where the confidence comes from. The how-to for running
each lane is in [tools/TESTING.md](../tools/TESTING.md) and the crate READMEs.

## The reference is a real OneNote

The only authority on "OneNote accepts this" is OneNote. The lab (`tools/w7`)
runs OneNote 2010 in disposable QEMU clones of a sealed Windows 7 image. An
agent inside each clone runs AutoHotkey and PowerShell (OneNote's COM API),
returns screenshots and files, and the clone is discarded afterwards.

Every storage feature goes through the same loop:

```text
observe    author the edit in OneNote (COM, or driving its UI) and dump what it stored
write      make Snowbound store the same thing through ops; export a candidate notebook
cold-open  a fresh clone with a fresh OneNote cache opens the candidate
           ─► integrity check passes, XML export, screenshots, PDF where layout matters
gate       a VM-free test compares the capture with what the candidate meant to say
install    candidate + capture become a corpus row (corpus/<feature>/…)
```

*Cold* matters. A warm OneNote has its own cached copy and can hide a file it
would reject. Some failures appear only at scale. OneNote once silently dropped
elements in about one build in five because of an identity choice its integrity
check accepted, and it refuses revision chains past a certain depth. Gates
therefore include large mixed candidates, not only one small row per feature.

The corpus is what makes this sustainable. Each row keeps the native capture
next to the candidate, and its `tools/test_*.py` gate checks it without a VM,
on every run, forever. Re-running the VM step is needed only when the bytes
Snowbound writes change.

## Oracles inside the build

Most checking needs no VM. It works by making independent views of one edit
agree:

- **The model oracle.** `onestore::op::model` interprets ops on the page model
  without going near bytes. After a random edit is applied to a `Section`,
  sealed and read back, the page must equal what the model predicted.
  Refused edits must leave the page unchanged.
- **Differential editing.** For every editor action, alone and in random
  sequences with undo and redo, on every page of a set of corpus sections,
  four views must agree: the page stored through the ops, the model's
  prediction from those ops, the sealed image read back, and the editor's own
  page (`crates/canvas/tests/ops_differential.rs`).
- **Seals check themselves.** Each seal validates exactly what it appended. The
  full-file validator runs on open and over the tests' images.
- **Layout agrees with itself and with OneNote.** Incremental layout must
  equal a fresh layout after long structural histories. Undo and redo must
  restore geometry exactly. Unicode range replacement is compared with plain
  string replacement across hundreds of combinations. Wraps and heights are
  compared with OneNote's own XML and PDF exports.
- **Conversions verify their output.** A cache schema migration must reproduce
  every page the old cache held, or it rolls back.

## Failure, on purpose

Durability claims are tested by making things fail at every point that
matters.

- **Torn writes.** The storage crash model is a disk that, at an injected
  failure, persists a random subset of the bytes written since the last flush.
  Every I/O point of ordinary commits and counter-rollover commits is cut.
  Afterwards the file must read as the old image or the new one, never a
  third thing. The same model drives the commit fuzzer.
- **Crashes in the replica.** SQLite transactions are cut while their frames
  are in the log but the commit frame isn't, at every step of edit, seal,
  publish and acknowledge. Reopening must recover the queue as it was.
- **Lost replies.** `tools/smb-proxy.py` sits between the client and a lab
  Samba server and withholds a chosen request or response. The matrix cuts
  every message of a commit. Each outcome must keep its promise:
  `NotCommitted` really didn't publish, `Committed` really did, and `Unknown`
  cases really went either way, with the confirmation that follows finding
  out which.
- **Power cuts.** The lab can kill a VM outright mid-workload, and the
  surviving files are then cold-opened by a real OneNote.
- **Schedules.** Deterministic multi-actor tests run several replicas editing
  offline and publishing through a fault-injecting in-memory remote, then
  check convergence and conflict pages.

## Real clients on a real share

The collaboration lab puts several OneNote 2010 clients and several Snowbound
writers and readers on one section in a Samba share. They make random edits
through outages, reconnects, offline stretches and OneNote's own maintenance
(Optimize rewrites the file underneath everyone). The pass criteria are strict:

- every intent from every client is in the final file exactly once, or is
  accounted for as a conflict page;
- a wire trace shows OneNote was never refused a read; the only refusals it
  met are the ones its own protocol makes between two writers;
- a fresh OneNote cold-opens the result and agrees.

## Fuzzing

`fuzz/` is its own cargo-fuzz workspace. It covers parsing (storage, property
streams, revisions, documents), the commit protocol with interruptions, edits
of many kinds, the page model, protected sections, the offline queue, and the
canvas editor's state machine. The parsers see arbitrary bytes, and the
writers see arbitrary sequences of edits whose results must still validate.

## Two tiers of tests

Tests fall into two kinds, and the suite is being split to match:

- **Correctness tests** are deterministic and run on every change: `cargo test`
  over the workspace, clippy, and the Python gates over retained captures.
  `tools/check_public.py` runs all of it from a clean checkout, with no private
  notebooks, VMs or credentials.
- **Sweeps and fuzzing** are random sequences over large corpora and
  coverage-guided fuzzers. They are opt-in (today, sweeps widen through
  environment variables such as `CANVAS_SWEEP_*` and `OPS_SWEEP_SECTIONS`) and
  are meant to run for days or weeks on dedicated machines. What they find gets
  reduced to a small deterministic test in the first tier.

Lab lanes (real OneNote, Samba VMs, the proxy) sit beside both tiers. In Rust
they appear as ignored tests that name the environment they need, and the rest
live in Python harnesses under `tools/`.

## What counts as evidence

The project is strict about what a result establishes, and that strictness is
itself a source of confidence:

- An ignored test, a missing capture, an unreachable lab, a mocked harness or
  an iOS build that only compiled establishes nothing about native
  compatibility.
- A process exit, a VM interruption and a physical power loss are different
  fault models, and one never stands in for another. The evidence covers
  transport failures and VM cuts, not every server's lock implementation or
  real hardware losing power.
- Notebooks under test are always disposable copies with recorded hashes.
  Nothing points an editing harness, a replay or the app at an original.
- Accessibility is tested by asserting on the AccessKit tree the app builds
  and on the running app's accessibility hierarchy. Nobody turns a screen
  reader on to listen for it.
