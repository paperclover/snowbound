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
    fn stamp(&mut self) -> io::Result<Option<Stamp>> {
        Ok(Stamp::of(&self.visible).ok())
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
    fn confirm(&mut self, snapshot: &[u8]) -> Result<(), CommitError> {
        self.confirmations += 1;
        if matches!(self.fault, Fault::Confirm) {
            self.fault = Fault::None;
            return Err(failure(CommitState::Unknown));
        }
        onestore::confirm_snapshot(self, snapshot)?;
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
