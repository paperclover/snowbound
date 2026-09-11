# Native page-removal references

Eleven synthetic OneNote 2010 cases exercise individual parent, child, grandchild,
trailing and leading-page removal; explicit multi-page removal; removal of every
page; permanent removal; and recycling pages containing tables, list controls,
tags, images, attachments and ink. The two leading cases also contain a nonempty
page-version graph. These are sequential COM `DeleteHierarchy` calls with frozen
original page identities. They do not represent keyboard deletion of a collapsed
selection or an atomic notebook-wide transaction.

Every case retains synchronized before/after section files, full native page XML
and binary payloads, scripts, requested page indices, and an independent cold
capture. `provenance.json` records the captured hashes and all 22 clone teardown
records. Identical files within the capture have one stored copy and relative
links. All fixtures originate from the public synthetic corpus.

The raw Rust oracle checks retained page/object identities and bodies, page order,
series reuse, indentation, source tombstones, retained revisions, labeled history,
and exact cold graphs. Recycled pages are associated by their notebook-management
GUID, including when titles are identical. Reachable properties, nested property
sets, ordered references, context relationships and internal file payloads compare
across identifier remapping. Only identical author records may merge. Current-page
metadata normalizes to level one; observed modification-time changes must equal
the copied page's timestamp. Version-page levels and timestamps remain exact.
Native discards nine old storage revisions in the ink case and eight in the
feature case; those counts are explicit regression expectations. Removal of
storage revisions is distinct from losing semantic page versions.

The copy oracle includes negative controls for an ordinary text edit and a
changed attachment payload. Python independently compares native XML, character
formatting and media against before, after and cold stores. Empty native section
hierarchies can contain only an XML declaration after reopening.

```sh
cargo build -p onestore-notebook --all-features --example document
cargo test -p onestore --all-features --test page_removal
python -m unittest discover -s tools -p test_page_removal.py
```

A fresh owned-clone capture uses `python tools/native_page_removal.py CASE OUTPUT`,
where `CASE` is a key from `provenance.json` and `OUTPUT` is a new directory. The
controller removes its clones after use. These fixtures establish reference
semantics for the page-removal writer; they do not implement that writer.

`sanitizer.json` records 64 initial mutation inputs and the 32 supplemental store
paths/hashes for the existing `document` fuzz target. With ASan and seed 1993,
20,000 executions completed in 40 seconds without an invariant or sanitizer
failure. Decode the input hex into a new writable corpus and set
`ONESTORE_DOCUMENT_SEEDS` to the recorded repository-relative paths joined with
the platform path separator, then use `cargo +nightly fuzz run document CORPUS --
-seed=1993 -max_total_time=120 -timeout=30 -runs=20000`. The target also retains its
built-in synthetic sources. This is bounded reader testing of these inputs.

`rust-followup` takes four Rust `delete_pages_permanently` outputs (`parent`, `all`,
`features`, `leading-parent`) and lets OneNote 2010 create a titled page with a bold
body in each section through COM (`tools/native/page-removal-followup.ps1`), then
reopens the natively saved section through a fresh cache. The Rust test
`native_pages_created_after_rust_removal_reopen_cold` requires every surviving page
to keep its exact active revision through both native phases, the new page to carry
the native title and body, and the cold reopen to change no object space.
`tools/test_page_removal.py` compares both captures' native XML with the Rust model
and finds the native title exactly once in each hierarchy. Regenerate the candidates
with `ONESTORE_PAGE_REMOVAL_OUTPUT` on the removal reference test, then run the
runner with `--author` into `followup` and cold into `followup-cold`.
