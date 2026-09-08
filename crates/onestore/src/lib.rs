#![forbid(unsafe_code)]
#![doc = include_str!("../README.md")]

mod bytes;
mod commit;
mod create;
pub mod document;
mod edit;
mod files;
mod flush;
mod formatting;
mod insertion;
mod objects;
mod properties;
mod revisions;
mod snapshot;
mod store;
mod write;

pub use commit::{
    CommitError, CommitIo, CommitState, PreparedEdit, commit_property_bytes, commit_text,
    confirm_snapshot,
};
#[cfg(any(unix, windows))]
pub use commit::{commit_file_property, commit_file_text, read_file, read_file_limited};
pub use create::{create_section, create_table_of_contents};
pub use edit::replace_text;
pub use files::FileDataReference;
pub use formatting::TextAttribute;
pub use insertion::Insertion;
pub use objects::{Object, ObjectData, ObjectReferences, ResolvedRevision};
pub use properties::{IdStream, Property, PropertySets, Value};
pub use revisions::{ExGuid, ObjectSpace, Revision, RevisionIndex};
pub use snapshot::{read_snapshot, read_storage_snapshot};
pub use store::{Chunk, Error, FileType, Header, Node, NodeList, Reference, Store};
pub use write::replace_property_bytes;
