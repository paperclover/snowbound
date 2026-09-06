use crate::{Error, FileType, Node, Reference, Store, bytes::Cursor};
use std::{collections::BTreeMap, fmt};

type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ExGuid {
    /// GUID bytes in Microsoft's mixed-endian order.
    pub guid: [u8; 16],
    pub n: u32,
}

impl std::str::FromStr for ExGuid {
    type Err = Error;

    /// Parses the display form, accepting either hexadecimal letter case.
    fn from_str(value: &str) -> Result<Self> {
        let invalid = || Error {
            offset: 0,
            message: "Invalid extended GUID",
        };
        let (guid_text, extension) = value.split_once(',').ok_or_else(invalid)?;
        if guid_text.len() != 38 || !guid_text.is_ascii() || !(40..=49).contains(&value.len()) {
            return Err(invalid());
        }
        let mut guid = [0; 16];
        for (byte, at) in guid
            .iter_mut()
            .zip([1, 3, 5, 7, 10, 12, 15, 17, 20, 22, 25, 27, 29, 31, 33, 35])
        {
            *byte = u8::from_str_radix(&guid_text[at..at + 2], 16).map_err(|_| invalid())?;
        }
        guid[..4].reverse();
        guid[4..6].reverse();
        guid[6..8].reverse();
        let id = Self {
            guid,
            n: extension.parse().map_err(|_| invalid())?,
        };
        if (id.guid == [0; 16] && id.n != 0) || !id.to_string().eq_ignore_ascii_case(value) {
            return Err(invalid());
        }
        Ok(id)
    }
}

impl fmt::Display for ExGuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let g = &self.guid;
        write!(
            f,
            "{{{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}},{}",
            u32::from_le_bytes(g[..4].try_into().unwrap()),
            u16::from_le_bytes(g[4..6].try_into().unwrap()),
            u16::from_le_bytes(g[6..8].try_into().unwrap()),
            g[8],
            g[9],
            g[10],
            g[11],
            g[12],
            g[13],
            g[14],
            g[15],
            self.n
        )
    }
}

impl serde::Serialize for ExGuid {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> serde::Deserialize<'de> for ExGuid {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        let value = <String as serde::Deserialize>::deserialize(deserializer)?;
        value.parse().map_err(serde::de::Error::custom)
    }
}

impl Cursor<'_> {
    pub(crate) fn exguid(&mut self) -> Result<ExGuid> {
        let id = ExGuid {
            guid: self.read()?,
            n: u32::from_le_bytes(self.read()?),
        };
        if id.guid == [0; 16] && id.n != 0 {
            return Err(Error {
                offset: self.offset - 20,
                message: "Zero GUID has a nonzero extension",
            });
        }
        Ok(id)
    }
}

impl<'a> Node<'a> {
    pub(crate) fn fields(&self, store: &Store<'a>) -> Cursor<'a> {
        Cursor {
            bytes: self.payload,
            offset: self
                .payload
                .as_ptr()
                .addr()
                .checked_sub(store.data.as_ptr().addr())
                .filter(|offset| *offset <= store.data.len())
                .unwrap_or(self.offset),
        }
    }

    pub(crate) fn referenced_list<'s>(&self, store: &'s Store<'a>) -> Result<&'s [Node<'a>]> {
        let Some(Reference::NodeList(chunk)) = self.reference else {
            return Err(Error {
                offset: self.offset,
                message: "Node requires a file-node list reference",
            });
        };
        Ok(&store.list(chunk)?.nodes)
    }
}

#[derive(Debug)]
pub struct Revision<'a> {
    pub dependency: Option<ExGuid>,
    pub encrypted: bool,
    pub nodes: &'a [Node<'a>],
}

#[derive(Debug)]
pub struct ObjectSpace<'a> {
    pub revisions: BTreeMap<ExGuid, Revision<'a>>,
    pub labels: BTreeMap<(ExGuid, u32), ExGuid>,
}

#[derive(Debug)]
pub struct RevisionIndex<'a> {
    pub root: ExGuid,
    pub spaces: BTreeMap<ExGuid, ObjectSpace<'a>>,
    pub(crate) store: &'a Store<'a>,
}

impl<'a> RevisionIndex<'a> {
    pub fn parse(store: &'a Store<'a>) -> Result<Self> {
        let mut root = None;
        let mut spaces = BTreeMap::new();
        for node in &store.list(store.header.root)?.nodes {
            match node.id {
                4 => {
                    let id = node.fields(store).exguid()?;
                    if !spaces.contains_key(&id) {
                        return Err(Error {
                            offset: node.offset,
                            message: "Root object space is referenced before its declaration",
                        });
                    }
                    if root.replace(id).is_some() {
                        return Err(Error {
                            offset: node.offset,
                            message: "Multiple root object spaces",
                        });
                    }
                }
                8 => {
                    let id = node.fields(store).exguid()?;
                    let manifest = node.referenced_list(store)?;
                    let Some(first) = manifest.first() else {
                        return Err(Error {
                            offset: node.offset,
                            message: "Empty object-space manifest",
                        });
                    };
                    if id == ExGuid::default()
                        || first.id != 0xc
                        || first.fields(store).exguid()? != id
                    {
                        return Err(Error {
                            offset: first.offset,
                            message: "Object-space identity does not match its manifest",
                        });
                    }
                    let last = manifest.iter().rfind(|node| node.id == 0x10).ok_or(Error {
                        offset: first.offset,
                        message: "Object space has no revision manifest list",
                    })?;
                    let nodes = last.referenced_list(store)?;
                    let Some(first) = nodes.first() else {
                        return Err(Error {
                            offset: last.offset,
                            message: "Empty revision manifest list",
                        });
                    };
                    if first.id != 0x14 || first.fields(store).exguid()? != id {
                        return Err(Error {
                            offset: first.offset,
                            message: "Revision list belongs to a different object space",
                        });
                    }
                    let mut revisions: BTreeMap<ExGuid, Revision<'a>> = BTreeMap::new();
                    let mut labels = BTreeMap::new();
                    let mut encryption_data = None;
                    let mut position = 1;
                    while position < nodes.len() {
                        let node = &nodes[position];
                        let mut c = node.fields(store);
                        match node.id {
                            0x1b | 0x1e | 0x1f => {
                                if (node.id == 0x1b)
                                    != (store.header.file_type == FileType::TableOfContents)
                                {
                                    return Err(Error {
                                        offset: node.offset,
                                        message: "Revision encoding does not match the file type",
                                    });
                                }
                                let rid = c.exguid()?;
                                let dependency = c.exguid()?;
                                if node.id == 0x1b {
                                    c.take(8)?;
                                }
                                let role = u32::from_le_bytes(c.read()?);
                                let encoding = u16::from_le_bytes(c.read()?);
                                let context = if node.id == 0x1f {
                                    c.exguid()?
                                } else {
                                    ExGuid::default()
                                };
                                if rid == ExGuid::default()
                                    || role > 0xffff
                                    || !matches!(encoding, 0 | 2)
                                    || (node.id == 0x1b && encoding != 0)
                                {
                                    return Err(Error {
                                        offset: node.offset,
                                        message: "Invalid revision identity, role, or encoding",
                                    });
                                }
                                let dependency = if dependency == ExGuid::default() {
                                    None
                                } else {
                                    let previous = revisions.get(&dependency).ok_or(Error {
                                        offset: node.offset,
                                        message: "Revision dependency is not an earlier revision",
                                    })?;
                                    if previous.encrypted != (encoding == 2) {
                                        return Err(Error {
                                            offset: node.offset,
                                            message: "Revision changes its dependency's encryption mode",
                                        });
                                    }
                                    Some(dependency)
                                };
                                let start = position + 1;
                                position = start;
                                while position < nodes.len() && nodes[position].id != 0x1c {
                                    if matches!(
                                        nodes[position].id,
                                        0x1b | 0x1e | 0x1f | 0x5c | 0x5d
                                    ) {
                                        return Err(Error {
                                            offset: nodes[position].offset,
                                            message: "Unterminated revision manifest",
                                        });
                                    }
                                    position += 1;
                                }
                                if position == nodes.len() {
                                    return Err(Error {
                                        offset: node.offset,
                                        message: "Unterminated revision manifest",
                                    });
                                }
                                let body = &nodes[start..position];
                                for (position, item) in body.iter().enumerate() {
                                    if item.id == 0xb0
                                        && body.get(position + 1).is_none_or(|next| next.id != 0x84)
                                    {
                                        return Err(Error {
                                            offset: item.offset,
                                            message: "Object group lacks its dependency overrides",
                                        });
                                    }
                                }
                                if (encoding == 2)
                                    != body.first().is_some_and(|node| node.id == 0x7c)
                                {
                                    return Err(Error {
                                        offset: node.offset,
                                        message: "Revision encryption key does not match its encoding",
                                    });
                                }
                                if revisions
                                    .values()
                                    .next()
                                    .is_some_and(|previous| previous.encrypted != (encoding == 2))
                                {
                                    return Err(Error {
                                        offset: node.offset,
                                        message: "Object space mixes encrypted and unencrypted revisions",
                                    });
                                }
                                if encoding == 2 {
                                    let key = &body[0];
                                    let Some(Reference::Data(chunk)) = key.reference else {
                                        return Err(Error {
                                            offset: key.offset,
                                            message: "Encryption key lacks a data reference",
                                        });
                                    };
                                    let data = store.encryption_key(chunk)?;
                                    if encryption_data
                                        .replace(data)
                                        .is_some_and(|previous| previous != data)
                                    {
                                        return Err(Error {
                                            offset: key.offset,
                                            message: "Object space changes its encryption key",
                                        });
                                    }
                                }
                                if revisions
                                    .insert(
                                        rid,
                                        Revision {
                                            dependency,
                                            encrypted: encoding == 2,
                                            nodes: body,
                                        },
                                    )
                                    .is_some()
                                {
                                    return Err(Error {
                                        offset: node.offset,
                                        message: "Duplicate revision identity",
                                    });
                                }
                                labels.insert((context, role), rid);
                            }
                            0x5c | 0x5d => {
                                let rid = c.exguid()?;
                                let role = u32::from_le_bytes(c.read()?);
                                let context = if node.id == 0x5d {
                                    c.exguid()?
                                } else {
                                    ExGuid::default()
                                };
                                if !revisions.contains_key(&rid) || role > 0xffff {
                                    return Err(Error {
                                        offset: node.offset,
                                        message: "Revision label has an invalid target or role",
                                    });
                                }
                                labels.insert((context, role), rid);
                            }
                            _ => {
                                return Err(Error {
                                    offset: node.offset,
                                    message: "Unexpected node between revision manifests",
                                });
                            }
                        }
                        position += 1;
                    }
                    if spaces
                        .insert(id, ObjectSpace { revisions, labels })
                        .is_some()
                    {
                        return Err(Error {
                            offset: node.offset,
                            message: "Duplicate object-space identity",
                        });
                    }
                }
                0x90 => {}
                _ => {
                    return Err(Error {
                        offset: node.offset,
                        message: "Unexpected node in the root file-node list",
                    });
                }
            }
        }
        let root = root.filter(|id| spaces.contains_key(id)).ok_or(Error {
            offset: 172,
            message: "Root object space is not declared",
        })?;
        Ok(Self {
            root,
            spaces,
            store,
        })
    }
}
