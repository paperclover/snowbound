# Offline page batches merged as page series

`../offline-edits` regenerated since page lists merge as OneNote merges page series
(`../../conflict-page/native-pages`): a page the remote moved keeps the remote's place.
In every native movement state OneNote gave the pages the offline batch places series of
their own, so all eighteen keep OneNote's placement and publish the body edit alone.
`twelve-clients` moves disjoint pages, each into a series of its own; each client's
batch publishes whole, twelve publications, all merged.

Every candidate reopened in a fresh OneNote 2010 clone without changes beyond the header,
except two native repairs of inputs OneNote wrote: `indent-02-promoted-parent` refreshes the
stale section metadata level that `../movement/02-promoted-parent` left (MS-ONE 2.2.81,
note 9), and `twelve-clients` fills the seed page's absent optional metadata copy. The
verifier accounts for both. `provenance.json` records the captures (`evidence/series`) and
removed clones.

```sh
ONESTORE_OFFLINE_PAGE_EDIT_OUTPUT=/tmp/one-page-edits cargo test -p notebook --all-features --test sync_pages native_page_changes_reconcile_with_atomic_offline_batches -- --exact
python tools/offline_page_edits.py /tmp/one-page-edits /tmp/one-page-edits-cold --workers 4
ONESTORE_OFFLINE_PAGE_CLIENT_OUTPUT=/tmp/one-page-clients cargo test -p notebook --all-features --test sync_pages twelve_disjoint_page_batches_merge_with_dependent_bodies_without_review -- --exact
python tools/native_runner.py /tmp/one-page-clients /tmp/one-page-clients-cold --expected-pages 37 --collect-notebook
```
