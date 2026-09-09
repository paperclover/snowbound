# Page creation

`candidate` adds four pages to `../03-renamed/notebook/Lifecycle.one`: an appended
page without a title node, a prepended page with an empty title field, and two
appended pages titled `New 🦋 é`. All thirteen ordered page identities survive
each fresh OneNote 2010 reopen. Identical artifacts are relative symlinks.

`cold` captures the first reopen. `native` adds a distinct body paragraph to each
duplicate-title page through `tools/native/created-pages.ps1`; `native/cold`
reopens that result in another cache. `followup` prefixes those bodies with
`Rust + ` and the titles with `Reviewed `; `followup/cold` reopens that result.
The retained `provenance.json` records the native capture inputs and clone removal.

After building workspace examples, run `python -m unittest discover -s tools
-p test_page_creation.py -v`. It compares native XML content and character formats,
ordered identities, and complete active object graphs across each cold reopen.
These thirteen-page comparisons need no graph normalization.

Export fresh Rust candidates by setting each variable to a new output directory:

| Core integration test (`cargo test -p onestore --test page TEST -- --exact`) | Variable |
| --- | --- |
| `native_section_accepts_empty_and_titled_pages_with_preserved_history` | `ONESTORE_PAGE_CREATION_OUTPUT` |
| `native_changes_on_created_pages_accept_rust_followups` | `ONESTORE_PAGE_FOLLOWUP_OUTPUT` |
| `repeated_page_creation_crosses_root_fragments_counters_and_section_checkpoints` | `ONESTORE_PAGE_STRESS_OUTPUT` |

The last test produces 521 pages, crosses a section revision checkpoint and
transaction-counter rollover, and checks that every unpublished candidate still
exposes the complete preceding page order. Its large native capture is local
evidence. Reproduce with `tools/native_runner.py INPUT OUTPUT --expected-pages 521
--collect-notebook`, then `PYTHONPATH=tools python tools/verify_page_creation.py
INPUT OUTPUT`. That comparison permits only exact native materialization of a
previously absent section metadata copy; all other graph fields compare exactly.

This core operation deliberately creates one optional title outline without
generated date/time text or template content. MS-ONE 2.2.50 permits a title with
one or two outlines. The fixture establishes this explicit empty-page contract.
