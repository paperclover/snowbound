use std::io;

/// Observed directory metadata, not a stable notebook identity or a file snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryEntry {
    pub name: String,
    pub size: u64,
    /// LastWriteTime, FILETIME.
    pub modified: u64,
    /// MS-FSCC file attributes; directory is 0x10 and reparse point is 0x400.
    pub attributes: u32,
}

pub(super) fn decode(mut bytes: &[u8]) -> io::Result<Vec<DirectoryEntry>> {
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
            modified: u64::from_le_bytes(bytes[24..32].try_into().unwrap()),
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
        bytes[24..32].copy_from_slice(&(size + 7).to_le_bytes());
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
                modified: size + 7,
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
