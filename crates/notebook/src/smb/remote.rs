use super::Client;
use crate::Remote;
use onestore::{CommitError, Stamp, Transaction};
use std::{io, sync::Arc};

/// Binds every reconciliation operation to one share-relative file and read limit.
/// Connection loss retires the client; reconnect before subsequent sync attempts.
pub struct SmbRemote {
    client: Arc<Client>,
    path: String,
    limit: usize,
}

impl SmbRemote {
    /// A client shared between remotes serves each of their files over one connection.
    pub fn new(client: impl Into<Arc<Client>>, path: impl Into<String>, limit: usize) -> Self {
        Self {
            client: client.into(),
            path: path.into(),
            limit,
        }
    }
}

impl Remote for SmbRemote {
    fn read(&mut self) -> io::Result<Vec<u8>> {
        self.client.read(&self.path, self.limit)
    }

    fn stamp(&mut self) -> io::Result<Stamp> {
        self.client.stamp(&self.path)
    }

    fn publish(&mut self, transaction: &Transaction) -> Result<(), CommitError> {
        self.client.commit_transaction(&self.path, transaction)
    }

    fn confirm(&mut self, base: &Stamp) -> Result<(), CommitError> {
        self.client.confirm(&self.path, base)
    }
}
