use crate::{
    Chunk, Error, ExGuid, FileType, PropertySets, Reference, RevisionIndex, Store,
    store::crc,
    write::{append, append_list, fresh_guid, node},
};
use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

type Result<T> = std::result::Result<T, Error>;

pub(crate) fn string(value: &str) -> Vec<u8> {
    value
        .encode_utf16()
        .chain([0])
        .flat_map(u16::to_le_bytes)
        .collect()
}

pub(crate) fn default_text_style() -> Vec<(u32, Vec<u8>)> {
    vec![
        (0x14001c3b, 0x409_u32.to_le_bytes().to_vec()),
        (0x1c001c0a, string("Calibri")),
        (0x10001c0b, 22_u16.to_le_bytes().to_vec()),
    ]
}

pub(crate) fn properties(values: &[(u32, Vec<u8>)]) -> Result<Vec<u8>> {
    let mut streams: [Vec<u8>; 3] = std::array::from_fn(|_| Vec::new());
    let mut fields = Vec::new();
    for (id, value) in values {
        let kind = (id >> 26) & 0x1f;
        match kind {
            1 | 2 => assert!(value.is_empty()),
            3..=6 => {
                assert_eq!(value.len(), 1 << (kind - 3));
                fields.extend(value);
            }
            7 => {
                if value.len() >= 0x40000000 {
                    return Err(Error {
                        offset: 0,
                        message: "Property data exceeds the format length limit",
                    });
                }
                fields.extend_from_slice(&(value.len() as u32).to_le_bytes());
                fields.extend(value);
            }
            8..=13 => {
                assert!(value.len().is_multiple_of(4));
                if kind & 1 == 0 {
                    assert_eq!(value.len(), 4);
                } else {
                    fields
                        .extend_from_slice(&u32::try_from(value.len() / 4).unwrap().to_le_bytes());
                }
                streams[((kind - 8) / 2) as usize].extend(value);
            }
            _ => unreachable!(),
        }
    }
    let mut data = Vec::new();
    let extended = !streams[2].is_empty();
    let last = if extended {
        2
    } else if !streams[1].is_empty() {
        1
    } else {
        0
    };
    for (index, stream) in streams.iter().enumerate().take(last + 1) {
        let mut header = u32::try_from(stream.len() / 4).unwrap();
        assert!(header <= 0xffffff);
        if index == 0 && last == 0 {
            header |= 0x80000000;
        }
        if index < 2 && extended {
            header |= 0x40000000;
        }
        data.extend_from_slice(&header.to_le_bytes());
        data.extend(stream);
    }
    data.extend_from_slice(&u16::try_from(values.len()).unwrap().to_le_bytes());
    for (id, _) in values {
        data.extend_from_slice(&id.to_le_bytes());
    }
    data.extend(fields);
    data.resize(data.len().next_multiple_of(8), 0);
    PropertySets::parse(&data)?;
    Ok(data)
}

struct NewObject {
    id: u32,
    jcid: u32,
    properties: Vec<(u32, Vec<u8>)>,
}

struct NewSpace {
    id: u32,
    roots: Vec<(u32, u32)>,
    objects: Vec<NewObject>,
}

pub(crate) fn current_timestamps() -> Result<(u32, u64)> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| Error {
            offset: 0,
            message: "System time precedes the Unix epoch",
        })?
        .as_secs();
    let modified = now
        .checked_sub(315532800)
        .and_then(|time| u32::try_from(time).ok())
        .ok_or(Error {
            offset: 0,
            message: "System time exceeds the OneNote Time32 range",
        })?;
    Ok((modified, (now + 11644473600) * 10000000))
}

/// Creates a section with one page and one plain-text paragraph, without a template.
/// Text must not contain NUL or line-feed characters.
pub fn create_section(file_name: &str, text: &str, author: &str) -> Result<Vec<u8>> {
    if file_name.contains(['/', '\\', '\0'])
        || !file_name.to_ascii_lowercase().ends_with(".one")
        || text.contains(['\0', '\n'])
        || author.contains('\0')
    {
        return Err(Error {
            offset: 0,
            message: "Invalid section name, paragraph text, or author",
        });
    }
    let (modified, timestamp) = current_timestamps()?;
    let timestamp = timestamp.to_le_bytes().to_vec();
    let page_guid = fresh_guid()?;
    let metadata = vec![
        (0x1c001c30, page_guid.to_vec()),
        (0x1c001cf3, string(crate::edit::automatic_title(text))),
        (0x14001d82, 40_u32.to_le_bytes().to_vec()),
        (0x1400348b, 40_u32.to_le_bytes().to_vec()),
        (0x14001dff, 1_u32.to_le_bytes().to_vec()),
        (0x18001c65, timestamp.clone()),
    ];
    let id = |value: u32| value.to_le_bytes().to_vec();
    let last_modified = || (0x14001d7a, modified.to_le_bytes().to_vec());
    let spaces = vec![
        NewSpace {
            id: 1,
            roots: vec![(1, 10), (2, 11)],
            objects: vec![
                NewObject {
                    id: 10,
                    jcid: 0x60007,
                    properties: vec![
                        (0x1c001c30, fresh_guid()?.to_vec()),
                        (0x18001c65, timestamp.clone()),
                        (0x24001c20, id(12)),
                    ],
                },
                NewObject {
                    id: 11,
                    jcid: 0x20031,
                    properties: vec![
                        (0x14001d82, id(40)),
                        (0x1400348b, id(40)),
                        (0x14001cbe, vec![0x8a, 0xa8, 0xe4, 0]),
                    ],
                },
                NewObject {
                    id: 12,
                    jcid: 0x60008,
                    properties: vec![
                        (0x1c001c30, fresh_guid()?.to_vec()),
                        (0x18001c65, timestamp),
                        (0x2c001d63, id(257)),
                    ],
                },
            ],
        },
        NewSpace {
            id: 257,
            roots: vec![(1, 20), (2, 21)],
            objects: vec![
                NewObject {
                    id: 20,
                    jcid: 0x60037,
                    properties: vec![(0x24001c1f, id(22))],
                },
                NewObject {
                    id: 21,
                    jcid: 0x20030,
                    properties: metadata,
                },
                NewObject {
                    id: 22,
                    jcid: 0x6000b,
                    properties: vec![
                        last_modified(),
                        (0x24001c20, id(23)),
                        (0x1c001d75, string(author)),
                        (0x1c001d3c, string(crate::edit::automatic_title(text))),
                    ],
                },
                NewObject {
                    id: 23,
                    jcid: 0x6000c,
                    properties: vec![
                        last_modified(),
                        (0x24001c20, id(24)),
                        (0x0c001c03, vec![1]),
                        (0x1c001c12, vec![1, 0, 0, 0, 0, 0, 0, 0]),
                        (0x14001c14, 1_f32.to_le_bytes().to_vec()),
                        (0x14001c15, 1_f32.to_le_bytes().to_vec()),
                        (0x14001c1b, 13_f32.to_le_bytes().to_vec()),
                        (0x14001c1c, 0.6_f32.to_le_bytes().to_vec()),
                    ],
                },
                NewObject {
                    id: 24,
                    jcid: 0x6000d,
                    properties: vec![
                        last_modified(),
                        (0x24001c1f, id(25)),
                        (0x0c001c03, vec![1]),
                        (0x20001d78, id(26)),
                        (0x20001d79, id(26)),
                        (0x14001d09, modified.to_le_bytes().to_vec()),
                    ],
                },
                NewObject {
                    id: 25,
                    jcid: 0x6000e,
                    properties: vec![
                        last_modified(),
                        (0x1c001c22, string(text)),
                        (0x24001e13, id(27)),
                        (0x10001cfe, 0x409_u16.to_le_bytes().to_vec()),
                    ],
                },
                NewObject {
                    id: 26,
                    jcid: 0x120001,
                    properties: vec![(0x1c001d75, string(author))],
                },
                NewObject {
                    id: 27,
                    jcid: 0x12004d,
                    properties: default_text_style(),
                },
            ],
        },
    ];
    create(file_name, FileType::Section, spaces)
}

/// Creates a notebook table of contents from section filenames and file identities.
pub fn create_table_of_contents(file_name: &str, sections: &[(&str, [u8; 16])]) -> Result<Vec<u8>> {
    if file_name.contains(['/', '\\', '\0'])
        || !file_name.to_ascii_lowercase().ends_with(".onetoc2")
        || sections.len() > 0xfffffe
    {
        return Err(Error {
            offset: 0,
            message: "Invalid table-of-contents filename or section count",
        });
    }
    let mut objects = Vec::new();
    let mut children = Vec::new();
    let mut names = std::collections::BTreeSet::new();
    let mut identities = std::collections::BTreeSet::new();
    for (index, (name, identity)) in sections.iter().enumerate() {
        if name.contains(['/', '\\', '\0'])
            || !name.to_ascii_lowercase().ends_with(".one")
            || !names.insert(name.to_lowercase())
            || *identity == [0; 16]
            || !identities.insert(*identity)
        {
            return Err(Error {
                offset: 0,
                message: "Invalid or duplicate section filename or identity",
            });
        }
        let id = (((index + 1) as u32) << 8) | 10;
        children.extend_from_slice(&id.to_le_bytes());
        objects.push(NewObject {
            id,
            jcid: 0x20001,
            properties: vec![
                (0x1c001d94, identity.to_vec()),
                (0x14001cb9, ((index + 1) as u32).to_le_bytes().to_vec()),
                (0x1c001d6b, string(name)),
                (0x14001cbe, vec![0xff; 4]),
            ],
        });
    }
    objects.push(NewObject {
        id: 10,
        jcid: 0x20001,
        properties: vec![(0x24001cf6, children)],
    });
    create(
        file_name,
        FileType::TableOfContents,
        vec![NewSpace {
            id: 1,
            roots: vec![(1, 10)],
            objects,
        }],
    )
}

fn create(file_name: &str, file_type: FileType, spaces: Vec<NewSpace>) -> Result<Vec<u8>> {
    let is_section = file_type == FileType::Section;
    let maximum = spaces
        .iter()
        .flat_map(|space| {
            std::iter::once(space.id).chain(space.objects.iter().map(|object| object.id))
        })
        .max()
        .unwrap();
    let guids = (0..=(maximum >> 8))
        .map(|_| fresh_guid())
        .collect::<Result<Vec<_>>>()?;
    let exguid = |compact: u32| ExGuid {
        guid: guids[(compact >> 8) as usize],
        n: compact & 255,
    };
    let mut output = vec![0; 1024];
    let mut root_payload = Vec::new();
    exguid(spaces[0].id).encode(&mut root_payload);
    let mut root = Vec::new();
    let mut log = Vec::new();
    let mut list_id = 16_u32;
    let mut hashed = Vec::new();
    for space in spaces {
        let mut counts = BTreeMap::<u32, u32>::new();
        for (_, id) in &space.roots {
            *counts.entry(*id).or_default() += 1;
        }
        for object in &space.objects {
            for (property, value) in &object.properties {
                if matches!((property >> 26) & 0x1f, 8 | 9) {
                    for id in value.chunks_exact(4) {
                        *counts
                            .entry(u32::from_le_bytes(id.try_into().unwrap()))
                            .or_default() += 1;
                    }
                }
            }
        }
        let group_id = ExGuid {
            guid: fresh_guid()?,
            n: 1,
        };
        let mut group_payload = Vec::new();
        group_id.encode(&mut group_payload);
        let mut group = if is_section {
            vec![node(0xb4, None, &group_payload)?, node(0x22, None, &[])?]
        } else {
            vec![node(0x21, None, &[0])?]
        };
        for (index, guid) in guids.iter().enumerate() {
            let mut entry = (index as u32).to_le_bytes().to_vec();
            entry.extend_from_slice(guid);
            group.push(node(0x24, None, &entry)?);
        }
        group.push(node(0x28, None, &[])?);
        let mut override_crc = if is_section { u32::MAX } else { 0 };
        for object in space.objects {
            let data = properties(&object.properties)?;
            let chunk = append(&mut output, &data)?;
            let mut payload = object.id.to_le_bytes().to_vec();
            let mut flags = 0;
            for (property, value) in object.properties {
                if !value.is_empty() {
                    match (property >> 26) & 0x1f {
                        8 | 9 => flags |= 1,
                        10..=13 => flags |= 2,
                        _ => {}
                    }
                }
            }
            if is_section {
                payload.extend_from_slice(&object.jcid.to_le_bytes());
                payload.push(flags);
            } else {
                let body = 1_u64 | (u64::from(flags & 1) << 16);
                payload.extend_from_slice(&body.to_le_bytes()[..6]);
            }
            let count = counts.get(&object.id).copied().unwrap_or(0).to_le_bytes();
            payload.extend_from_slice(&count);
            override_crc = crc(override_crc, &count, file_type);
            let readonly = object.jcid & 0x100000 != 0;
            if readonly {
                let hash = md5::compute(&data).0;
                payload.extend_from_slice(&hash);
                hashed.push(node(0xc2, Some(Reference::Data(chunk)), &hash)?);
            }
            group.push(node(
                if !is_section {
                    0x2e
                } else if readonly {
                    0xc5
                } else {
                    0xa5
                },
                Some(Reference::Data(chunk)),
                &payload,
            )?);
        }
        let group_chunk = if is_section {
            group.push(node(0xb8, None, &[])?);
            Some(append_list(&mut output, list_id, &group)?)
        } else {
            None
        };
        let group_count = group.len();
        let mut start = Vec::new();
        exguid(space.id).encode(&mut start);
        start.extend_from_slice(&0_u32.to_le_bytes());
        let mut revision = Vec::new();
        ExGuid {
            guid: fresh_guid()?,
            n: 1,
        }
        .encode(&mut revision);
        ExGuid::default().encode(&mut revision);
        if !is_section {
            revision.extend_from_slice(&0_u64.to_le_bytes());
        }
        revision.extend_from_slice(&1_u32.to_le_bytes());
        revision.extend_from_slice(&0_u16.to_le_bytes());
        let mut manifest = vec![
            node(0x14, None, &start)?,
            node(if is_section { 0x1e } else { 0x1b }, None, &revision)?,
        ];
        if let Some(chunk) = group_chunk {
            manifest.push(node(
                0xb0,
                Some(Reference::NodeList(chunk)),
                &group_payload,
            )?);
            let mut overrides = vec![0; 8];
            overrides.extend_from_slice(&(!override_crc).to_le_bytes());
            manifest.push(node(
                0x84,
                Some(Reference::Data(Chunk {
                    offset: u64::MAX,
                    length: 0,
                })),
                &overrides,
            )?);
        } else {
            manifest.extend(group);
        }
        for (role, oid) in space.roots {
            let mut payload = Vec::new();
            if is_section {
                exguid(oid).encode(&mut payload);
            } else {
                payload.extend_from_slice(&oid.to_le_bytes());
            }
            payload.extend_from_slice(&role.to_le_bytes());
            manifest.push(node(if is_section { 0x5a } else { 0x59 }, None, &payload)?);
        }
        manifest.push(node(0x1c, None, &[])?);
        let manifest_chunk = append_list(&mut output, list_id + 1, &manifest)?;
        let mut space_payload = Vec::new();
        exguid(space.id).encode(&mut space_payload);
        let space_nodes = vec![
            node(0xc, None, &space_payload)?,
            node(0x10, Some(Reference::NodeList(manifest_chunk)), &[])?,
        ];
        let space_chunk = append_list(&mut output, list_id + 2, &space_nodes)?;
        root.push(node(
            8,
            Some(Reference::NodeList(space_chunk)),
            &space_payload,
        )?);
        for (id, count) in [
            (list_id, group_count),
            (list_id + 1, manifest.len()),
            (list_id + 2, space_nodes.len()),
        ] {
            if id == list_id && !is_section {
                continue;
            }
            log.extend_from_slice(&id.to_le_bytes());
            log.extend_from_slice(&u32::try_from(count).unwrap().to_le_bytes());
        }
        list_id += 3;
    }
    root.push(node(4, None, &root_payload)?);
    let root_chunk = append_list(&mut output, list_id, &root)?;
    log.extend_from_slice(&list_id.to_le_bytes());
    log.extend_from_slice(&u32::try_from(root.len()).unwrap().to_le_bytes());
    let hashed_chunk = if !hashed.is_empty() {
        let chunk = append_list(&mut output, list_id + 1, &hashed)?;
        log.extend_from_slice(&(list_id + 1).to_le_bytes());
        log.extend_from_slice(&u32::try_from(hashed.len()).unwrap().to_le_bytes());
        Some(chunk)
    } else {
        None
    };
    let checksum = if is_section {
        !crc(u32::MAX, &log, file_type)
    } else {
        crc(0, &log, file_type)
    };
    log.extend_from_slice(&1_u32.to_le_bytes());
    log.extend_from_slice(&checksum.to_le_bytes());
    log.extend_from_slice(&u64::MAX.to_le_bytes());
    log.extend_from_slice(&0_u32.to_le_bytes());
    let log_chunk = append(&mut output, &log)?;
    output[..16].copy_from_slice(&[
        0xe4, 0x52, 0x5c, 0x7b, 0x8c, 0xd8, 0xa7, 0x4d, 0xae, 0xb1, 0x53, 0x78, 0xd0, 0x29, 0x96,
        0xd3,
    ]);
    if !is_section {
        output[..16].copy_from_slice(&[
            0xa1, 0x2f, 0xff, 0x43, 0xd9, 0xef, 0x76, 0x4c, 0x9e, 0xe2, 0x10, 0xea, 0x57, 0x22,
            0x76, 0x5f,
        ]);
    }
    output[16..32].copy_from_slice(&fresh_guid()?);
    output[48..64].copy_from_slice(&[
        0x3f, 0xdd, 0x9a, 0x10, 0x1b, 0x91, 0xf5, 0x49, 0xa5, 0xd0, 0x17, 0x91, 0xed, 0xc8, 0xae,
        0xd8,
    ]);
    for at in [64, 68, 72, 76] {
        output[at..at + 4]
            .copy_from_slice(&(if is_section { 42_u32 } else { 27_u32 }).to_le_bytes());
    }
    for at in [88, 112] {
        output[at..at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    }
    output[96..100].copy_from_slice(&1_u32.to_le_bytes());
    output[144..148]
        .copy_from_slice(&(!crc(u32::MAX, &string(file_name), FileType::Section)).to_le_bytes());
    for (at, chunk) in hashed_chunk
        .map(|chunk| (148, chunk))
        .into_iter()
        .chain([(160, log_chunk), (172, root_chunk)])
    {
        output[at..at + 8].copy_from_slice(&chunk.offset.to_le_bytes());
        output[at + 8..at + 12].copy_from_slice(&(chunk.length as u32).to_le_bytes());
    }
    let length = output.len() as u64;
    output[196..204].copy_from_slice(&length.to_le_bytes());
    output[212..228].copy_from_slice(&fresh_guid()?);
    output[228..236].copy_from_slice(&1_u64.to_le_bytes());
    output[236..252].copy_from_slice(&fresh_guid()?);
    let store = Store::parse(&output)?;
    RevisionIndex::parse(&store)?.validate_current()?;
    Ok(output)
}
