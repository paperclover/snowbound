use crate::{RevisionIndex, Store};
use std::io;

pub(crate) fn read_exact(
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
/// `None` is a torn read or storage that does not validate.
pub fn read_snapshot(
    read: impl FnMut(u64, &mut [u8]) -> io::Result<usize>,
    limit: usize,
) -> io::Result<Option<Vec<u8>>> {
    let Some(bytes) = image(read, limit)? else {
        return Ok(None);
    };
    let valid = Store::parse(&bytes).is_ok_and(|store| {
        store.checksum_mismatches.is_empty()
            && RevisionIndex::parse(&store).is_ok_and(|index| index.validate_current().is_ok())
    });
    Ok(valid.then_some(bytes))
}

/// Reads stable storage and checksums without requiring traversable property references.
/// Used to inspect encrypted or incomplete documents; this does not establish edit readiness.
/// The caller must provide fresh I/O and exclude in-place maintenance, as for `read_snapshot`.
/// `None` is a torn read; stable storage that fails to parse or checksum is `InvalidData`.
pub fn read_storage_snapshot(
    read: impl FnMut(u64, &mut [u8]) -> io::Result<usize>,
    limit: usize,
) -> io::Result<Option<Vec<u8>>> {
    let Some(bytes) = image(read, limit)? else {
        return Ok(None);
    };
    let store =
        Store::parse(&bytes).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    if !store.checksum_mismatches.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Checksum mismatch",
        ));
    }
    Ok(Some(bytes))
}

/// The file's bytes, or `None` where its header changed or its storage ended early meanwhile.
fn image(
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
    Ok((header == after).then_some(bytes))
}
