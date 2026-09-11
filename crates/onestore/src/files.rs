use crate::{Error, Object, ObjectData, Reference, Store, bytes::Cursor};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FileDataReference {
    Internal([u8; 16]),
    External(String),
    Invalid,
}

fn guid(text: &str) -> Option<[u8; 16]> {
    let bytes = text.as_bytes();
    if bytes.len() != 36 || [8, 13, 18, 23].iter().any(|index| bytes[*index] != b'-') {
        return None;
    }
    let mut digits = bytes
        .iter()
        .enumerate()
        .filter_map(|(index, byte)| (![8, 13, 18, 23].contains(&index)).then_some(*byte));
    let mut value = [0; 16];
    for byte in &mut value {
        let high = char::from(digits.next()?).to_digit(16)?;
        let low = char::from(digits.next()?).to_digit(16)?;
        *byte = u8::try_from((high << 4) | low).ok()?;
    }
    value[..4].reverse();
    value[4..6].reverse();
    value[6..8].reverse();
    Some(value)
}

impl Object<'_> {
    pub fn file_reference(&self) -> Result<Option<FileDataReference>, Error> {
        let ObjectData::File { reference, .. } = self.data else {
            return Ok(None);
        };
        if !reference.len().is_multiple_of(2) {
            return Err(Error {
                offset: 0,
                message: "Odd UTF-16 file-data reference length",
            });
        }
        let text = String::from_utf16(
            &reference
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect::<Vec<_>>(),
        )
        .map_err(|_| Error {
            offset: 0,
            message: "Invalid UTF-16 file-data reference",
        })?;
        text.parse().map(Some)
    }
}

impl std::str::FromStr for FileDataReference {
    type Err = Error;

    fn from_str(text: &str) -> Result<Self, Error> {
        let parsed = if let Some(name) = text.strip_prefix("<file>") {
            name.strip_suffix(".onebin")
                .and_then(guid)
                .map(|_| FileDataReference::External(name.to_owned()))
        } else if let Some(id) = text
            .strip_prefix("<ifndf>{")
            .and_then(|text| text.strip_suffix('}'))
        {
            guid(id).map(FileDataReference::Internal)
        } else if text == "<invfdo>" {
            Some(FileDataReference::Invalid)
        } else {
            None
        };
        parsed.ok_or(Error {
            offset: 0,
            message: "Invalid file-data reference syntax",
        })
    }
}

impl<'a> Store<'a> {
    pub(crate) fn encryption_key(&self, chunk: crate::Chunk) -> Result<&'a [u8], Error> {
        let bytes = self.chunk_data(chunk)?;
        let footer = 0x2649294f8e198b3c_u64.to_le_bytes();
        if bytes.len() >= 16 && bytes[..8] == 0xfb6ba385dad1a067_u64.to_le_bytes() {
            for padding in 0..=7 {
                if bytes.len() < 16 + padding {
                    break;
                }
                let end = bytes.len() - padding;
                if bytes[end - 8..end] == footer
                    && bytes[end..].iter().all(|byte| *byte == 0)
                    && (padding == 0 || bytes.len().is_multiple_of(8))
                {
                    return Ok(&bytes[8..end - 8]);
                }
            }
        }
        Err(Error {
            offset: usize::try_from(chunk.offset).unwrap(),
            message: "Invalid encryption key container",
        })
    }

    pub fn file_data(&self, guid: [u8; 16]) -> Result<&'a [u8], Error> {
        let mut found = None;
        for node in self
            .lists
            .values()
            .flat_map(|list| &list.nodes)
            .filter(|node| node.id == 0x94)
        {
            let mut c = node.fields(self);
            if c.read::<16>()? != guid {
                continue;
            }
            let Some(Reference::Data(chunk)) = node.reference else {
                return Err(Error {
                    offset: node.offset,
                    message: "File-data object lacks a data reference",
                });
            };
            if found.replace(chunk).is_some() {
                return Err(Error {
                    offset: node.offset,
                    message: "Duplicate file-data identity",
                });
            }
        }
        let chunk = found.ok_or(Error {
            offset: 0,
            message: "File-data object is not declared",
        })?;
        let mut c = Cursor {
            bytes: self.chunk_data(chunk)?,
            offset: usize::try_from(chunk.offset).unwrap(),
        };
        if c.read::<16>()?
            != [
                0xe7, 0x16, 0xe3, 0xbd, 0x65, 0x26, 0x11, 0x45, 0xa4, 0xc4, 0x8d, 0x4d, 0x0b, 0x7a,
                0x9e, 0xac,
            ]
        {
            return Err(Error {
                offset: c.offset - 16,
                message: "Invalid file-data object header",
            });
        }
        let length = usize::try_from(u64::from_le_bytes(c.read()?)).map_err(|_| Error {
            offset: c.offset - 8,
            message: "File-data length exceeds address space",
        })?;
        c.take(12)?;
        let bytes = c.take(length)?;
        // The object length is aligned; its position in the file need not be.
        let padding = (8 - (36 + length) % 8) % 8;
        c.take(padding)?;
        if c.read::<16>()?
            != [
                0x22, 0xa7, 0xfb, 0x71, 0x79, 0x0f, 0x0b, 0x4a, 0xbb, 0x13, 0x89, 0x92, 0x56, 0x42,
                0x6b, 0x24,
            ]
            || !c.bytes.is_empty()
        {
            return Err(Error {
                offset: c.offset - 16,
                message: "Invalid file-data object footer or length",
            });
        }
        Ok(bytes)
    }
}
