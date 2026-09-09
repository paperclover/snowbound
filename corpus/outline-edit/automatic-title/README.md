# Automatic titles after outline movement

Three native pages distinguish vertical ordering, horizontal ordering and an
explicit title. Moving the second outline ahead of the first changes the two
automatic titles to `Second 🦋 é`; the explicit title remains unchanged.

`before` is the native source. `native` records the corresponding COM moves and
an independent cold reopen. `candidate` is the Rust edit; `cold` is its independent
OneNote reopen. Cold captures preserve exact active graphs. Rust movement retains
all text identities, content, formatting and history. Identical artifacts link
to one copy; `provenance.json` records the native runs and clone teardown.

The Rust regression originally left both automatic-title fields unchanged.
Position edits now update the existing page and metadata caches within the same
revision as the coordinates. The offline regression additionally changes the
remote title-producing text, loses the publication reply and reopens the cache
before confirmation and a dependent edit.

Run `tools.test_outline_edit.OutlineEditTest.test_automatic_titles_follow_native_and_rust_outline_movement`
through Python unittest after building the workspace examples with all features.
The Rust test `native_automatic_and_explicit_titles_follow_outline_movement` can
export a new candidate directory through `ONESTORE_OUTLINE_TITLE_OUTPUT`.

To regenerate the native controls, copy `fixture.json` and the synthetic section
from `corpus/native-delete/deletion-05/before` into a new input notebook directory.
Capture it with `tools/native_runner.py --author tools/native/pages.ps1
--collect-notebook`. Capture that result's notebook with
`--author tools/native/outline-title.ps1 --collect-notebook`, then independently
cold-open both the native result and the Rust candidate without an author script.
