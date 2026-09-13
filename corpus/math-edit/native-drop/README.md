# OneNote 2010 drops outline elements written with `{guid},0` identities

`dropped/candidate/four.one` is one of six identical-content builds of the
ten-equation page (`equations_are_written_and_read_back`); only this build
came back from OneNote 2010 with an empty outline element (`dropped/cold`:
the fifth equation `x_i^2` has no content, alignment or style). The object
bytes, identity tables, run data, styles, revision manifest and transaction
log are identical in shape to the builds that rendered, the drop is
deterministic for a given file, and across thirty builds written with the
model's `{random guid},0` identities seven lost one element each, always an
outline element, at varying positions. OneNote never stores an object as
`{guid},0` (its own objects are `{page guid},n` with `n` from 1); after
`page::text::new_id` moved to `n` 1, eighteen builds and the written
candidate (`../written`) rendered every paragraph.

`relocated/candidate` is the dropped build with only that element's 56-byte
data chunk copied to the end of the file and the declaration retargeted; it
rendered (`relocated/cold`), so the loss also depends on where the data
sits, which is consistent with the identity being mishandled in a
position-keyed structure rather than being rejected outright. The
transaction writer additionally keeps native files' 1 KiB reservation after
a transaction-log fragment that ends the file, so no object data abuts the
log.
