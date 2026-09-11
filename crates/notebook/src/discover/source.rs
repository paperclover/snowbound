use super::{Entry, EntryKind, Source, component};
use std::{
    io,
    path::{Path, PathBuf},
};

/// A canonical local root; symbolic-link entries are not traversed.
pub struct Local {
    root: PathBuf,
}

impl Local {
    pub fn open(root: impl AsRef<Path>) -> io::Result<Self> {
        let root = root.as_ref().canonicalize()?;
        if !root.is_dir() {
            return Err(io::ErrorKind::NotADirectory.into());
        }
        Ok(Self { root })
    }

    fn path(&self, relative: &str) -> io::Result<PathBuf> {
        if !relative.is_empty() && !relative.split('/').all(component) {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        let path = self.root.join(relative).canonicalize()?;
        if !path.starts_with(&self.root) {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        Ok(path)
    }
}

impl Source for Local {
    fn entries(&mut self, path: &str, limit: usize) -> io::Result<Vec<Entry>> {
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(self.path(path)?)? {
            let entry = entry?;
            if entries.len() == limit {
                return Err(io::ErrorKind::FileTooLarge.into());
            }
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| io::ErrorKind::InvalidData)?;
            let kind = entry.file_type()?;
            entries.push(Entry {
                name,
                kind: if kind.is_dir() {
                    EntryKind::Directory
                } else if kind.is_file() {
                    EntryKind::File
                } else {
                    EntryKind::Other
                },
            });
        }
        Ok(entries)
    }

    fn read(&mut self, path: &str, limit: usize) -> io::Result<Vec<u8>> {
        onestore::read_file_limited(self.path(path)?, limit)
    }
}

#[cfg(feature = "smb")]
/// A share-relative root on an existing blocking SMB connection.
pub struct Smb<'a> {
    client: &'a crate::smb::Client,
    root: String,
}

#[cfg(feature = "smb")]
impl<'a> Smb<'a> {
    pub fn new(client: &'a crate::smb::Client, root: &str) -> io::Result<Self> {
        let root = root.replace('\\', "/");
        if !root.is_empty() && !root.split('/').all(component) {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        Ok(Self { client, root })
    }

    fn path(&self, relative: &str) -> io::Result<String> {
        if !relative.is_empty() && !relative.split('/').all(component) {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        Ok(if relative.is_empty() {
            self.root.clone()
        } else {
            super::join(&self.root, relative)
        })
    }
}

#[cfg(feature = "smb")]
impl Source for Smb<'_> {
    fn entries(&mut self, path: &str, limit: usize) -> io::Result<Vec<Entry>> {
        Ok(self
            .client
            .read_dir(&self.path(path)?, limit)?
            .into_iter()
            .map(|entry| Entry {
                name: entry.name,
                kind: if entry.attributes & 0x400 != 0 {
                    EntryKind::Other
                } else if entry.attributes & 0x10 != 0 {
                    EntryKind::Directory
                } else {
                    EntryKind::File
                },
            })
            .collect())
    }

    fn read(&mut self, path: &str, limit: usize) -> io::Result<Vec<u8>> {
        self.client.read_storage(&self.path(path)?, limit)
    }

    fn read_asset(&mut self, path: &str, limit: usize) -> io::Result<Vec<u8>> {
        self.client.read_asset(&self.path(path)?, limit)
    }
}
