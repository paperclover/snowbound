# onestore-notebook

Read-only notebook discovery over a caller-supplied root. This experimental
sibling crate keeps directory traversal out of the single-file storage parser.

Discovery returns ordered sections and nested groups with file identities and
share-relative paths. Section display-name overrides remain distinct from file
names. TOC references whose identities are absent from the directory remain
inspectable; a cached filename never substitutes for an identity match.
Reserved `_onefiles` directories are excluded from section-group traversal.
Encrypted sections and valid storage with an unreadable document graph retain
their identity as `Locked` or `Unreadable`, without being presented as empty pages.
Malformed storage, failed reads and ambiguous identities reject the discovery.

Each file read must be a consistent, bounded snapshot. The result is an
observation across multiple files, not an atomic notebook transaction or
authorization to publish an edit. Refresh rejects observed topology changes and
duplicate physical files with the same logical identity. Retain the last accepted
catalog if discovery fails; a connection failure does not mean files were deleted.

`read_external_asset` resolves a validated UUID `.onebin` filename beneath the
selected section's sibling `_onefiles` folder and returns exact bounded bytes.
Missing files, permissions and size failures retain their I/O error kinds; an
empty payload is a successful empty buffer. Embedded payloads remain available
directly from the core document model.
