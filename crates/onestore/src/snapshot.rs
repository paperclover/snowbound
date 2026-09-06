use crate::{RevisionIndex, Store};
use std::io;

fn read_exact(
    read: &mut impl FnMut(u64, &mut [u8]) -> io::Result<usize>,
    mut offset: u64,
    mut output: &mut [u8],
) -> io::Result<()> {
    while !output.is_empty() {
        match read(offset, output) {
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(count) if count <= output.len() => {
                offset += count as u64;
                output = &mut output[count..];
            }
            Ok(_) => return Err(io::ErrorKind::InvalidData.into()),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// Reads a bounded snapshot; the caller must provide fresh I/O and exclude in-place maintenance.
/// Includes unpublished trailing bytes so subsequent commits can validate the physical file.
pub fn read_snapshot(
    mut read: impl FnMut(u64, &mut [u8]) -> io::Result<usize>,
    limit: usize,
) -> io::Result<Option<Vec<u8>>> {
    let mut header = [0; 1024];
    read_exact(&mut read, 0, &mut header)?;
    let length = u64::from_le_bytes(header[196..204].try_into().unwrap());
    let length = usize::try_from(length)
        .ok()
        .filter(|length| (1024..=limit).contains(length))
        .ok_or(io::ErrorKind::InvalidData)?;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(length).map_err(io::Error::other)?;
    bytes.extend_from_slice(&header);
    bytes.resize(length, 0);
    if let Err(error) = read_exact(&mut read, 1024, &mut bytes[1024..]) {
        // Native writers can shorten unused storage before publishing the new expected length.
        return if error.kind() == io::ErrorKind::UnexpectedEof {
            Ok(None)
        } else {
            Err(error)
        };
    }
    let mut tail = [0; 65536];
    loop {
        let size = (limit - bytes.len()).clamp(1, tail.len());
        match read(bytes.len() as u64, &mut tail[..size]) {
            Ok(0) => break,
            Ok(count) if count <= size && count <= limit - bytes.len() => {
                bytes.try_reserve_exact(count).map_err(io::Error::other)?;
                bytes.extend_from_slice(&tail[..count]);
            }
            Ok(_) => return Err(io::ErrorKind::InvalidData.into()),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    let mut after = [0; 1024];
    read_exact(&mut read, 0, &mut after)?;
    if header != after {
        return Ok(None);
    }
    let parsed = Store::parse(&bytes).and_then(|store| {
        if !store.checksum_mismatches.is_empty() {
            return Ok(false);
        }
        RevisionIndex::parse(&store)?.validate_current()?;
        Ok(true)
    });
    Ok(matches!(parsed, Ok(true)).then_some(bytes))
}
