#![forbid(unsafe_code)]
#![doc = include_str!("../README.md")]

mod active;
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
pub mod op;
mod outline;
pub mod page;
mod pages;
mod paragraph;
mod properties;
#[cfg(feature = "protected")]
pub mod protected;
mod revisions;
mod section;
mod snapshot;
mod store;
#[cfg(test)]
#[path = "../tests/support/sweep.rs"]
mod sweep;
mod toc;
mod tree;
mod write;

pub use commit::{CommitError, CommitIo, CommitState, Stamp, Transaction, confirm, place};
#[cfg(any(unix, windows))]
pub use commit::{confirm_file, place_file, read_file, read_file_limited};
pub use create::{create_empty_section, create_section, create_table_of_contents};
pub use files::FileDataReference;
pub use formatting::{FONT_SIZES, TextAttribute};
pub(crate) use insertion::Insertion;
pub use objects::{Object, ObjectData, ObjectReferences, ResolvedRevision};
pub use outline::OutlineEdit;
pub use pages::{ConflictPage, PageCreation, PageEdit, PagePosition};
pub(crate) use paragraph::{ParagraphJoin, ParagraphSplit};
pub use properties::{IdStream, Property, PropertySets, Value};
pub use revisions::{ExGuid, ObjectSpace, Revision, RevisionIndex};
pub use section::{Arena, Section};
pub use snapshot::{read_snapshot, read_storage_snapshot};
pub use store::{Chunk, Error, FileType, Header, Node, NodeList, Reference, Store};
pub use toc::{TocEdit, edit_table_of_contents};
pub(crate) use tree::TreeEdit;
