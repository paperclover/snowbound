# Native page lifecycle controls

Six sequential OneNote 2010 controls, each independently reopened in a fresh
Windows clone. `notebook` is the closed source; `read` is its native section XML;
`cold/notebook` and `cold/read` retain the independently reopened notebook and XML.
Identical artifacts link to one copy. `provenance.json` records the source,
controller hash and clone teardown. The seed section is the existing synthetic
deletion fixture; personal notebooks are not inputs.

| Phase | Native operation |
| --- | --- |
| 01-blank | Create default, blank-with-title and blank-without-title pages. |
| 02-authored | Add six pages, including duplicate explicit titles and a body-derived title. |
| 03-renamed | Rename one duplicate with Unicode; clear another title. |
| 04-nested | Place child/grandchild pages at levels 2 and 3. |
| 05-reordered | Move an ordinary page before the parent/subpage group using complete explicit order. |
| 06-deleted-parent | Soft-delete the parent while retaining its subpages. |

The native deletion moves the parent into `OneNote_RecycleBin` and appends its
surviving level-2/3 pages to the preceding page series. These COM controls do not
define keyboard selection or implicit subtree movement. Renaming reallocates
title content; body identities, formatting and content remain unchanged.

Run `PYTHONPATH=tools python3 -m unittest tools.test_page_lifecycle -v` after
building the workspace examples with all features. The tests compare every phase
to its native XML and cold output, then independently check page styles, titles,
order, levels, series grouping and exact preserved body graphs.

To reproduce, copy `corpus/native-delete/deletion-05/before/synthetic.one` into a
new notebook directory and run `tools/native_runner.py INPUT OUTPUT --author
tools/native/page-lifecycle.ps1 --collect-notebook`. Cold-capture each generated
phase's notebook with the same runner, without `--author`. The controller waits
for stable native content before closing and refreshes COM IDs after reopening;
COM IDs are not the stored object-space identities.
