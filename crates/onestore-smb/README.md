# onestore-smb

Optional blocking SMB access for OneNote sections and table-of-contents files.
The core `onestore` crate remains independent of network runtimes. This is an
experimental Rust API with native interoperability evidence in the repository's
[Milestone 9](../../evidence/MILESTONE9.md).

```no_run
use onestore_smb::{Client, Credentials};
use std::time::Duration;

let client = Client::connect(
    "server:445",
    "notes",
    Credentials { username: "user", password: "password", domain: "" },
    Duration::from_secs(5),
)?;
let snapshot = client.read("Personal/Video.one", 64 * 1024 * 1024)?;
let store = onestore::Store::parse(&snapshot)?;
let revisions = onestore::RevisionIndex::parse(&store)?;
let document = onestore::document::Document::parse(&revisions)?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

Paths are relative to the share. The read limit bounds the complete physical
snapshot. Call from a background thread outside a Tokio runtime. Use identities
from the document and the same snapshot with `Client::commit_text` or
`Client::commit_property_bytes`; their errors retain `onestore::CommitState`.
`PreparedEdit::{text,insert,format}` separate preparation from I/O: inspect the immutable image
and persist the intended revision identity before `Client::commit_prepared`.
`Client::confirm_snapshot` compares and flushes an observed image, then refreshes
its header version metadata without adding a revision. The caller must first
establish which intents that image contains and reread before another commit.

Readers use shared native guards while writers publish under native write-open
and byte-lock exclusion. Maintenance is excluded during each operation; pathname
identity is checked after acquiring the guards. Connection loss retires the
client. Reconnect for subsequent operations, and reconcile an `Unknown` edit
before retrying it. The transport does not automatically replay requests.

`Client::read_dir(path, entry_limit)` enumerates a directory, including the share
root with an empty path. It follows every response page and returns no partial
list on interruption, entry-limit overflow or close failure. Entries retain exact
Unicode names, observed sizes and MS-FSCC attributes, including directory/reparse
flags. Concurrent directory changes are not an atomic snapshot; repeated names
are rejected with `ResourceBusy`. Notebook identities come from the files, not
directory names or sizes. Missing paths, denied access and non-directory paths
have distinct I/O error kinds.

`python3 tools/test_smb_directory.py VM OUTPUT` checks a caller-owned disposable
Linux lab VM against its filesystem listing and interrupts directory requests,
responses and close. It creates synthetic files in that VM; the caller retains
responsibility for VM teardown. The parser also has bounded-record/truncation
tests independent of the server.

Device and simulator builds link for iOS. Native acceptance uses disposable
OneNote 2010 clients and Samba; it does not establish on-device execution or
physical power-loss durability.
