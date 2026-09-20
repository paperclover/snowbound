use crate::bytes::Cursor;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct Error {
    /// Parser byte offset; zero also represents errors without a byte location.
    pub offset: usize,
    /// Diagnostic text, not a stable machine-readable error code.
    pub message: &'static str,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at byte {:#x}", self.message, self.offset)
    }
}

impl std::error::Error for Error {}

type Result<T> = std::result::Result<T, Error>;

impl Cursor<'_> {
    fn chunk(&mut self) -> Result<Chunk> {
        Ok(Chunk {
            offset: u64::from_le_bytes(self.read()?),
            length: u64::from(u32::from_le_bytes(self.read()?)),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileType {
    Section,
    TableOfContents,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chunk {
    pub offset: u64,
    pub length: u64,
}

impl Chunk {
    pub(crate) fn absent(self) -> bool {
        self.length == 0 && (self.offset == 0 || self.offset == u64::MAX)
    }

    fn range(self, data: &[u8]) -> Result<Range<usize>> {
        let start = usize::try_from(self.offset).map_err(|_| Error {
            offset: 0,
            message: "Chunk offset exceeds address space",
        })?;
        let size = usize::try_from(self.length).map_err(|_| Error {
            offset: start,
            message: "Chunk length exceeds address space",
        })?;
        let end = start.checked_add(size).filter(|end| *end <= data.len());
        match end {
            Some(end) if start >= 1024 && end > start => Ok(start..end),
            _ => Err(Error {
                offset: start,
                message: "Chunk extends outside the data area",
            }),
        }
    }
}

#[derive(Debug)]
pub struct Header {
    pub file_type: FileType,
    pub file_id: [u8; 16],
    /// The parent table of contents' file identity; zero outside a notebook.
    pub ancestor: [u8; 16],
    /// CRC of the file name (a section's file name, a group's folder name).
    pub name_crc: u32,
    pub transaction_count: u32,
    pub expected_length: u64,
    pub version_id: [u8; 16],
    pub generation: u64,
    pub deny_read_id: [u8; 16],
    pub transaction_log: Chunk,
    pub root: Chunk,
    pub hashed_chunks: Chunk,
}

impl Header {
    pub(crate) fn parse(data: &[u8]) -> Result<Self> {
        let mut c = Cursor {
            bytes: data,
            offset: 0,
        };
        let bytes = c.take(1024)?;
        let mut c = Cursor { bytes, offset: 0 };
        let file_type = match c.read()? {
            [
                0xe4,
                0x52,
                0x5c,
                0x7b,
                0x8c,
                0xd8,
                0xa7,
                0x4d,
                0xae,
                0xb1,
                0x53,
                0x78,
                0xd0,
                0x29,
                0x96,
                0xd3,
            ] => FileType::Section,
            [
                0xa1,
                0x2f,
                0xff,
                0x43,
                0xd9,
                0xef,
                0x76,
                0x4c,
                0x9e,
                0xe2,
                0x10,
                0xea,
                0x57,
                0x22,
                0x76,
                0x5f,
            ] => FileType::TableOfContents,
            _ => {
                return Err(Error {
                    offset: 0,
                    message: "Unrecognized revision-store file type",
                });
            }
        };
        let file_id = c.read()?;
        c.take(16)?;
        if c.read::<16>()?
            != [
                0x3f, 0xdd, 0x9a, 0x10, 0x1b, 0x91, 0xf5, 0x49, 0xa5, 0xd0, 0x17, 0x91, 0xed, 0xc8,
                0xae, 0xd8,
            ]
        {
            return Err(Error {
                offset: 48,
                message: "Unrecognized revision-store format",
            });
        }
        c.take(12)?;
        let version = u32::from_le_bytes(c.read()?);
        let supported = match file_type {
            FileType::Section => 0x2a,
            FileType::TableOfContents => 0x1b,
        };
        if version != supported {
            return Err(Error {
                offset: 76,
                message: "Unsupported file-format version",
            });
        }
        c.take(16)?;
        let transaction_count = u32::from_le_bytes(c.read()?);
        if transaction_count == 0 {
            return Err(Error {
                offset: 96,
                message: "Missing committed transaction",
            });
        }
        c.take(28)?;
        let ancestor = c.read()?;
        let name_crc = u32::from_le_bytes(c.read()?);
        let hashed_chunks = c.chunk()?;
        let transaction_log = c.chunk()?;
        let root = c.chunk()?;
        c.take(12)?;
        let expected_length = u64::from_le_bytes(c.read()?);
        c.take(8)?;
        let version_id = c.read()?;
        let generation = u64::from_le_bytes(c.read()?);
        let deny_read_id = c.read()?;
        Ok(Self {
            file_type,
            file_id,
            ancestor,
            name_crc,
            transaction_count,
            expected_length,
            version_id,
            generation,
            deny_read_id,
            transaction_log,
            root,
            hashed_chunks,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reference {
    Data(Chunk),
    NodeList(Chunk),
}

#[derive(Debug)]
pub struct Node<'a> {
    pub id: u16,
    pub offset: usize,
    pub payload: &'a [u8],
    pub reference: Option<Reference>,
}

#[derive(Debug)]
pub struct NodeList<'a> {
    pub fragments: Vec<Chunk>,
    pub nodes: Vec<Node<'a>>,
}

#[derive(Debug)]
pub(crate) struct TransactionFragment<'a> {
    pub chunk: Chunk,
    pub entries: &'a [u8],
}

#[derive(Debug)]
pub struct Store<'a> {
    pub(crate) data: &'a [u8],
    pub(crate) transaction_fragments: Vec<TransactionFragment<'a>>,
    pub header: Header,
    pub lists: BTreeMap<u32, NodeList<'a>>,
    pub checksum_mismatches: Vec<usize>,
}

fn claim(occupied: &mut BTreeMap<usize, usize>, range: Range<usize>) -> Result<()> {
    if occupied
        .range(..range.end)
        .next_back()
        .is_some_and(|(_, end)| *end > range.start)
    {
        return Err(Error {
            offset: range.start,
            message: "Overlapping structural chunks or a reference cycle",
        });
    }
    occupied.insert(range.start, range.end);
    Ok(())
}

pub(crate) fn crc(mut value: u32, bytes: &[u8], file_type: FileType) -> u32 {
    for byte in bytes {
        match file_type {
            FileType::Section => {
                value ^= u32::from(*byte);
                for _ in 0..8 {
                    value = (value >> 1) ^ if value & 1 != 0 { 0xedb88320 } else { 0 };
                }
            }
            FileType::TableOfContents => {
                let mut entry = ((value >> 24) ^ u32::from(*byte)) << 24;
                for _ in 0..8 {
                    entry = (entry << 1) ^ if entry & 0x80000000 != 0 { 0xaf } else { 0 };
                }
                value = (value << 8) ^ (entry & 0xffff);
            }
        }
    }
    value
}

pub(crate) fn transaction_crc(value: u32, bytes: &[u8], file_type: FileType, full: bool) -> u32 {
    // OneNote excludes the entry flushed at a TOC fragment boundary (MS-ONESTORE 2.3.3.2 note 8).
    let bytes = if full && file_type == FileType::TableOfContents {
        &bytes[..bytes.len().saturating_sub(8)]
    } else {
        bytes
    };
    crc(value, bytes, file_type)
}

impl<'a> Store<'a> {
    /// Parses committed storage while retaining checksum mismatches for diagnostic inspection.
    pub fn parse(data: &'a [u8]) -> Result<Self> {
        let header = Header::parse(data)?;
        let mut occupied = BTreeMap::new();
        let mut counts = BTreeMap::new();
        let mut checksum_mismatches = Vec::new();
        let mut chunk = header.transaction_log;
        let initial_crc = if header.file_type == FileType::Section {
            u32::MAX
        } else {
            0
        };
        let mut checksum = initial_crc;
        let mut committed = 0;
        let mut transaction_fragments = Vec::new();
        while committed < header.transaction_count {
            let current_chunk = chunk;
            let range = chunk.range(data)?;
            claim(&mut occupied, range.clone())?;
            let mut c = Cursor {
                bytes: &data[range.clone()],
                offset: range.start,
            };
            let end = loop {
                if committed == header.transaction_count {
                    break c.offset;
                }
                if (12..20).contains(&c.bytes.len()) {
                    let end = c.offset;
                    chunk = c.chunk()?;
                    break end;
                }
                let offset = c.offset;
                let entry = c.take(8)?;
                let id = u32::from_le_bytes(entry[..4].try_into().unwrap());
                let value = u32::from_le_bytes(entry[4..].try_into().unwrap());
                if id == 1 {
                    let expected = if header.file_type == FileType::Section {
                        !checksum
                    } else {
                        checksum
                    };
                    if value != expected {
                        checksum_mismatches.push(offset);
                    }
                    committed += 1;
                } else {
                    if id < 0x10 || counts.get(&id).is_some_and(|previous| *previous > value) {
                        return Err(Error {
                            offset,
                            message: "Incorrect file-node count in transaction log",
                        });
                    }
                    counts.insert(id, value);
                }
                checksum = transaction_crc(checksum, entry, header.file_type, c.bytes.len() < 20);
            };
            transaction_fragments.push(TransactionFragment {
                chunk: current_chunk,
                entries: &data[range.start..end],
            });
        }
        let mut pending = vec![header.root];
        if !header.hashed_chunks.absent() {
            pending.push(header.hashed_chunks);
        }
        let mut lists = BTreeMap::new();
        let mut list_ids = BTreeSet::new();
        while let Some(mut chunk) = pending.pop() {
            let mut nodes = Vec::new();
            let mut fragments = Vec::new();
            let mut list_id = None;
            let mut required = 0;
            loop {
                let range = chunk.range(data)?;
                claim(&mut occupied, range.clone())?;
                if range.len() < 36 {
                    return Err(Error {
                        offset: range.start,
                        message: "Truncated file-node fragment",
                    });
                }
                let mut c = Cursor {
                    bytes: &data[range.clone()],
                    offset: range.start,
                };
                if u64::from_le_bytes(c.read()?) != 0xa4567ab1f5f7f4c4 {
                    return Err(Error {
                        offset: range.start,
                        message: "Incorrect file-node fragment signature",
                    });
                }
                let id = u32::from_le_bytes(c.read()?);
                let sequence = u32::from_le_bytes(c.read()?);
                if usize::try_from(sequence).ok() != Some(fragments.len())
                    || list_id.is_some_and(|previous| id != previous)
                {
                    return Err(Error {
                        offset: range.start + 8,
                        message: "Incorrect file-node fragment sequence",
                    });
                }
                if list_id.is_none() {
                    if !list_ids.insert(id) {
                        return Err(Error {
                            offset: range.start + 8,
                            message: "Duplicate file-node list identity",
                        });
                    }
                    required = usize::try_from(*counts.get(&id).ok_or(Error {
                        offset: range.start + 8,
                        message: "File-node list is absent from the transaction log",
                    })?)
                    .map_err(|_| Error {
                        offset: range.start + 8,
                        message: "File-node count exceeds address space",
                    })?;
                    list_id = Some(id);
                }
                let mut tail = Cursor {
                    bytes: &data[range.end - 20..range.end],
                    offset: range.end - 20,
                };
                let next = tail.chunk()?;
                if u64::from_le_bytes(tail.read()?) != 0x8bc215c38233ba4b {
                    return Err(Error {
                        offset: range.end - 8,
                        message: "Incorrect file-node fragment footer",
                    });
                }
                c.bytes = &data[range.start + 16..range.end - 20];
                while nodes.len() < required && c.bytes.len() >= 4 {
                    let offset = c.offset;
                    let raw = u32::from_le_bytes(c.read()?);
                    let size = usize::try_from((raw >> 10) & 0x1fff).unwrap();
                    if size < 4 {
                        return Err(Error {
                            offset,
                            message: "File node is shorter than its header",
                        });
                    }
                    let id = u16::try_from(raw & 0x3ff).unwrap();
                    let body = c.take(size - 4)?;
                    if id == 0xff {
                        break;
                    }
                    let mut fields = Cursor {
                        bytes: body,
                        offset: offset + 4,
                    };
                    let reference = match (raw >> 27) & 0xf {
                        0 => None,
                        base @ (1 | 2) => {
                            let (stp, nil, shift) = match (raw >> 23) & 3 {
                                0 => (u64::from_le_bytes(fields.read()?), u64::MAX, 0),
                                1 => (
                                    u64::from(u32::from_le_bytes(fields.read()?)),
                                    u64::from(u32::MAX),
                                    0,
                                ),
                                2 => (
                                    u64::from(u16::from_le_bytes(fields.read()?)),
                                    u64::from(u16::MAX),
                                    3,
                                ),
                                3 => (
                                    u64::from(u32::from_le_bytes(fields.read()?)),
                                    u64::from(u32::MAX),
                                    3,
                                ),
                                _ => unreachable!(),
                            };
                            let cb = match (raw >> 25) & 3 {
                                0 => u64::from(u32::from_le_bytes(fields.read()?)),
                                1 => u64::from_le_bytes(fields.read()?),
                                2 => u64::from(fields.read::<1>()?[0]) * 8,
                                3 => u64::from(u16::from_le_bytes(fields.read()?)) * 8,
                                _ => unreachable!(),
                            };
                            let reference = Chunk {
                                offset: if cb == 0 && stp == nil {
                                    u64::MAX
                                } else {
                                    stp << shift
                                },
                                length: cb,
                            };
                            if base == 1 {
                                if !reference.absent() {
                                    reference.range(data)?;
                                }
                                Some(Reference::Data(reference))
                            } else {
                                Some(Reference::NodeList(reference))
                            }
                        }
                        _ => {
                            return Err(Error {
                                offset,
                                message: "Unsupported file-node base type",
                            });
                        }
                    };
                    nodes.push(Node {
                        id,
                        offset,
                        payload: fields.bytes,
                        reference,
                    });
                }
                fragments.push(chunk);
                if nodes.len() == required {
                    break;
                }
                chunk = next;
            }
            let last_revision_list = nodes.iter().rposition(|node| node.id == 0x10);
            for (index, node) in nodes.iter().enumerate() {
                if let Some(Reference::NodeList(reference)) = node.reference
                    && (node.id != 0x10 || Some(index) == last_revision_list)
                {
                    pending.push(reference);
                }
            }
            nodes.shrink_to_fit();
            lists.insert(list_id.unwrap(), NodeList { fragments, nodes });
        }
        Ok(Self {
            data,
            transaction_fragments,
            header,
            lists,
            checksum_mismatches,
        })
    }

    pub fn chunk_data(&self, chunk: Chunk) -> Result<&'a [u8]> {
        Ok(&self.data[chunk.range(self.data)?])
    }

    pub(crate) fn list(&self, chunk: Chunk) -> Result<&NodeList<'a>> {
        let data = self.chunk_data(chunk)?;
        let mut c = Cursor {
            bytes: data,
            offset: usize::try_from(chunk.offset).unwrap(),
        };
        c.take(8)?;
        let id = u32::from_le_bytes(c.read()?);
        self.lists
            .get(&id)
            .filter(|list| list.fragments.first() == Some(&chunk))
            .ok_or(Error {
                offset: c.offset - 4,
                message: "Reference does not identify a committed file-node list",
            })
    }
}
