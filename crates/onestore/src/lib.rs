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
mod outline;
pub mod page;
mod pages;
mod paragraph;
mod properties;
#[cfg(feature = "protected")]
pub mod protected;
mod revisions;
mod snapshot;
mod store;
mod toc;
mod tree;
mod write;

pub use commit::{
    CommitError, CommitIo, CommitState, PreparedEdit, commit_property_bytes, commit_text,
    confirm_snapshot,
};
#[cfg(any(unix, windows))]
pub use commit::{
    commit_file_property, commit_file_text, confirm_file_snapshot, place_file, read_file,
    read_file_limited,
};
pub use create::{create_section, create_table_of_contents};
pub use edit::replace_text;
pub use files::FileDataReference;
pub use formatting::TextAttribute;
pub use insertion::Insertion;
pub use objects::{Object, ObjectData, ObjectReferences, ResolvedRevision};
pub use outline::OutlineEdit;
pub use pages::{PageCreation, PageEdit, PagePosition};
pub use paragraph::{ParagraphJoin, ParagraphSplit};
pub use properties::{IdStream, Property, PropertySets, Value};
pub use revisions::{ExGuid, ObjectSpace, Revision, RevisionIndex};
pub use snapshot::{read_snapshot, read_storage_snapshot};
pub use store::{Chunk, Error, FileType, Header, Node, NodeList, Reference, Store};
pub use toc::TocEdit;
pub use tree::TreeEdit;
pub use write::replace_property_bytes;
