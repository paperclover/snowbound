# Atomic Rust page edits

Nine Rust-generated outcomes corresponding to the [native movement controls](../movement/README.md).
Each candidate was independently opened in a fresh OneNote 2010 clone. Cold output
preserves every byte beyond the file header and every active object graph without
metadata repair. `provenance.json` records the generator, inputs and removed clones.

Run `cargo test -p onestore --all-features --test page_movement` for native semantic
comparisons, exact page-content/history preservation, invalid intents, and interrupted
publication. Set `ONESTORE_PAGE_MOVEMENT_OUTPUT` to an absolute output directory to
export the nine candidate states. Then use `tools/native_runner.py INPUT OUTPUT
--expected-pages 9 --collect-notebook` for fresh independent captures.

`PYTHONPATH=tools python3 -m unittest tools.test_page_movement -v` compares these
committed fixtures with native XML and the complete cold-reopened graphs.

`stress` retains 520 alternating nest/promote actions across counter carries and
revision checkpoints. Its final two-page file also reopened without changes beyond
the header. Set `ONESTORE_PAGE_MOVEMENT_STRESS_OUTPUT` to an absolute directory when
running the Rust checkpoint test to export another stress file.

`native` records OneNote renaming a moved page with a Unicode title and adding a
body outline, followed by a cold reopen. `rust-followup` nests that page and edits
the new native paragraph, then reopens it in OneNote again. Every original page
identity and existing text remains accounted for across both writers.

`optional-cache` removes an optional series metadata reference while retaining
the old objects in history. Native OneNote restores those references in a different
order from the pages. Rust preserves metadata identity through restoration and
movement; the regression matches copies by their page identifiers. The unit test
also retains an unknown property and rejects detached edits without reattachment.
