//! Blocking SMB access to OneNote sections, table-of-contents files and payloads.

use onestore::{CommitError, CommitIo, CommitState};
use smb2::{
    Session, Tree,
    client::connection::{CompoundOp, Connection, Frame, NegotiatedParams},
    msg::{
        close::{CloseRequest, CloseResponse},
        create::{
            CreateDisposition, CreateRequest, CreateResponse, ImpersonationLevel, ShareAccess,
        },
        flush::{FlushRequest, FlushResponse},
        lock::{LockElement, LockRequest, LockResponse},
        query_info::{InfoType, QueryInfoRequest, QueryInfoResponse},
        read::{ReadRequest, ReadResponse},
        set_info::{SetInfoRequest, SetInfoResponse},
        write::{WriteRequest, WriteResponse},
    },
    pack::{Pack, ReadCursor, Unpack},
    types::{
        Command, CreditCharge, Dialect, FileId, OplockLevel,
        flags::{Capabilities, FileAccessMask},
    },
};
use std::{io, sync::Mutex, time::Duration};
use tokio::runtime::{Handle, Runtime};

mod directory;
mod remote;
pub use directory::DirectoryEntry;
pub use remote::SmbRemote;

/// FILE_ATTRIBUTE_HIDDEN.
pub const HIDDEN: u32 = 0x2;

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
    /// The server as `connect` was given it, which names the share's `location`.
    address: String,
    tree: Tree,
    timeout: Duration,
    runtime: Mutex<Option<Runtime>>,
}

impl Client {
    /// The notebook folder `root` on this share, as `crate::location` names it.
    pub(crate) fn location(&self, root: &str) -> String {
        crate::location::smb(&self.address, &self.tree.share_name, root)
    }

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
                    let mut connection = sign_in(address, &credentials, timeout).await?;
                    let tree = Tree::connect(&mut connection, share)
                        .await
                        .map_err(io::Error::other)?;
                    Ok::<_, io::Error>((connection, tree))
                })
                .await
            })
            .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))??;
        Ok(Self {
            connection: Mutex::new(Some(connection)),
            address: address.to_owned(),
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
        self.unpack(&self.body(command, frame)?)
    }

    /// Sends `requests` as one related compound request, which the server performs in order,
    /// and returns each response body or status error. A request after a CREATE names the
    /// file it opened as `FileId::SENTINEL`; each request fits one credit.
    fn compound(&self, requests: &[(Command, &dyn Pack)]) -> io::Result<Vec<io::Result<Vec<u8>>>> {
        if Handle::try_current().is_ok() {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        let connection = self
            .connection
            .lock()
            .map_err(|_| io::ErrorKind::Other)?
            .clone()
            .ok_or(io::ErrorKind::NotConnected)?;
        let operations: Vec<_> = requests
            .iter()
            .map(|(command, body)| CompoundOp::new(*command, *body, Some(self.tree.tree_id)))
            .collect();
        let frames = {
            let runtime = self.runtime.lock().map_err(|_| io::ErrorKind::Other)?;
            runtime
                .as_ref()
                .ok_or(io::ErrorKind::NotConnected)?
                .block_on(async {
                    tokio::time::timeout(self.timeout, connection.execute_compound(&operations))
                        .await
                })
        };
        let frames = frames
            .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))
            .and_then(|result| result.map_err(io::Error::other))
            .inspect_err(|_| self.retire())?;
        Ok(frames
            .into_iter()
            .zip(requests)
            .map(|(frame, (command, _))| {
                let frame = frame
                    .map_err(io::Error::other)
                    .inspect_err(|_| self.retire())?;
                self.body(*command, frame)
            })
            .collect())
    }

    /// The body of a response to `command`, or its status as an error.
    fn body(&self, command: Command, frame: Frame) -> io::Result<Vec<u8>> {
        if frame.header.command != command {
            self.retire();
            return Err(io::ErrorKind::InvalidData.into());
        }
        if frame.header.status.0 != 0 {
            let kind = match frame.header.status.0 {
                0xc0000043 | 0xc0000054 | 0xc0000055 => io::ErrorKind::WouldBlock,
                0xc0000011 => io::ErrorKind::UnexpectedEof,
                0xc0000034 | 0xc000003a => io::ErrorKind::NotFound,
                0xc0000035 => io::ErrorKind::AlreadyExists,
                // Windows reports a delete-pending file as access denied too.
                0xc0000022 | 0xc0000056 => io::ErrorKind::PermissionDenied,
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
        Ok(frame.body)
    }

    fn unpack<T: Unpack>(&self, body: &[u8]) -> io::Result<T> {
        T::unpack(&mut ReadCursor::new(body)).map_err(|error| {
            self.retire();
            io::Error::new(io::ErrorKind::InvalidData, error)
        })
    }

    /// Opens as OneNote 2010 does: readers share everything, a writer denies other writers.
    fn open(&self, path: &str, write: bool) -> io::Result<File<'_>> {
        self.open_shared(path, write, if write { 5 } else { 7 })
    }

    fn open_shared(&self, path: &str, write: bool, sharing: u32) -> io::Result<File<'_>> {
        self.open_with(
            path,
            if write { 0xc0000000 } else { 0x80000000 },
            sharing,
            CreateDisposition::FileOpen,
            0x40,
        )
    }

    fn open_with(
        &self,
        path: &str,
        access: u32,
        sharing: u32,
        disposition: CreateDisposition,
        options: u32,
    ) -> io::Result<File<'_>> {
        let response: CreateResponse = self.request(
            Command::Create,
            create_request(path, access, sharing, disposition, options)?,
        )?;
        Ok(File::new(self, &response))
    }

    /// Creates a file holding `bytes`; an existing file is an error.
    pub fn create(&self, path: &str, bytes: &[u8]) -> io::Result<()> {
        let mut file = self.open_with(path, 0xc0000000, 0, CreateDisposition::FileCreate, 0x40)?;
        let mut written = 0;
        while written < bytes.len() {
            let count = file.write_at(written as u64, &bytes[written..])?;
            if count == 0 {
                return Err(io::ErrorKind::WriteZero.into());
            }
            written += count;
        }
        file.flush()?;
        file.close()
    }

    /// Creates a directory; an existing one is an error.
    pub fn create_directory(&self, path: &str) -> io::Result<()> {
        self.open_with(path, 0x80000000, 7, CreateDisposition::FileCreate, 0x1)?
            .close()
    }

    /// Renames or moves a file or directory within the share; an existing target is an
    /// error.
    pub fn rename(&self, from: &str, to: &str) -> io::Result<()> {
        self.rename_over(from, to, false)
    }

    /// Renames a file over another within the share, replacing it.
    pub fn replace(&self, from: &str, to: &str) -> io::Result<()> {
        self.rename_over(from, to, true)
    }

    fn rename_over(&self, from: &str, to: &str, replace: bool) -> io::Result<()> {
        if to.is_empty() || to.contains('\0') {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        let file = self.open_with(
            from,
            0x00010000 | 0x80000000,
            7,
            CreateDisposition::FileOpen,
            0,
        )?;
        let name: Vec<u8> = to
            .replace('/', "\\")
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        // FileRenameInformation: ReplaceIfExists, then reserved bytes and no root directory.
        let mut buffer = vec![0; 16];
        buffer[0] = u8::from(replace);
        buffer.extend_from_slice(&u32::try_from(name.len()).unwrap().to_le_bytes());
        buffer.extend_from_slice(&name);
        let _: SetInfoResponse = self.request(
            Command::SetInfo,
            SetInfoRequest {
                info_type: InfoType::File,
                file_info_class: 10,
                additional_information: 0,
                file_id: file.id.ok_or(io::ErrorKind::InvalidInput)?,
                buffer,
            },
        )?;
        file.close()
    }

    /// Gives a file or directory the hidden attribute, keeping its others, as OneNote 2010
    /// skips a hidden folder.
    pub fn hide(&self, path: &str) -> io::Result<()> {
        // FILE_READ_ATTRIBUTES and FILE_WRITE_ATTRIBUTES.
        let file = self.open_with(path, 0x180, 7, CreateDisposition::FileOpen, 0)?;
        let file_id = file.id.ok_or(io::ErrorKind::InvalidInput)?;
        // FileBasicInformation: four times, then the attributes; a time of 0 stays as it is.
        let basic: QueryInfoResponse = self.request(
            Command::QueryInfo,
            QueryInfoRequest {
                info_type: InfoType::File,
                file_info_class: 4,
                output_buffer_length: 40,
                additional_information: 0,
                flags: 0,
                file_id,
                input_buffer: Vec::new(),
            },
        )?;
        let attributes = basic
            .output_buffer
            .get(32..36)
            .and_then(|bytes| bytes.try_into().ok())
            .map(u32::from_le_bytes)
            .ok_or(io::ErrorKind::InvalidData)?;
        // Set even where reported: Samba reports a dot name hidden without storing it so.
        let mut buffer = vec![0; 40];
        buffer[32..36].copy_from_slice(&(attributes | HIDDEN).to_le_bytes());
        let _: SetInfoResponse = self.request(
            Command::SetInfo,
            SetInfoRequest {
                info_type: InfoType::File,
                file_info_class: 4,
                additional_information: 0,
                file_id,
                buffer,
            },
        )?;
        file.close()
    }

    /// Deletes a file or an empty directory.
    pub fn delete(&self, path: &str) -> io::Result<()> {
        self.open_with(path, 0x00010000, 7, CreateDisposition::FileOpen, 0x1000)?
            .close()
    }

    /// Names a section or TOC file for its notebook (`onestore::place`) under native
    /// writer coordination.
    pub fn place(&self, path: &str, ancestor: [u8; 16], name: &str) -> Result<(), CommitError> {
        self.commit(path, &[(0, 1024)], |file| {
            onestore::place(file, ancestor, name).map_err(|error| CommitError {
                state: CommitState::NotCommitted,
                error,
            })
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
            snapshot(|offset, output| file.read_at(offset, output), limit).map(Some)
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
        let mut file = self
            .open(path, false)?
            .coordinate(path, false, &[(0, 1024)])?;
        let result = snapshot(&mut file)
            .and_then(|snapshot| snapshot.ok_or_else(|| io::ErrorKind::WouldBlock.into()));
        let closed = file.close();
        let snapshot = result?;
        closed?;
        Ok(snapshot)
    }

    /// Publishes a transaction using the same native writer coordination as text commits.
    pub fn commit_transaction(
        &self,
        path: &str,
        transaction: &onestore::Transaction,
    ) -> Result<(), CommitError> {
        self.commit(path, &checked(transaction.base()), |file| {
            transaction.commit(file)
        })
    }

    /// The header and length of a revision store, in one round trip, without writer
    /// coordination or path identity checks: a change detector for polling, never a snapshot
    /// to edit.
    pub fn stamp(&self, path: &str) -> io::Result<onestore::Stamp> {
        let create = create_request(path, 0x80000000, 7, CreateDisposition::FileOpen, 0x40)?;
        let read = read_request(FileId::SENTINEL, 0, 1024);
        let close = CloseRequest {
            file_id: FileId::SENTINEL,
            flags: 0,
        };
        let [created, read, closed]: [_; 3] = self
            .compound(&[
                (Command::Create, &create),
                (Command::Read, &read),
                (Command::Close, &close),
            ])?
            .try_into()
            .map_err(|_| io::ErrorKind::InvalidData)?;
        let created: CreateResponse = self.unpack(&created?)?;
        if closed.is_err() {
            self.retire();
        }
        let read: ReadResponse = self.unpack(&read?)?;
        closed?;
        let header = read
            .data
            .try_into()
            .map_err(|_| io::ErrorKind::UnexpectedEof)?;
        Ok(onestore::Stamp {
            header,
            length: created.end_of_file,
        })
    }

    /// Confirms that the file still has `base`'s stamp and is durable, then refreshes its
    /// version, under native writer coordination.
    pub fn confirm(&self, path: &str, base: &onestore::Stamp) -> Result<(), CommitError> {
        self.commit(path, &checked(base), |file| onestore::confirm(file, base))
    }

    /// Opens `path` for writing under OneNote's coordination, making `reads` with the locks.
    fn commit(
        &self,
        path: &str,
        reads: &[(u64, usize)],
        operation: impl FnOnce(&mut File<'_>) -> Result<(), CommitError>,
    ) -> Result<(), CommitError> {
        let mut file = self
            .open(path, true)
            .and_then(|file| file.coordinate(path, true, reads))
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

/// The disk shares the server at `address` offers the account, by name.
pub fn shares(
    address: &str,
    credentials: Credentials<'_>,
    timeout: Duration,
) -> io::Result<Vec<String>> {
    if Handle::try_current().is_ok() || timeout.is_zero() {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let shares = runtime
        .block_on(async {
            tokio::time::timeout(timeout, async {
                let mut connection = sign_in(address, &credentials, timeout).await?;
                let shares = smb2::client::list_shares(&mut connection)
                    .await
                    .map_err(io::Error::other);
                connection.mark_dead();
                shares
            })
            .await
        })
        .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))??;
    Ok(shares.into_iter().map(|share| share.name).collect())
}

/// A negotiated session with the server at `address`, signed in with `credentials`.
async fn sign_in(
    address: &str,
    credentials: &Credentials<'_>,
    timeout: Duration,
) -> io::Result<Connection> {
    let mut connection = Connection::connect(address, timeout)
        .await
        .map_err(io::Error::other)?;
    connection.set_compression_requested(false);
    if let Err(error) = connection.negotiate().await {
        connection.mark_dead();
        return Err(if speaks_only_smb1(address, timeout).await {
            io::Error::new(io::ErrorKind::Unsupported, Refusal::Smb1)
        } else {
            io::Error::other(error)
        });
    }
    Session::setup(
        &mut connection,
        credentials.username,
        credentials.password,
        credentials.domain,
    )
    .await
    .map_err(io::Error::other)?;
    Ok(connection)
}

/// Whether the server at `address` agrees to SMB1, as one that turned SMB2 away does when
/// it speaks only SMB1.
async fn speaks_only_smb1(address: &str, timeout: Duration) -> bool {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    const DIALECT: &[u8] = b"\x02NT LM 0.12\0";
    let mut message = Vec::with_capacity(35 + DIALECT.len());
    message.extend_from_slice(b"\xffSMB\x72");
    // Status, then flags (canonical paths, case-insensitive) and flags2 (NT status codes).
    message.extend_from_slice(&[0, 0, 0, 0, 0x18, 0x01, 0x40]);
    // PID high, signature, reserved, TID, PID, UID and MID.
    message.extend_from_slice(&[0; 14]);
    message.extend_from_slice(&[0xff, 0xff, 0xff, 0xfe, 0, 0, 0, 0]);
    message.push(0);
    message.extend_from_slice(&(DIALECT.len() as u16).to_le_bytes());
    message.extend_from_slice(DIALECT);
    let probe = async {
        let mut stream = tokio::net::TcpStream::connect(address).await?;
        stream
            .write_all(&(message.len() as u32).to_be_bytes())
            .await?;
        stream.write_all(&message).await?;
        // The frame's length, the SMB1 header, then the word count and the dialect chosen.
        let mut reply = [0; 39];
        stream.read_exact(&mut reply).await?;
        io::Result::Ok(reply[4..8] == *b"\xffSMB" && reply[37..] == [0, 0])
    };
    matches!(tokio::time::timeout(timeout, probe).await, Ok(Ok(true)))
}

/// Why a server turned a connection or a listing away, as a person can remedy it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Nothing answered at the address in time.
    Unreachable,
    /// The server speaks only SMB1, which this client does not.
    Smb1,
    /// The server refused the name and password, or a guest.
    SignIn,
    /// The server has no share by that name.
    NoShare,
    /// The account may not open the share or folder.
    Denied,
    /// The folder is not on the share.
    NoFolder,
    Other,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Unreachable => "The server can't be reached",
            Self::Smb1 => "The server speaks only SMB1",
            Self::SignIn => "The server refused the sign-in",
            Self::NoShare => "The server has no such share",
            Self::Denied => "Access denied",
            Self::NoFolder => "The folder is not on the share",
            Self::Other => "The server refused the request",
        })
    }
}

impl std::error::Error for Refusal {}

impl Refusal {
    /// What an error from `Client::connect`, `shares` or `Client::read_dir` means.
    pub fn of(error: &io::Error) -> Self {
        use smb2::ErrorKind as Smb;
        let inner = error.get_ref();
        if let Some(refusal) = inner.and_then(|inner| inner.downcast_ref::<Self>()) {
            return *refusal;
        }
        let Some(smb) = inner.and_then(|inner| inner.downcast_ref::<smb2::Error>()) else {
            return match error.kind() {
                io::ErrorKind::TimedOut | io::ErrorKind::NotConnected => Self::Unreachable,
                _ => Self::Other,
            };
        };
        let tree = matches!(
            smb,
            smb2::Error::Protocol {
                command: Command::TreeConnect,
                ..
            }
        );
        match smb.kind() {
            Smb::AuthRequired | Smb::SigningRequired => Self::SignIn,
            Smb::NotFound if tree => Self::NoShare,
            Smb::NotFound | Smb::NotADirectory => Self::NoFolder,
            Smb::AccessDenied => Self::Denied,
            Smb::Io | Smb::ConnectionLost | Smb::TimedOut => Self::Unreachable,
            _ => Self::Other,
        }
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
    /// Writes `flush` sends ahead of its flush, in order.
    writes: Vec<(u64, Vec<u8>)>,
    /// Reads made under the coordination locks, each returned once to the same `read_at`.
    prefetched: Vec<(u64, usize, Vec<u8>)>,
}
impl<'a> File<'a> {
    fn new(client: &'a Client, created: &CreateResponse) -> Self {
        Self {
            client,
            id: Some(created.file_id),
            writes: Vec::new(),
            prefetched: Vec::new(),
        }
    }

    /// Takes OneNote 2010's coordination locks in one request (the shared reader byte, and
    /// to write the exclusive writer byte), makes `reads` under them, then checks that the
    /// handle is still the file at `path`: maintenance replaces a file while holding its
    /// locks.
    fn coordinate(mut self, path: &str, write: bool, reads: &[(u64, usize)]) -> io::Result<Self> {
        let id = self.id.unwrap();
        let mut locks = vec![LockElement {
            offset: 0xfffffffb,
            length: 1,
            flags: 0x11,
        }];
        if write {
            locks.push(LockElement {
                offset: 0xfffffffd,
                length: 1,
                flags: 0x12,
            });
        }
        let lock = LockRequest {
            file_id: id,
            lock_sequence: 0,
            locks,
        };
        let [index, volume] = identity_requests(id);
        let reading: Vec<_> = reads
            .iter()
            .map(|(offset, length)| read_request(id, *offset, *length))
            .collect();
        let mut requests: Vec<(Command, &dyn Pack)> = vec![
            (Command::Lock, &lock),
            (Command::QueryInfo, &index),
            (Command::QueryInfo, &volume),
        ];
        requests.extend(
            reading
                .iter()
                .map(|read| (Command::Read, read as &dyn Pack)),
        );
        let mut responses = self.client.compound(&requests)?.into_iter();
        let mut next = || responses.next().ok_or(io::ErrorKind::InvalidData);
        let _: LockResponse = self.client.unpack(&next()??)?;
        let own = self.client.identity(next()?, next()?)?;
        for (offset, length) in reads {
            let data = match next()? {
                Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => Vec::new(),
                body => self.client.unpack::<ReadResponse>(&body?)?.data,
            };
            if data.len() > *length {
                self.client.retire();
                return Err(io::ErrorKind::InvalidData.into());
            }
            self.prefetched.push((*offset, *length, data));
        }
        let create = create_request(path, 0x80, 7, CreateDisposition::FileOpen, 0)?;
        let [index, volume] = identity_requests(FileId::SENTINEL);
        let close = CloseRequest {
            file_id: FileId::SENTINEL,
            flags: 0,
        };
        let [created, index, volume, closed]: [_; 4] = self
            .client
            .compound(&[
                (Command::Create, &create),
                (Command::QueryInfo, &index),
                (Command::QueryInfo, &volume),
                (Command::Close, &close),
            ])?
            .try_into()
            .map_err(|_| io::ErrorKind::InvalidData)?;
        created?;
        if closed.is_err() {
            self.client.retire();
        }
        let current = self.client.identity(index, volume)?;
        closed?;
        if own != current {
            return Err(io::ErrorKind::ResourceBusy.into());
        }
        Ok(self)
    }

    #[cfg(test)]
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

impl Client {
    /// A file's volume serial number and index, from the responses to `identity_requests`.
    fn identity(
        &self,
        index: io::Result<Vec<u8>>,
        volume: io::Result<Vec<u8>>,
    ) -> io::Result<(u64, u32)> {
        let index: QueryInfoResponse = self.unpack(&index?)?;
        let index = u64::from_le_bytes(
            index
                .output_buffer
                .try_into()
                .map_err(|_| io::ErrorKind::InvalidData)?,
        );
        if index == 0 {
            return Err(io::ErrorKind::Unsupported.into());
        }
        let volume: QueryInfoResponse = self.unpack(&volume?)?;
        let serial = volume
            .output_buffer
            .get(8..12)
            .ok_or(io::ErrorKind::InvalidData)?;
        Ok((index, u32::from_le_bytes(serial.try_into().unwrap())))
    }
}

/// FileInternalInformation and FileFsVolumeInformation queries.
fn identity_requests(file_id: FileId) -> [QueryInfoRequest; 2] {
    let query = |info_type, file_info_class, output_buffer_length| QueryInfoRequest {
        info_type,
        file_info_class,
        output_buffer_length,
        additional_information: 0,
        flags: 0,
        file_id,
        input_buffer: Vec::new(),
    };
    [
        query(InfoType::File, 6, 8),
        query(InfoType::Filesystem, 1, 1024),
    ]
}

fn create_request(
    path: &str,
    access: u32,
    sharing: u32,
    disposition: CreateDisposition,
    options: u32,
) -> io::Result<CreateRequest> {
    if path.is_empty() || path.contains('\0') || path.encode_utf16().count() > 32767 {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    Ok(CreateRequest {
        requested_oplock_level: OplockLevel::None,
        impersonation_level: ImpersonationLevel::Impersonation,
        desired_access: FileAccessMask::new(access),
        file_attributes: 0,
        share_access: ShareAccess(sharing),
        create_disposition: disposition,
        create_options: options,
        name: smb2::encode_path(&path.replace('\\', "/")),
        create_contexts: Vec::new(),
    })
}

fn read_request(file_id: FileId, offset: u64, length: usize) -> ReadRequest {
    ReadRequest {
        file_id,
        offset,
        length: length as u32,
        minimum_count: 1,
        flags: 0,
        padding: 0,
        channel: 0,
        remaining_bytes: 0,
        read_channel_info: Vec::new(),
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
        if let Some(at) = self
            .prefetched
            .iter()
            .position(|(start, length, _)| *start == offset && *length == output.len())
        {
            let (_, _, data) = self.prefetched.swap_remove(at);
            output[..data.len()].copy_from_slice(&data);
            return Ok(data.len());
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
    /// Queues at most one request's worth of `bytes`; `flush` sends it.
    fn write_at(&mut self, offset: u64, bytes: &[u8]) -> io::Result<usize> {
        let size = bytes.len().min(65536);
        if size > 0 {
            self.writes.push((offset, bytes[..size].to_vec()));
        }
        Ok(size)
    }

    /// Sends the queued writes and then the flush in related compound requests of at most
    /// sixteen, awaiting each: the server performs a compound in order, and answers the flush
    /// once every write before it is durable.
    fn flush(&mut self) -> io::Result<()> {
        let file_id = self.id.unwrap();
        let writes: Vec<_> = self
            .writes
            .drain(..)
            .map(|(offset, data)| WriteRequest {
                file_id,
                offset,
                data,
                data_offset: 112,
                flags: 0,
                channel: 0,
                remaining_bytes: 0,
                write_channel_info_offset: 0,
                write_channel_info_length: 0,
            })
            .collect();
        let flush = FlushRequest { file_id };
        let mut requests: Vec<(Command, &dyn Pack)> = writes
            .iter()
            .map(|write| (Command::Write, write as &dyn Pack))
            .collect();
        requests.push((Command::Flush, &flush));
        let mut sizes = writes.iter().map(|write| write.data.len());
        for compound in requests.chunks(16) {
            for (response, (command, _)) in
                self.client.compound(compound)?.into_iter().zip(compound)
            {
                let body = response?;
                if *command == Command::Flush {
                    let _: FlushResponse = self.client.unpack(&body)?;
                    continue;
                }
                let written: WriteResponse = self.client.unpack(&body)?;
                let size = sizes.next().ok_or(io::ErrorKind::InvalidData)?;
                if written.count as usize != size {
                    return Err(if (written.count as usize) < size {
                        io::ErrorKind::WriteZero
                    } else {
                        io::ErrorKind::InvalidData
                    }
                    .into());
                }
            }
        }
        Ok(())
    }
}

/// The reads `Stamp::check` makes: the header and a probe of the last byte.
/// `onestore::read_snapshot`, telling a file that is stably unreadable from a torn read:
/// storage that is consistent yet fails validation is refused, not reported as contention.
fn snapshot(
    mut read: impl FnMut(u64, &mut [u8]) -> io::Result<usize>,
    limit: usize,
) -> io::Result<Vec<u8>> {
    if let Some(bytes) = onestore::read_snapshot(&mut read, limit)? {
        return Ok(bytes);
    }
    let storage =
        onestore::read_storage_snapshot(&mut read, limit)?.ok_or(io::ErrorKind::WouldBlock)?;
    // A writer may have finished between the two reads.
    let at = |offset: u64, output: &mut [u8]| {
        let rest = storage.get(offset as usize..).unwrap_or_default();
        let count = rest.len().min(output.len());
        output[..count].copy_from_slice(&rest[..count]);
        Ok(count)
    };
    if let Some(bytes) = onestore::read_snapshot(at, limit)? {
        return Ok(bytes);
    }
    let locked =
        onestore::Store::parse(&storage).is_ok_and(|store| crate::discover::locked(&store));
    Err(if locked {
        io::Error::new(io::ErrorKind::Unsupported, "Password protected")
    } else {
        io::Error::new(io::ErrorKind::InvalidData, "Can't read this section")
    })
}

fn checked(stamp: &onestore::Stamp) -> [(u64, usize); 2] {
    [(0, 1024), (stamp.length.saturating_sub(1), 2)]
}

#[cfg(test)]
mod tests;
