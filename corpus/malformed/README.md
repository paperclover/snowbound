`native-inflight.one` was read without exclusion during OneNote 2010's shared
conflict save. Its committed revision references an object space that was not
yet present. The eventual locked snapshot is valid and lives in
`../collaboration/round-01/live/notebook/`. This intermediate file is a rejection
fixture, not an accepted notebook. The commit test checks that even a no-op edit
fails graph validation before any storage I/O.
