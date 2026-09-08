# Original schema-4 offline state

These databases were produced by `onestore-offline` at revision `2244711b`,
before the external-media table and schema-5 migration existed. Tests copy the
live database before opening it; the archive remains read-only.

[`producer.rs`](producer.rs) records the original producer. Run it against that
revision's core/offline crates, from a checkout whose `corpus/offline-v4` does
not exist. It uses the public native external-asset fixture, queues a text edit,
simulates a publication with a lost response, queues a dependent text edit, and
exports the complete recovery archive. Generated revision IDs vary between runs.

[`provenance.json`](provenance.json) records the original source/database hashes
and the retained uncertain and pending edit IDs. No credentials or personal
notebook content are included.
