use super::*;
use smb2::msg::query_directory::{
    FileInformationClass, QueryDirectoryFlags, QueryDirectoryRequest, QueryDirectoryResponse,
};
use std::collections::BTreeMap;

/// Observed directory metadata, not a stable notebook identity or a file snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryEntry {
    pub name: String,
    pub size: u64,
    /// MS-FSCC file attributes; directory is 0x10 and reparse point is 0x400.
    pub attributes: u32,
}

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
                    .insert(entry.name, (entry.size, entry.attributes))
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
            .map(|(name, (size, attributes))| DirectoryEntry {
                name,
                size,
                attributes,
            })
            .collect())
    }
}

fn decode(mut bytes: &[u8]) -> io::Result<Vec<DirectoryEntry>> {
    if bytes.len() > 65536 {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let mut entries = Vec::new();
    loop {
        if bytes.len() < 64 {
            return Err(io::ErrorKind::InvalidData.into());
        }
        let next = u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize;
        let length = u32::from_le_bytes(bytes[60..64].try_into().unwrap()) as usize;
        let end = 64usize
            .checked_add(length)
            .ok_or(io::ErrorKind::InvalidData)?;
        if length == 0
            || !length.is_multiple_of(2)
            || end > bytes.len()
            || (next != 0 && (next < end || !next.is_multiple_of(8) || next >= bytes.len()))
            || (next == 0 && bytes.len() - end > 7)
        {
            return Err(io::ErrorKind::InvalidData.into());
        }
        let units: Vec<_> = bytes[64..end]
            .chunks_exact(2)
            .map(|unit| u16::from_le_bytes([unit[0], unit[1]]))
            .collect();
        let name = String::from_utf16(&units).map_err(|_| io::ErrorKind::InvalidData)?;
        if name.contains(['\0', '/', '\\']) {
            return Err(io::ErrorKind::InvalidData.into());
        }
        let size = i64::from_le_bytes(bytes[40..48].try_into().unwrap());
        entries.push(DirectoryEntry {
            name,
            size: size.try_into().map_err(|_| io::ErrorKind::InvalidData)?,
            attributes: u32::from_le_bytes(bytes[56..60].try_into().unwrap()),
        });
        if next == 0 {
            return Ok(entries);
        }
        bytes = &bytes[next..];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(name: &str, size: u64, attributes: u32) -> Vec<u8> {
        let mut bytes = vec![0; 64];
        bytes[40..48].copy_from_slice(&size.to_le_bytes());
        bytes[56..60].copy_from_slice(&attributes.to_le_bytes());
        let name: Vec<_> = name.encode_utf16().flat_map(u16::to_le_bytes).collect();
        bytes[60..64].copy_from_slice(&(name.len() as u32).to_le_bytes());
        bytes.extend(name);
        bytes
    }

    #[test]
    fn directory_records_preserve_names_sizes_and_attributes() {
        let mut bytes = Vec::new();
        let mut expected = Vec::new();
        for i in 0..300u32 {
            let name = format!("Section {i} 🦀 e\u{301}.one");
            let size = u64::from(i) * 100_000_000;
            let attributes = if i % 3 == 0 { 0x410 } else { 0x20 };
            let mut entry = record(&name, size, attributes);
            if i != 299 {
                entry.resize(entry.len().next_multiple_of(8), 0xa5);
                let length = entry.len() as u32;
                entry[..4].copy_from_slice(&length.to_le_bytes());
            }
            bytes.extend(entry);
            expected.push(DirectoryEntry {
                name,
                size,
                attributes,
            });
        }
        assert_eq!(decode(&bytes).unwrap(), expected);
        for end in 0..bytes.len() {
            assert!(decode(&bytes[..end]).is_err(), "accepted prefix {end}");
        }
    }

    #[test]
    fn invalid_directory_records_never_become_partial_results() {
        assert!(decode(&vec![0; 65537]).is_err());
        for name in ["", "bad\0name", "a/b", "a\\b"] {
            assert!(decode(&record(name, 0, 0)).is_err());
        }
        let valid = record("a.one", 12, 0x20);
        for (at, value) in [
            (0, 8),
            (0, 65),
            (0, 72),
            (0, u32::MAX),
            (60, 0),
            (60, 1),
            (60, u32::MAX),
            (44, u32::MAX),
        ] {
            let mut bytes = valid.clone();
            bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
            assert!(decode(&bytes).is_err(), "offset={at} value={value}");
        }
        let mut bytes = valid.clone();
        bytes[64..66].copy_from_slice(&0xd800u16.to_le_bytes());
        assert!(decode(&bytes).is_err());
        let mut bytes = valid;
        bytes.extend_from_slice(&[0; 8]);
        assert!(decode(&bytes).is_err());
        for name in [".", ".."] {
            assert_eq!(decode(&record(name, 0, 0x10)).unwrap()[0].name, name);
        }
    }
}
