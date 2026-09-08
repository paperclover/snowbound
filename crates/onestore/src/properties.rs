use crate::{Error, bytes::Cursor};
use std::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdStream {
    Objects,
    ObjectSpaces,
    Contexts,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Value<'a> {
    NoData,
    Bytes(&'a [u8]),
    References {
        stream: IdStream,
        compact_ids: &'a [u8],
    },
    Sets(Range<usize>),
}

#[derive(Debug, PartialEq, Eq)]
pub struct Property<'a> {
    pub id: u32,
    pub value: Value<'a>,
}

/// Set zero is the root; nested sets are ranges of indices in this arena.
#[derive(Debug, PartialEq, Eq)]
pub struct PropertySets<'a> {
    pub sets: Vec<Vec<Property<'a>>>,
    pub(crate) padding: &'a [u8],
    pub(crate) root_ids: &'a [u8],
}

enum Work<'a> {
    Set(usize),
    Fields(usize, &'a [u8]),
}

impl<'a> PropertySets<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, Error> {
        let mut c = Cursor { bytes, offset: 0 };
        let mut streams = reference_streams(&mut c)?;
        let mut sets = vec![Vec::new()];
        let mut root_ids = &bytes[..0];
        let mut work = vec![Work::Set(0)];
        while let Some(item) = work.pop() {
            match item {
                Work::Set(index) => {
                    let count = usize::from(u16::from_le_bytes(c.read()?));
                    let ids = c.take(count * 4)?;
                    if index == 0 {
                        root_ids = ids;
                    }
                    work.push(Work::Fields(index, ids));
                }
                Work::Fields(index, ids) => {
                    let Some((first, rest)) = ids.split_first_chunk::<4>() else {
                        continue;
                    };
                    let id = u32::from_le_bytes(*first);
                    let kind = (id >> 26) & 0x1f;
                    work.push(Work::Fields(index, rest));
                    let value = match kind {
                        1 | 2 => Value::NoData,
                        3..=6 => Value::Bytes(c.take(1 << (kind - 3))?),
                        7 => {
                            let count = u32::from_le_bytes(c.read()?);
                            if count >= 0x40000000 {
                                return Err(Error {
                                    offset: c.offset - 4,
                                    message: "Property data exceeds the format length limit",
                                });
                            }
                            Value::Bytes(c.take(usize::try_from(count).unwrap())?)
                        }
                        8..=13 => {
                            let count = if kind & 1 != 0 {
                                u32::from_le_bytes(c.read()?)
                            } else {
                                1
                            };
                            let stream_index = usize::try_from((kind - 8) / 2).unwrap();
                            let count = usize::try_from(count)
                                .ok()
                                .and_then(|count| count.checked_mul(4))
                                .ok_or(Error {
                                    offset: c.offset,
                                    message: "Reference count exceeds address space",
                                })?;
                            let compact_ids = streams[stream_index].take(count)?;
                            let stream = match stream_index {
                                0 => IdStream::Objects,
                                1 => IdStream::ObjectSpaces,
                                _ => IdStream::Contexts,
                            };
                            Value::References {
                                stream,
                                compact_ids,
                            }
                        }
                        16 | 17 => {
                            let count = if kind == 16 {
                                let count = u32::from_le_bytes(c.read()?);
                                if count != 0 {
                                    let element = u32::from_le_bytes(c.read()?);
                                    if (element >> 26) & 0x1f != 17 {
                                        return Err(Error {
                                            offset: c.offset - 4,
                                            message: "Property array contains a non-set element type",
                                        });
                                    }
                                }
                                usize::try_from(count).map_err(|_| Error {
                                    offset: c.offset,
                                    message: "Property-set count exceeds address space",
                                })?
                            } else {
                                1
                            };
                            let start = sets.len();
                            let end = start
                                .checked_add(count)
                                .filter(|end| *end <= bytes.len() / 2)
                                .ok_or(Error {
                                    offset: c.offset,
                                    message: "Property-set count exceeds the available data",
                                })?;
                            sets.resize_with(end, Vec::new);
                            for child in (start..end).rev() {
                                work.push(Work::Set(child));
                            }
                            Value::Sets(start..end)
                        }
                        _ => {
                            return Err(Error {
                                offset: c.offset,
                                message: "Unsupported property type",
                            });
                        }
                    };
                    sets[index].push(Property { id, value });
                }
            }
        }
        for stream in streams {
            if !stream.bytes.is_empty() {
                return Err(Error {
                    offset: stream.offset,
                    message: "Unconsumed property references",
                });
            }
        }
        if !c.bytes.is_empty() && (c.bytes.len() > 7 || !bytes.len().is_multiple_of(8)) {
            return Err(Error {
                offset: c.offset,
                message: "Trailing data exceeds property-set padding",
            });
        }
        Ok(Self {
            sets,
            padding: c.bytes,
            root_ids,
        })
    }
}

pub(crate) fn reference_streams<'a>(c: &mut Cursor<'a>) -> Result<[Cursor<'a>; 3], Error> {
    let mut streams = std::array::from_fn::<_, 3, _>(|_| Cursor {
        bytes: &[],
        offset: 0,
    });
    let mut extended = false;
    for (index, stream) in streams.iter_mut().enumerate() {
        let header = u32::from_le_bytes(c.read()?);
        let count = usize::try_from(header & 0xffffff).unwrap();
        if (index > 0 && header & 0x80000000 != 0)
            || (index == 0 && header & 0xc0000000 == 0xc0000000)
            || (index == 1 && (header & 0x40000000 != 0) != extended)
            || (index == 2 && header & 0x40000000 != 0)
        {
            return Err(Error {
                offset: c.offset - 4,
                message: "Inconsistent property reference-stream flags",
            });
        }
        let offset = c.offset;
        *stream = Cursor {
            bytes: c.take(count * 4)?,
            offset,
        };
        if index == 0 {
            extended = header & 0x40000000 != 0;
            if header & 0x80000000 != 0 {
                break;
            }
        } else if index == 1 && !extended {
            break;
        }
    }
    Ok(streams)
}
