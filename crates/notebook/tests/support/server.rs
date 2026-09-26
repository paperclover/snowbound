//! A fault-injecting in-memory remote for reconciliation tests.
#![allow(dead_code)]

use notebook::Remote;
use onestore::{
    CommitError, CommitIo, CommitState, ExGuid, RevisionIndex, Stamp, Store, Transaction,
    document::{Document, Kind},
};
use std::io;

#[derive(Clone, Copy, Default)]
pub enum Fault {
    #[default]
    None,
    Before,
    UnknownBefore,
    UnknownAfter,
    Committed,
    PanicBefore,
    PanicAfter,
    Confirm,
    ConfirmCommitted,
}

pub struct Server {
    pub visible: Vec<u8>,
    pub durable: Vec<u8>,
    pub fault: Fault,
    pub publications: usize,
    pub confirmations: usize,
}

impl Server {
    pub fn new(source: &[u8]) -> Self {
        Self {
            visible: source.to_vec(),
            durable: source.to_vec(),
            fault: Fault::None,
            publications: 0,
            confirmations: 0,
        }
    }
}

pub fn failure(state: CommitState) -> CommitError {
    CommitError {
        state,
        error: io::Error::from(io::ErrorKind::ConnectionAborted),
    }
}

impl CommitIo for Server {
    fn read_at(&mut self, offset: u64, output: &mut [u8]) -> io::Result<usize> {
        let offset = usize::try_from(offset).unwrap();
        let size = output.len().min(self.visible.len().saturating_sub(offset));
        if size > 0 {
            output[..size].copy_from_slice(&self.visible[offset..offset + size]);
        }
        Ok(size)
    }
    fn write_at(&mut self, offset: u64, bytes: &[u8]) -> io::Result<usize> {
        let offset = usize::try_from(offset).unwrap();
        self.visible
            .resize(self.visible.len().max(offset + bytes.len()), 0);
        self.visible[offset..offset + bytes.len()].copy_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        self.durable.clone_from(&self.visible);
        Ok(())
    }
}

impl Remote for Server {
    fn read(&mut self) -> io::Result<Vec<u8>> {
        Ok(self.visible.clone())
    }
    fn stamp(&mut self) -> io::Result<Stamp> {
        Stamp::of(&self.visible).map_err(io::Error::other)
    }
    fn publish(&mut self, transaction: &Transaction) -> Result<(), CommitError> {
        self.publications += 1;
        let fault = std::mem::take(&mut self.fault);
        match fault {
            Fault::Before => return Err(failure(CommitState::NotCommitted)),
            Fault::UnknownBefore => return Err(failure(CommitState::Unknown)),
            Fault::PanicBefore => panic!("Terminated before remote I/O"),
            _ => {}
        }
        let old = self.durable.clone();
        transaction.commit(self)?;
        match fault {
            Fault::UnknownAfter => {
                self.durable = old;
                Err(failure(CommitState::Unknown))
            }
            Fault::Committed => Err(failure(CommitState::Committed)),
            Fault::PanicAfter => panic!("Terminated after remote publication"),
            _ => Ok(()),
        }
    }
    fn confirm(&mut self, base: &Stamp) -> Result<(), CommitError> {
        self.confirmations += 1;
        if matches!(self.fault, Fault::Confirm) {
            self.fault = Fault::None;
            return Err(failure(CommitState::Unknown));
        }
        onestore::confirm(self, base)?;
        if matches!(self.fault, Fault::ConfirmCommitted) {
            self.fault = Fault::None;
            return Err(failure(CommitState::Committed));
        }
        Ok(())
    }
}

pub fn text(source: &[u8]) -> (ExGuid, ExGuid, String) {
    let store = Store::parse(source).unwrap();
    assert!(store.checksum_mismatches.is_empty());
    let index = RevisionIndex::parse(&store).unwrap();
    index.validate_current().unwrap();
    let doc = Document::parse(&index).unwrap();
    doc.spaces
        .iter()
        .find_map(|(sid, space)| {
            space.revisions[&space.contexts[&ExGuid::default()]]
                .nodes
                .iter()
                .find_map(|(oid, node)| match &node.kind {
                    Kind::RichText { text, .. } => Some((*sid, *oid, text.clone())),
                    _ => None,
                })
        })
        .unwrap()
}

/// Every page of an image, in section order: what two images holding the same edits under
/// different revision identities share.
pub fn pages(source: &[u8]) -> Vec<(ExGuid, onestore::page::Page)> {
    let store = Store::parse(source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    document
        .pages()
        .unwrap()
        .into_iter()
        .map(|(space, _)| {
            (
                space,
                onestore::page::Page::from_space(&document, space).unwrap(),
            )
        })
        .collect()
}

/// Queues a section op as its own edit.
pub fn section_op(cache: &notebook::Replica, op: onestore::op::SectionOp) -> u64 {
    cache
        .apply(
            "Fixture",
            onestore::op::Edit {
                at: 133_000_000_000_000_000,
                ops: vec![onestore::op::Op::Section(op)],
            },
        )
        .unwrap()
}

/// The image the queue leaves, as a recovery archive of the cache records it.
pub fn snapshot(cache: &notebook::Replica) -> Vec<u8> {
    archived(cache, |recovery| recovery.snapshot())
}

/// The remote image the cache last observed, as a recovery archive records it.
pub fn remote_snapshot(cache: &notebook::Replica) -> Vec<u8> {
    archived(cache, |recovery| recovery.remote_snapshot())
}

fn archived(
    cache: &notebook::Replica,
    image: impl FnOnce(&notebook::Recovery) -> Result<Vec<u8>, notebook::Error>,
) -> Vec<u8> {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("recovery.sqlite");
    cache.export_recovery(&path).unwrap();
    image(&notebook::Recovery::open(&path).unwrap()).unwrap()
}

/// `image` after another writer replaces `range` of the text object `text` with `with`.
pub fn typed(
    image: &[u8],
    space: ExGuid,
    text: ExGuid,
    range: std::ops::Range<u32>,
    with: &str,
) -> Vec<u8> {
    let op = onestore::op::PageOp::Text {
        text,
        range,
        with: with.into(),
    };
    edited(image, vec![onestore::op::Op::Page { space, op }]).unwrap()
}

/// `image` after another writer's `ops`, sealed as one revision per space they change.
pub fn edited(image: &[u8], ops: Vec<onestore::op::Op>) -> Result<Vec<u8>, onestore::op::OpError> {
    let arena = onestore::Arena::default();
    let mut section =
        onestore::Section::open(&arena, image.to_vec()).map_err(onestore::op::OpError::Failed)?;
    let edit = onestore::op::Edit {
        at: 134_000_000_000_000_000,
        ops,
    };
    section.apply("Other writer", &edit)?;
    section.seal().map_err(onestore::op::OpError::Failed)?;
    Ok(section.image())
}

/// A conflict page as the tests compare it: whose version it is and its texts.
pub type Version = (String, Vec<String>);

/// Each page's conflict pages in list order.
pub fn conflicts(image: &[u8]) -> Vec<(ExGuid, Vec<Version>)> {
    let arena = onestore::Arena::default();
    let mut section = onestore::Section::open(&arena, image.to_vec()).unwrap();
    section
        .conflicts()
        .unwrap()
        .into_iter()
        .map(|(page, conflicts)| {
            let conflicts = conflicts
                .into_iter()
                .map(|conflict| {
                    let page = section.page(conflict.space).unwrap();
                    (conflict.user, page_texts(&page))
                })
                .collect();
            (page, conflicts)
        })
        .collect()
}

/// The texts of a page's outlines and title, in page order.
pub fn page_texts(page: &onestore::page::Page) -> Vec<String> {
    use onestore::page::PageObject;
    page.objects
        .iter()
        .flat_map(|object| match object {
            PageObject::Outline(outline) => outline.paragraphs.clone(),
            PageObject::Title(title) => title
                .outlines
                .iter()
                .flat_map(|outline| outline.paragraphs.clone())
                .collect(),
            _ => Vec::new(),
        })
        .filter_map(|paragraph| Some(paragraph.text()?.text.text().to_owned()))
        .collect()
}

/// Whether `image` lists a conflict page under page `space`.
pub fn conflicted(image: &[u8], space: ExGuid) -> bool {
    conflicts(image).iter().any(|(page, _)| *page == space)
}
