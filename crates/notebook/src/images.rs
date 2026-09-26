//! The cache's two section images. `base` is stored whole; `working` is stored as the byte
//! ranges where it differs from `base`, so saving an edit writes the revision it appended
//! instead of the section.

use crate::Result;
use onestore::Stamp;
use rusqlite::{Connection, params};
use std::io;

const BLOCK: usize = 4096;

/// `image` as its length and the `(offset, bytes)` runs of blocks that differ from `base`;
/// empty when the images are equal.
fn difference(base: &[u8], image: &[u8]) -> Vec<u8> {
    if base == image {
        return Vec::new();
    }
    let mut patch = (image.len() as u64).to_le_bytes().to_vec();
    let mut run: Option<usize> = None;
    let close = |patch: &mut Vec<u8>, start: usize, end: usize| {
        patch.extend_from_slice(&(start as u64).to_le_bytes());
        patch.extend_from_slice(&((end - start) as u64).to_le_bytes());
        patch.extend_from_slice(&image[start..end]);
    };
    for start in (0..image.len()).step_by(BLOCK) {
        let end = (start + BLOCK).min(image.len());
        if base.get(start..end) == Some(&image[start..end]) {
            if let Some(from) = run.take() {
                close(&mut patch, from, start);
            }
        } else {
            run.get_or_insert(start);
        }
    }
    if let Some(from) = run {
        close(&mut patch, from, image.len());
    }
    patch
}

fn restore(mut base: Vec<u8>, patch: &[u8]) -> Result<Vec<u8>> {
    let damaged = || io::Error::new(io::ErrorKind::InvalidData, "Damaged working image");
    let Some((length, mut runs)) = patch.split_first_chunk::<8>() else {
        return if patch.is_empty() {
            Ok(base)
        } else {
            Err(damaged().into())
        };
    };
    // Every block past the base is a run, so a whole patch bounds the image.
    let length = usize::try_from(u64::from_le_bytes(*length))
        .ok()
        .filter(|length| *length <= base.len() + patch.len())
        .ok_or_else(damaged)?;
    base.resize(length, 0);
    while let Some((header, rest)) = runs.split_first_chunk::<16>() {
        let offset = usize::try_from(u64::from_le_bytes(header[..8].try_into().unwrap()))
            .map_err(|_| damaged())?;
        let size = usize::try_from(u64::from_le_bytes(header[8..].try_into().unwrap()))
            .map_err(|_| damaged())?;
        let (bytes, rest) = rest.split_at_checked(size).ok_or_else(damaged)?;
        base.get_mut(offset..)
            .and_then(|target| target.get_mut(..size))
            .ok_or_else(damaged)?
            .copy_from_slice(bytes);
        runs = rest;
    }
    if runs.is_empty() {
        Ok(base)
    } else {
        Err(damaged().into())
    }
}

pub(crate) fn base(connection: &Connection) -> Result<Vec<u8>> {
    Ok(connection.query_row("SELECT base FROM replica WHERE id=1", [], |row| row.get(0))?)
}

/// `(base, working)`.
pub(crate) fn both(connection: &Connection) -> Result<(Vec<u8>, Vec<u8>)> {
    let (base, patch): (Vec<u8>, Vec<u8>) =
        connection.query_row("SELECT base, working FROM replica WHERE id=1", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?;
    let working = restore(base.clone(), &patch)?;
    Ok((base, working))
}

/// The working image's length, without restoring it.
pub(crate) fn working_length(connection: &Connection) -> Result<u64> {
    Ok(connection.query_row(
        "SELECT length(base), substr(working, 1, 8) FROM replica WHERE id=1",
        [],
        |row| {
            // SQLite's substr of an empty blob is NULL.
            let header: Option<Vec<u8>> = row.get(1)?;
            Ok(match header.as_deref().and_then(<[u8]>::first_chunk::<8>) {
                Some(length) => u64::from_le_bytes(*length),
                None => row.get::<_, i64>(0)? as u64,
            })
        },
    )?)
}

/// Whether no edit is queued and the working image is the base image, without reading
/// either image.
pub(crate) fn settled(connection: &Connection) -> Result<bool> {
    Ok(connection.query_row(
        "SELECT length(working) = 0 AND NOT EXISTS (SELECT 1 FROM edits) FROM replica WHERE id=1",
        [],
        |row| row.get(0),
    )?)
}

/// The base image's stamp, without reading the image.
pub(crate) fn stamp(connection: &Connection) -> Result<Stamp> {
    let (header, length): (Vec<u8>, i64) = connection.query_row(
        "SELECT substr(base, 1, 1024), length(base) FROM replica WHERE id=1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    Ok(Stamp {
        header: header
            .try_into()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "Damaged base image"))?,
        length: u64::try_from(length).map_err(io::Error::other)?,
    })
}

pub(crate) fn working(connection: &Connection) -> Result<Vec<u8>> {
    let (base, patch): (Vec<u8>, Vec<u8>) =
        connection.query_row("SELECT base, working FROM replica WHERE id=1", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?;
    restore(base, &patch)
}

/// Replaces the working image; `base` is the stored base image.
pub(crate) fn set_working(connection: &Connection, base: &[u8], working: &[u8]) -> Result<()> {
    connection.execute(
        "UPDATE replica SET working=?1 WHERE id=1",
        [difference(base, working)],
    )?;
    Ok(())
}

/// Replaces the base image, keeping `working` as the working image. An unchanged base
/// leaves its stored bytes alone.
pub(crate) fn set_base(
    connection: &Connection,
    old: &[u8],
    base: &[u8],
    working: &[u8],
) -> Result<()> {
    if old == base {
        return set_working(connection, base, working);
    }
    connection.execute(
        "UPDATE replica SET base=?1, working=?2 WHERE id=1",
        params![base, difference(base, working)],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_working_image_survives_as_its_difference_from_any_base() {
        let base: Vec<u8> = (0..3 * BLOCK + 17).map(|i| i as u8).collect();
        let mut edited = base.clone();
        edited[5] ^= 1;
        edited[2 * BLOCK + 1] ^= 1;
        edited.extend_from_slice(b"appended revision");
        for image in [
            base.clone(),
            edited.clone(),
            base[..BLOCK + 3].to_vec(),
            Vec::new(),
            vec![7; 5 * BLOCK],
        ] {
            let patch = difference(&base, &image);
            assert_eq!(restore(base.clone(), &patch).unwrap(), image);
        }
        assert!(difference(&base, &base).is_empty());
        assert!(difference(&base, &edited).len() < 3 * BLOCK);
        let mut huge = difference(&base, &edited);
        huge[..8].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(restore(base.clone(), &huge).is_err());
        let patch = difference(&base, &edited);
        for cut in 1..patch.len() {
            if let Ok(image) = restore(base.clone(), &patch[..cut]) {
                assert_ne!(image, edited);
            }
        }
    }
}
