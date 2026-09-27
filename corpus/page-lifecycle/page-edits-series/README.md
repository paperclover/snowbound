# Rust page moves in series of their own

`../page-edits` regenerated since a moved page heads a new page series, as OneNote's
drag gives the dragged pages one (`../movement`); its following subpages join the series
before it. `01` and `02` only indent and are unchanged. Each candidate reopened in a fresh
OneNote 2010 clone with every byte after the header and every active graph unchanged;
`provenance.json` records the captures (`evidence/series`) and removed clones.

Export with `ONESTORE_PAGE_MOVEMENT_OUTPUT` on `explicit_page_edits_match_native_selection_and_indentation`
in `crates/onestore/tests/page_movement.rs`, then capture each with `tools/native_runner.py
INPUT OUTPUT --expected-pages 9 --collect-notebook`.
