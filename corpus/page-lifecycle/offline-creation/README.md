# Offline page creation

Twelve independent replicas start from the same one-page section. Each queues
a page titled `Same 🦋 é`, a body outline and a dependent body-text edit, then
reopens its cache and synchronizes. All 36 receipts complete; page identities
remain distinct and body text identifies each originating replica.

`candidate/pages.one` is the resulting 13-page section. `cold` captures its fresh
OneNote 2010 reopen. Native XML content/order and 216 explicit character-format
comparisons pass. Active graphs compare exactly apart from the original seed
page's previously absent section metadata copy, materialized natively with exact
page metadata. The creation verifier checks that specific addition.
`provenance.json` records the capture and removal of clone m6-cc15a5ee.

Set `ONESTORE_OFFLINE_PAGE_OUTPUT` to an absolute, new output directory and run
`cargo test -p onestore-offline --all-features --test sync
page::offline_page_creation_rebases_with_dependent_edits_and_duplicate_titles
-- --exact`. Capture the result with `tools/native_runner.py INPUT OUTPUT
--expected-pages 13 --collect-notebook`. After building examples,
`python -m unittest discover -s tools -p test_page_creation.py -v` verifies the
retained native capture.

This fixture is a serial reconciliation of twelve offline branches. The separate
`offline_page` sanitizer target varies interleavings, interruption and reopen
through the shared `tests/support/page_schedule.rs` model.
