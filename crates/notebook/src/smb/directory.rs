use super::*;
use smb2::msg::query_directory::{
    FileInformationClass, QueryDirectoryFlags, QueryDirectoryRequest, QueryDirectoryResponse,
};
use std::collections::BTreeMap;

mod records;
pub use records::DirectoryEntry;
use records::decode;

impl Client {
    /// Enumerates a share-relative directory completely or returns an error without a partial list.
    /// An empty path selects the share root. The limit excludes `.` and `..`.
    /// Concurrent directory changes are not an atomic snapshot; repeated names return ResourceBusy.
    pub fn read_dir(&self, path: &str, limit: usize) -> io::Result<Vec<DirectoryEntry>> {
        if path.contains('\0') || path.encode_utf16().count() > 32767 {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        let response: CreateResponse = self.request(
            Command::Create,
            CreateRequest {
                requested_oplock_level: OplockLevel::None,
                impersonation_level: ImpersonationLevel::Impersonation,
                desired_access: FileAccessMask::new(0x80000000),
                file_attributes: 0,
                share_access: ShareAccess(7),
                create_disposition: CreateDisposition::FileOpen,
                create_options: 0x1,
                name: smb2::encode_path(&path.replace('\\', "/")),
                create_contexts: Vec::new(),
            },
        )?;
        let file = File::new(self, &response);
        let mut entries = BTreeMap::new();
        loop {
            let response: io::Result<QueryDirectoryResponse> =
                self.request_with(Command::QueryDirectory, |connection| {
                    (
                        QueryDirectoryRequest {
                            file_information_class: FileInformationClass::FileDirectoryInformation,
                            flags: QueryDirectoryFlags(if entries.is_empty() { 1 } else { 0 }),
                            file_index: 0,
                            file_id: file.id.unwrap(),
                            output_buffer_length: connection
                                .params()
                                .expect("connected SMB session is negotiated")
                                .max_transact_size
                                .min(65536),
                            file_name: "*".into(),
                        },
                        CreditCharge(1),
                    )
                });
            let response = match response {
                Ok(response) => response,
                Err(error)
                    if matches!(
                        error.get_ref().and_then(|error| error.downcast_ref::<smb2::Error>()),
                        Some(smb2::Error::Protocol { status, command: Command::QueryDirectory })
                            if status.0 == 0x80000006 || (entries.is_empty() && status.0 == 0xc000000f)
                    ) =>
                {
                    break;
                }
                Err(error) => return Err(error),
            };
            for entry in decode(&response.output_buffer)? {
                if entries
                    .insert(entry.name, (entry.size, entry.modified, entry.attributes))
                    .is_some()
                {
                    return Err(io::ErrorKind::ResourceBusy.into());
                }
                if entries.len()
                    - usize::from(entries.contains_key("."))
                    - usize::from(entries.contains_key(".."))
                    > limit
                {
                    return Err(io::ErrorKind::FileTooLarge.into());
                }
            }
        }
        file.close()?;
        Ok(entries
            .into_iter()
            .filter(|(name, _)| name != "." && name != "..")
            .map(|(name, (size, modified, attributes))| DirectoryEntry {
                name,
                size,
                modified,
                attributes,
            })
            .collect())
    }

    /// Watches the directory tree at `path` as OneNote 2010 watches a notebook's folder: one
    /// CHANGE_NOTIFY with WATCH_TREE, kept armed on the server, so an idle watch sends
    /// nothing. `changed` runs on the client's runtime with each batch of changed paths,
    /// relative to `path` and `/`-separated, or `""` when the server lost count of them, then
    /// `Err(_)` once the watch ends with its connection or the client.
    pub fn watch(
        &self,
        path: &str,
        changed: impl FnMut(io::Result<Vec<String>>) + Send + 'static,
    ) -> io::Result<()> {
        if Handle::try_current().is_ok() {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| io::ErrorKind::Other)?
            .clone()
            .ok_or(io::ErrorKind::NotConnected)?;
        // Probing a connection whose only request is the watch would end OneNote's silence.
        connection.set_keepalive(None);
        let runtime = self.runtime.lock().map_err(|_| io::ErrorKind::Other)?;
        let runtime = runtime.as_ref().ok_or(io::ErrorKind::NotConnected)?;
        let path = path.replace('\\', "/");
        let mut watcher = runtime
            .block_on(async {
                tokio::time::timeout(self.timeout, self.tree.watch(&mut connection, &path, true))
                    .await
            })
            .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))?
            .map_err(|error| match &error {
                smb2::Error::Protocol { status, .. } if status.0 == 0xc0000034 => {
                    io::Error::new(io::ErrorKind::NotFound, error)
                }
                // STATUS_NOT_SUPPORTED, STATUS_INVALID_DEVICE_REQUEST
                smb2::Error::Protocol { status, .. }
                    if matches!(status.0, 0xc00000bb | 0xc0000010) =>
                {
                    io::Error::new(io::ErrorKind::Unsupported, error)
                }
                _ => super::io_error(error),
            })?;
        // However the task ends, the connection failing or the client retiring its runtime,
        // `changed` hears that the watch did.
        struct Ends<F: FnMut(io::Result<Vec<String>>)>(F);
        impl<F: FnMut(io::Result<Vec<String>>)> Drop for Ends<F> {
            fn drop(&mut self) {
                (self.0)(Err(io::ErrorKind::NotConnected.into()));
            }
        }
        let mut ends = Ends(changed);
        runtime.spawn(async move {
            loop {
                match watcher.next_events().await {
                    Ok(events) => {
                        (ends.0)(Ok(events.into_iter().map(|event| event.filename).collect()))
                    }
                    Err(smb2::Error::Protocol { status, .. }) if status.0 == 0x0000010c => {
                        (ends.0)(Ok(vec![String::new()]))
                    }
                    Err(_) => return,
                }
            }
        });
        Ok(())
    }
}
