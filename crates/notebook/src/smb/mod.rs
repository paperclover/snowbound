//! Blocking SMB access to OneNote sections, table-of-contents files and payloads.

use onestore::{CommitError, CommitIo, CommitState, ExGuid};
use smb2::{
    Session, Tree,
    client::connection::{Connection, NegotiatedParams},
    msg::{
        close::{CloseRequest, CloseResponse},
        create::{
            CreateDisposition, CreateRequest, CreateResponse, ImpersonationLevel, ShareAccess,
        },
        flush::{FlushRequest, FlushResponse},
        lock::{LockElement, LockRequest, LockResponse},
        query_info::{InfoType, QueryInfoRequest, QueryInfoResponse},
        read::{ReadRequest, ReadResponse},
        write::{WriteRequest, WriteResponse},
    },
    pack::{Pack, ReadCursor, Unpack},
    types::{
        Command, CreditCharge, Dialect, FileId, OplockLevel,
        flags::{Capabilities, FileAccessMask},
    },
};
use std::{io, ops::Range, sync::Mutex, time::Duration};
use tokio::runtime::{Handle, Runtime};

mod directory;
mod remote;
pub use directory::DirectoryEntry;
pub use remote::SmbRemote;

#[derive(Default)]
pub struct Credentials<'a> {
    pub username: &'a str,
    pub password: &'a str,
    pub domain: &'a str,
}

/// Blocking connection with no automatic request replay or cached file contents.
/// Call from a background thread outside a Tokio runtime.
/// Paths are relative to the share; both `/` and `\` are separators.
pub struct Client {
    connection: Mutex<Option<Connection>>,
    tree: Tree,
    timeout: Duration,
    runtime: Mutex<Option<Runtime>>,
}

impl Client {
    pub fn connect(
        address: &str,
        share: &str,
        credentials: Credentials<'_>,
        timeout: Duration,
    ) -> io::Result<Self> {
        if Handle::try_current().is_ok() || timeout.is_zero() {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()?;
        let (connection, tree) = runtime
            .block_on(async {
                tokio::time::timeout(timeout, async {
                    let mut connection = Connection::connect(address, timeout).await?;
                    connection.set_compression_requested(false);
                    connection.negotiate().await?;
                    Session::setup(
                        &mut connection,
                        credentials.username,
                        credentials.password,
                        credentials.domain,
                    )
                    .await?;
                    let tree = Tree::connect(&mut connection, share).await?;
                    Ok::<_, smb2::Error>((connection, tree))
                })
                .await
            })
            .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))?
            .map_err(io::Error::other)?;
        Ok(Self {
            connection: Mutex::new(Some(connection)),
            tree,
            timeout,
            runtime: Mutex::new(Some(runtime)),
        })
    }

    fn retire(&self) {
        if let Some(connection) = self
            .connection
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
        {
            connection.mark_dead();
        }
        if let Some(runtime) = self
            .runtime
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
        {
            runtime.shutdown_background();
        }
    }

    fn request<T: Unpack>(&self, command: Command, body: impl Pack) -> io::Result<T> {
        self.request_with(command, |_| (body, CreditCharge(1)))
    }

    fn request_with<T: Unpack, B: Pack>(
        &self,
        command: Command,
        prepare: impl FnOnce(&Connection) -> (B, CreditCharge),
    ) -> io::Result<T> {
        if Handle::try_current().is_ok() {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        let connection = self
            .connection
            .lock()
            .map_err(|_| io::ErrorKind::Other)?
            .clone()
            .ok_or(io::ErrorKind::NotConnected)?;
        let frame = {
            let runtime = self.runtime.lock().map_err(|_| io::ErrorKind::Other)?;
            let (body, charge) = prepare(&connection);
            runtime
                .as_ref()
                .ok_or(io::ErrorKind::NotConnected)?
                .block_on(async {
                    tokio::time::timeout(
                        self.timeout,
                        connection.execute_with_credits(
                            command,
                            &body,
                            Some(self.tree.tree_id),
                            charge,
                        ),
                    )
                    .await
                })
        };
        let frame = frame
            .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))
            .and_then(|result| result.map_err(io::Error::other))
            .inspect_err(|_| self.retire())?;
        if frame.header.command != command {
            self.retire();
            return Err(io::ErrorKind::InvalidData.into());
        }
        if frame.header.status.0 != 0 {
            let kind = match frame.header.status.0 {
                0xc0000043 | 0xc0000054 | 0xc0000055 => io::ErrorKind::WouldBlock,
                0xc0000011 => io::ErrorKind::UnexpectedEof,
                0xc0000034 | 0xc000003a => io::ErrorKind::NotFound,
                0xc0000022 => io::ErrorKind::PermissionDenied,
                0xc0000103 => io::ErrorKind::NotADirectory,
                _ => io::ErrorKind::Other,
            };
            return Err(io::Error::new(
                kind,
                smb2::Error::Protocol {
                    status: frame.header.status,
                    command,
                },
            ));
        }
        T::unpack(&mut ReadCursor::new(&frame.body)).map_err(|error| {
            self.retire();
            io::Error::new(io::ErrorKind::InvalidData, error)
        })
    }

    fn open(&self, path: &str, write: bool) -> io::Result<File<'_>> {
        self.open_shared(path, write, if write { 5 } else { 7 })
    }

    fn open_shared(&self, path: &str, write: bool, sharing: u32) -> io::Result<File<'_>> {
        if path.is_empty() || path.contains('\0') || path.encode_utf16().count() > 32767 {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        let response: CreateResponse = self.request(
            Command::Create,
            CreateRequest {
                requested_oplock_level: OplockLevel::None,
                impersonation_level: ImpersonationLevel::Impersonation,
                desired_access: FileAccessMask::new(if write { 0xc0000000 } else { 0x80000000 }),
                file_attributes: 0,
                share_access: ShareAccess(sharing),
                create_disposition: CreateDisposition::FileOpen,
                create_options: 0x42,
                name: smb2::encode_path(&path.replace('\\', "/")),
                create_contexts: Vec::new(),
            },
        )?;
        Ok(File {
            client: self,
            id: Some(response.file_id),
        })
    }

    /// Reads a bounded external payload while denying concurrent writes and deletion.
    /// Empty files succeed; limits, sharing contention and failed close return no payload.
    pub fn read_asset(&self, path: &str, limit: usize) -> io::Result<Vec<u8>> {
        let mut file = self.open_shared(path, false, 1)?;
        let mut bytes = Vec::new();
        let mut block = [0; 65536];
        loop {
            let count = (limit - bytes.len()).min(block.len() - 1) + 1;
            let read = file.read_at(
                u64::try_from(bytes.len()).map_err(|_| io::ErrorKind::InvalidInput)?,
                &mut block[..count],
            )?;
            if read == 0 {
                break;
            }
            bytes.extend_from_slice(&block[..read]);
            if bytes.len() > limit {
                return Err(io::ErrorKind::FileTooLarge.into());
            }
        }
        file.close()?;
        Ok(bytes)
    }

    /// Reads one bounded, consistent snapshot; contention returns WouldBlock.
    pub fn read(&self, path: &str, limit: usize) -> io::Result<Vec<u8>> {
        self.read_with(path, |file| {
            onestore::read_snapshot(|offset, output| file.read_at(offset, output), limit)
        })
    }

    /// Reads consistent storage, including encrypted or incomplete document graphs.
    /// Storage validation alone does not establish edit readiness.
    pub fn read_storage(&self, path: &str, limit: usize) -> io::Result<Vec<u8>> {
        self.read_with(path, |file| {
            onestore::read_storage_snapshot(|offset, output| file.read_at(offset, output), limit)
        })
    }

    fn read_with(
        &self,
        path: &str,
        snapshot: impl FnOnce(&mut File<'_>) -> io::Result<Option<Vec<u8>>>,
    ) -> io::Result<Vec<u8>> {
        let mut file = self.open(path, false)?.coordinate(path, false)?;
        let result = snapshot(&mut file)
            .and_then(|snapshot| snapshot.ok_or_else(|| io::ErrorKind::WouldBlock.into()));
        let closed = file.close();
        let snapshot = result?;
        closed?;
        Ok(snapshot)
    }

    pub fn commit_text(
        &self,
        path: &str,
        source: &[u8],
        space: ExGuid,
        object: ExGuid,
        range: Range<u32>,
        replacement: &str,
    ) -> Result<(), CommitError> {
        self.commit(path, |file| {
            onestore::commit_text(file, source, space, object, range, replacement)
        })
    }

    pub fn commit_property_bytes(
        &self,
        path: &str,
        source: &[u8],
        space: ExGuid,
        object: ExGuid,
        property: u32,
        value: &[u8],
    ) -> Result<(), CommitError> {
        self.commit(path, |file| {
            onestore::commit_property_bytes(file, source, space, object, property, value)
        })
    }

    /// Publishes a prepared edit using the same native writer coordination as text commits.
    pub fn commit_prepared(
        &self,
        path: &str,
        edit: &onestore::PreparedEdit<'_>,
    ) -> Result<(), CommitError> {
        self.commit(path, |file| edit.commit(file))
    }

    /// Confirms an observed snapshot's durability under native writer coordination.
    pub fn confirm_snapshot(&self, path: &str, source: &[u8]) -> Result<(), CommitError> {
        self.commit(path, |file| onestore::confirm_snapshot(file, source))
    }

    fn commit(
        &self,
        path: &str,
        operation: impl FnOnce(&mut File<'_>) -> Result<(), CommitError>,
    ) -> Result<(), CommitError> {
        let mut file = self
            .open(path, true)
            .and_then(|file| file.coordinate(path, true))
            .map_err(|error| CommitError {
                state: CommitState::NotCommitted,
                error,
            })?;
        let result = operation(&mut file);
        let closed = file.close();
        result?;
        closed.map_err(|error| CommitError {
            state: CommitState::Committed,
            error,
        })
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.retire();
    }
}

struct File<'a> {
    client: &'a Client,
    id: Option<FileId>,
}
impl File<'_> {
    fn coordinate(self, path: &str, write: bool) -> io::Result<Self> {
        self.lock(0xfffffffb, 0x11)?;
        if write {
            self.lock(0xfffffffd, 0x12)?;
        }
        let current = self.client.open(path, false)?;
        let same = self.identity()? == current.identity()?;
        current.close()?;
        if !same {
            return Err(io::ErrorKind::ResourceBusy.into());
        }
        Ok(self)
    }

    fn lock(&self, offset: u64, flags: u32) -> io::Result<()> {
        let _: LockResponse = self.client.request(
            Command::Lock,
            LockRequest {
                file_id: self.id.unwrap(),
                lock_sequence: 0,
                locks: vec![LockElement {
                    offset,
                    length: 1,
                    flags,
                }],
            },
        )?;
        Ok(())
    }

    fn identity(&self) -> io::Result<(u64, u32)> {
        let index: QueryInfoResponse = self.client.request(
            Command::QueryInfo,
            QueryInfoRequest {
                info_type: InfoType::File,
                file_info_class: 6,
                output_buffer_length: 8,
                additional_information: 0,
                flags: 0,
                file_id: self.id.unwrap(),
                input_buffer: Vec::new(),
            },
        )?;
        let index = u64::from_le_bytes(
            index
                .output_buffer
                .try_into()
                .map_err(|_| io::ErrorKind::InvalidData)?,
        );
        if index == 0 {
            return Err(io::ErrorKind::Unsupported.into());
        }
        let volume: QueryInfoResponse = self.client.request(
            Command::QueryInfo,
            QueryInfoRequest {
                info_type: InfoType::Filesystem,
                file_info_class: 1,
                output_buffer_length: 1024,
                additional_information: 0,
                flags: 0,
                file_id: self.id.unwrap(),
                input_buffer: Vec::new(),
            },
        )?;
        let serial = volume
            .output_buffer
            .get(8..12)
            .ok_or(io::ErrorKind::InvalidData)?;
        Ok((index, u32::from_le_bytes(serial.try_into().unwrap())))
    }

    fn close(mut self) -> io::Result<()> {
        self.release()
    }

    fn release(&mut self) -> io::Result<()> {
        if let Some(file_id) = self.id.take() {
            let result: io::Result<CloseResponse> = self
                .client
                .request(Command::Close, CloseRequest { file_id, flags: 0 });
            if result.is_err() {
                self.client.retire();
            }
            result?;
        }
        Ok(())
    }
}
impl Drop for File<'_> {
    fn drop(&mut self) {
        let _ = self.release();
    }
}
fn read_size(params: &NegotiatedParams, credits: u16, requested: usize) -> usize {
    let budget = if params.dialect != Dialect::Smb2_0_2
        && params.capabilities.contains(Capabilities::LARGE_MTU)
    {
        usize::from(credits.max(1)) * 65536
    } else {
        65536
    };
    requested
        .min(params.max_read_size as usize)
        .min(budget)
        .min(1024 * 1024)
}

impl CommitIo for File<'_> {
    fn read_at(&mut self, offset: u64, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        let mut size = 0;
        let response: io::Result<ReadResponse> =
            self.client.request_with(Command::Read, |connection| {
                size = read_size(
                    &connection
                        .params()
                        .expect("connected SMB session is negotiated"),
                    connection.credits(),
                    output.len(),
                );
                (
                    ReadRequest {
                        file_id: self.id.unwrap(),
                        offset,
                        length: size as u32,
                        minimum_count: 1,
                        flags: 0,
                        padding: 0,
                        channel: 0,
                        remaining_bytes: 0,
                        read_channel_info: Vec::new(),
                    },
                    CreditCharge(size.div_ceil(65536) as u16),
                )
            });
        let response = match response {
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(0),
            result => result?,
        };
        if response.data.len() > size {
            self.client.retire();
            return Err(io::ErrorKind::InvalidData.into());
        }
        output[..response.data.len()].copy_from_slice(&response.data);
        Ok(response.data.len())
    }
    fn write_at(&mut self, offset: u64, bytes: &[u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        let size = bytes.len().min(65536);
        let response: WriteResponse = self.client.request(
            Command::Write,
            WriteRequest {
                file_id: self.id.unwrap(),
                offset,
                data: bytes[..size].to_vec(),
                data_offset: 112,
                flags: 1,
                channel: 0,
                remaining_bytes: 0,
                write_channel_info_offset: 0,
                write_channel_info_length: 0,
            },
        )?;
        if response.count as usize > size {
            return Err(io::ErrorKind::InvalidData.into());
        }
        Ok(response.count as usize)
    }
    fn flush(&mut self) -> io::Result<()> {
        let _: FlushResponse = self.client.request(
            Command::Flush,
            FlushRequest {
                file_id: self.id.unwrap(),
            },
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
