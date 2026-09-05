#![forbid(unsafe_code)]
#![doc = include_str!("../../../README.md")]

mod bytes;
mod commit;
mod create;
pub mod document;
mod edit;
mod files;
mod flush;
mod objects;
mod properties;
mod revisions;
mod store;
mod write;

pub use commit::{CommitError, CommitIo, CommitState, commit_property_bytes, commit_text};
#[cfg(any(unix, windows))]
pub use commit::{commit_file_property, commit_file_text, read_file};
pub use create::{create_section, create_table_of_contents};
pub use edit::replace_text;
pub use files::FileDataReference;
pub use objects::{Object, ObjectData, ObjectReferences, ResolvedRevision};
pub use properties::{IdStream, Property, PropertySets, Value};
pub use revisions::{ExGuid, ObjectSpace, Revision, RevisionIndex};
pub use store::{Chunk, Error, FileType, Header, Node, NodeList, Reference, Store};
pub use write::replace_property_bytes;
