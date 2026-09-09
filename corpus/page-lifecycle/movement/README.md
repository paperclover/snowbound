# Native page movement controls

Nine OneNote 2010 reference outcomes from the synthetic `04-nested` section.
Every outcome has a closed notebook, native page XML, an independent cold reopen,
and saved desktop actions/screenshots. `provenance.json` records the controllers,
source, machines and teardown. Identical files link to one stored copy.

The controls distinguish three operations:

- Moving an expanded, individually selected tab moves that page alone. Its
  following subpages retain their levels and join the preceding series.
- Moving an explicitly selected range moves every selected page. Moving a
  collapsed top-level tab also moves its hidden subpages.
- Promotion/demotion changes only the selected page's level. A level-3 page can
  directly follow a level-1 page; indentation is not a conventional subtree.

`tools/native_page_movement.py OUTPUT` replays the eight numbered phases in an
owned 800×600 clone. The `single-page` reference comes from an earlier replay
whose group-movement expectation was rejected; its actual single-page outcome
was retained and independently validated. Its exact desktop actions are saved
alongside the notebook. The subsequent ordinary-click replay reproduced it.
Cold-capture each numbered phase with `tools/native_runner.py INPUT OUTPUT
--expected-pages 9 --collect-notebook`.

Run `PYTHONPATH=tools python3 -m unittest tools.test_page_movement -v` after
building the document example. The test verifies stable page identities, order,
levels, series membership, content-tree relationships, text and formatting,
then compares native XML and all active graphs across each cold reopen.

In `02-promoted-parent`, native OneNote left one section metadata copy at level 2
while the authoritative page metadata was level 1. Its cold reopen refreshed
that copy to 1. MS-ONE §2.2.81 product-behavior footnote 9 explicitly permits
temporary differences and describes subsequent native refresh. This fixture
accepts exactly one such level refresh; every other graph field must match.
The default comparison still rejects this change and remains strict for Rust
publication fixtures.
