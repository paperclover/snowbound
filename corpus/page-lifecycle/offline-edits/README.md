# Offline page order and indentation

Eighteen cases combine two offline intents with the nine retained native movement
states in `../movement`. Each starts from `../04-nested`, queues an indentation
change or explicit three-page movement, then a dependent body-text edit. Nine
cases reconcile directly; nine require reviewed placement retaining the original
page and allocated series identities. Both edits survive cache reopen.

Each case contains its generated candidate and intent manifest, plus a fresh
OneNote 2010 capture. Native page order, text and character formatting agree;
active graphs compare exactly and all bytes after the 1024-byte header remain
unchanged. The warm native promoted-parent input contains stale cached indentation;
the Rust topology edit refreshes it before publication. Its strict comparison
needs no stale-cache exception.

`twelve-clients` starts twelve independent offline branches from one 37-page
section. Each branch reorders two pages, nests another, and inserts its own body
outline. All 24 publications reconcile without review. The native reopen retains
all page identities, order, indentation and twelve distinct Unicode bodies.
OneNote fills the seed page's absent optional metadata copy; the verifier checks
its complete contents and references. `provenance.json` records all nineteen
captures, source hashes, scripts, base image and clone removal.

Reproduce candidates with new absolute output directories:

```sh
ONESTORE_OFFLINE_PAGE_EDIT_OUTPUT=/tmp/one-page-edits cargo test -p onestore-offline --all-features --test sync pages::native_page_changes_reconcile_with_atomic_offline_batches_and_review -- --exact
python tools/offline_page_edits.py /tmp/one-page-edits /tmp/one-page-edits-cold --workers 4
ONESTORE_OFFLINE_PAGE_CLIENT_OUTPUT=/tmp/one-page-clients cargo test -p onestore-offline --all-features --test sync pages::twelve_disjoint_page_batches_merge_with_dependent_bodies_without_review -- --exact
python tools/native_runner.py /tmp/one-page-clients /tmp/one-page-clients-cold --expected-pages 37 --collect-notebook
```

After building the `onestore-notebook` document example, run
`python -m unittest discover -s tools -p test_page_movement.py -v` to compare the
retained captures. The `offline_page` sanitizer target exercises twelve replicas,
atomic page batches, conflict review, creation, body insertion, cache reopen and
interrupted publication using the shared Rust schedule model. `schedules` retains
the twelve initial inputs for ASan seed 1987 (163 executions, including 150
mutations). Link them into a new writable corpus directory, then run
`cargo +nightly fuzz run offline_page CORPUS -- -seed=1987 -max_total_time=240
-timeout=30` from the repository root.

These captures establish native compatibility of reconciled results. The twelve
branches publish serially; the schedule model varies their interleavings. Neither
is a simultaneous native/Rust SMB campaign or physical power-loss test.
