# Conflict pages

What OneNote 2010 does when a sync finds that two clients changed the same object, and
that it reads and deletes the conflict pages Rust writes. `tools/native_conflict.py`
made every capture; `tools/test_conflict_page.py` checks them without a VM.

`native` is two OneNote clients on one Samba notebook (`native_conflict.py sync`, fixture
`native-ink/cold-ui-ink`): client A, offline, rewrote the first outline's paragraph and the
second outline's; client B rewrote the same first paragraph and the left table cell and
published (`b-published`); A reconnected and synced (`conflict`). A kept B's version of the
paragraph both changed and merged its own disjoint edit into the page; its own version of the
page became a conflict page:

- its own object space, listed only in the page manifest's `ChildGraphSpaceElementNodes`
  (`0x2c001d63`), never in the page series, with a copy of its metadata in the manifest's
  `0x24003442` whose identity is the space's XOR the metadata salt (MS-ONE 2.2.81);
- metadata root `jcidConflictPageMetaData` (`0x20038`): `CachedTitleString`, a fresh
  `NotebookManagementEntityGuid`, `PageLevel`, both schema revisions (`0x28`),
  `TopologyCreationTimeStamp` of the merge, `ConflictingUserName` and
  `ConflictingUserInitials` of the client whose version it is;
- a page node with `IsReadOnly`, `Deletable` and `IsConflictPage`, holding A's whole page;
  only the rich text of the paragraph both changed has `IsConflictObjectForSelection` and
  `IsConflictObjectForRender`;
- the page's metadata and the section's copy of it gain `HasConflictPages`.

Both clients show the same UI (`a/`, `b/`): the page's tab carries a conflict mark and a
yellow bar reads "This page has changes that could not be merged during synchronization.
Click here to show versions of the page with unmerged changes." Clicking it lists the
conflict pages under the page, each as its date and user ("9/26/2026 snow"), and opens the
first; that page's bar reads "Conflicting changes are highlighted in red. This page cannot be
edited, but you can copy changes to the primary page. Click here for more options." over a
light red highlight (RGB 255, 214, 214), and its menu offers Delete Conflict Page, Copy Page
To..., Select Previous/Next Conflicting Change and Collapse Conflict Pages.

`native-three` is `evidence/m8/conflict-abrupt-cold-01`: three clients' conflict pages under
one page. OneNote appends each to `ChildGraphSpaceElementNodes` and lists them last first
(`conflicts.png`); the metadata copies are in their own order.

`native-delete` is OneNote's Delete Conflict Page on `native/conflict` in a fresh clone
(`native_conflict.py cold --delete`): the manifest loses `ChildGraphSpaceElementNodes` (its
last entry) but keeps the metadata copy, the page's metadata and the section's copy lose
`HasConflictPages`, and the conflict page's space is left as it was.

`native-pages` and `native-restore` are page-list merges (`native_conflict.py pages`,
`restore`): client A, offline, moved pages through COM and edited `Target`'s body while
client B moved pages and deleted `Target`; A reconnected (`merged`, both clients' `listed.json`
and screenshots). OneNote merges page series, not page positions. A page a client moves
gets a series of its own, and a COM move also re-series every page it jumps over (a drag
re-series only the dragged pages, `page-lifecycle/movement`). The merge keeps each page in
the series of the client that re-seriesed it, the server's where both did, leaving the
other's series empty (`native-pages` keeps two, which MS-ONE's one-page minimum does not
allow); series only A made go after the server's. `Target` comes back as a new page space
holding A's edit, in a new series where A had it; B's version stays in the recycle bin.

- `native-pages`: from One, Two, Three, Four, Target, A moved Four before One and Two last
  (re-seriesing One, Two, Three), B moved Four last and Three before One (re-seriesing One,
  Two, Four): One, Two, Four, Three, Target.
- `native-restore`: from One, Two, Target, Three, Four, A moved Four before One
  (re-seriesing One, Two, Target, Three) and B deleted Target: Four, One, Two, Target, Three.

`page_moves_and_a_removed_page_merge_as_onenote_merges_them` in
`crates/notebook/tests/sync_pages.rs` replays both with Snowbound's queue, A's moves as
moves of the pages A re-seriesed, and reaches OneNote's order.

`candidate` is `a_conflict_page_is_stored_as_onenote_stores_one` in
`crates/onestore/tests/conflict.rs` (`ONESTORE_CONFLICT_EXPORT`): the remote renamed the
first word of the fixture's first paragraph, this machine did too, and `SectionOp::Conflict`
kept this machine's page for "Clover Snow", whose author objects carry her initials beside
her name as OneNote's do. `cold` is OneNote's fresh read and the same UI
walk, ending in Delete Conflict Page on the Rust-written page: its bar and list entry show,
the conflict page opens (window title) with the paragraph highlighted, and the deletion
stores what `native-delete` did.

`candidate-delete` is `deleting_a_conflict_page_stores_what_onenote_stores`
(`ONESTORE_CONFLICT_DELETE_EXPORT`): `SectionOp::Delete` of `native/conflict`'s conflict page,
which stores the objects OneNote's deletion did; `cold-delete` is its fresh read, without a
bar.
