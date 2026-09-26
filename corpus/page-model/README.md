# Page-model edits through a cold native reopen

`candidate` is `corpus/outline-edit/before` after eight compound page-model
publications, one per edited page, each combining several kinds of change in a single
transaction: paragraph reorder and Unicode replacement with a bold range; a subtree
move with a new nested child and an italic/colored/resized range; a subtree deletion
with an appended paragraph and a new two-paragraph outline; a mid-paragraph split; a
collapse with an outline move and fixed width; an outline reorder with text appended
in both; an outline deletion with underline/highlight/font changes; and a paragraph
join with strike/subscript. `candidate/manifest.json` records each case, the page's
position in the section and the model the writer read back.

`cold` is an independent OneNote 2010 capture of the candidate through a fresh
application cache, with the notebook retained after the native open. `followup`
opens `cold/notebook` in another fresh clone and replaces the Rust-written Unicode
paragraph on "Move leaf down" natively (`tools/native/page-model-followup.ps1`);
`followup-cold` reopens that result cold.

`PYTHONPATH=tools python3 -m unittest test_page_model -v` compares every page's
native XML with the Rust model of each notebook (text order, resolved formatting,
associated tags), requires the cold reopen to leave every page space's active
revision unchanged (OneNote only adds its per-series navigation metadata copies to
the section root on open, as it does for the untouched `outline-edit` fixture whose
first-page copy was already stale), and checks each case's expected outline positions, paragraph order,
nesting and collapse state in the candidate, the cold notebook and the follow-up.

To regenerate: `ONESTORE_PAGE_MODEL_OUTPUT=/absolute/new cargo test -p onestore
--test page_writer export_native_page_model_candidates -- --ignored`, then
`tools/native_runner.py /absolute/new/candidate/notebook /absolute/new/cold
--expected-pages 15 --collect-notebook`, then the same runner on `cold/notebook`
with `--author tools/native/page-model-followup.ps1` into `followup`, and a final
cold capture of `followup/notebook` into `followup-cold`. Each runner removes its clone.
