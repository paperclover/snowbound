use crate::Remote;
use onestore::{CommitError, PreparedEdit};
use onestore_smb::Client;
use std::io;

/// Binds every reconciliation operation to one share-relative file and read limit.
/// Connection loss retires the client; reconnect before subsequent sync attempts.
pub struct SmbRemote {
    client: Client,
    path: String,
    limit: usize,
}

impl SmbRemote {
    pub fn new(client: Client, path: impl Into<String>, limit: usize) -> Self {
        Self {
            client,
            path: path.into(),
            limit,
        }
    }
}

impl Remote for SmbRemote {
    fn read(&mut self) -> io::Result<Vec<u8>> {
        self.client.read(&self.path, self.limit)
    }

    fn publish(&mut self, edit: &PreparedEdit<'_>) -> Result<(), CommitError> {
        self.client.commit_prepared(&self.path, edit)
    }

    fn confirm(&mut self, snapshot: &[u8]) -> Result<(), CommitError> {
        self.client.confirm_snapshot(&self.path, snapshot)
    }
}
