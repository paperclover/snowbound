//! Snowbound's own files in a notebook folder, kept in `.snowbound`. OneNote 2010 makes a
//! section group of every subfolder but one with the Windows hidden attribute, so the
//! folder takes the attribute when made, and again whenever Snowbound writes to it; the dot
//! name hides it on Samba's defaults, macOS and Linux as well. Each feature keeps its own
//! names inside, and a notebook without the folder is whole.
//!
//! Tag art: `tags.json` maps a tag, by the name and symbol its definition stores, to a
//! picture in `tags/` named for its content, `<SHA-256>.png` or `.svg`, which is never
//! rewritten. A writer rereads the mapping, merges its own and replaces the file, then reads
//! it back and merges again until its own holds. The merge keeps every tag either side
//! mapped; where both mapped one tag, the later mapping wins, then the greater art name, so
//! writers agree whatever order they read in.

use crate::{Result, session::Storage};
use serde::{Deserialize, Serialize};
use std::io;

const FOLDER: &str = ".snowbound";
const MAPPING: &str = ".snowbound/tags.json";
const ART: &str = ".snowbound/tags";
/// The most bytes read of a mapping or a picture.
pub const LIMIT: usize = 1 << 20;

/// A tag drawn with art of Snowbound's.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TagMapping {
    /// The tag's name and symbol, as its definition stores them.
    pub name: String,
    pub shape: u16,
    /// Its picture in `tags/`.
    pub art: String,
    /// When it was mapped, as a FILETIME.
    pub mapped: u64,
}

/// The name `tags/` keeps a picture under: its SHA-256 and its extension, `png` or `svg`.
pub fn art_name(bytes: &[u8], extension: &str) -> String {
    let digest = <sha2::Sha256 as sha2::Digest>::digest(bytes);
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("{hex}.{extension}")
}

/// Whether `name` could be a picture's in `tags/`, and so names nothing else.
fn art_named(name: &str) -> bool {
    name.split_once('.').is_some_and(|(hash, extension)| {
        hash.len() == 64
            && hash
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
            && matches!(extension, "png" | "svg")
    })
}

/// Merges `mappings` into `into`, by the rule the module describes.
pub fn merge(into: &mut Vec<TagMapping>, mappings: impl IntoIterator<Item = TagMapping>) {
    for mapping in mappings {
        match into
            .iter_mut()
            .find(|kept| kept.name == mapping.name && kept.shape == mapping.shape)
        {
            Some(kept) => {
                if (mapping.mapped, &mapping.art) > (kept.mapped, &kept.art) {
                    *kept = mapping;
                }
            }
            None => into.push(mapping),
        }
    }
}

/// The mappings `tags.json` holds: none without it, and none it holds unreadably.
pub(crate) fn mappings(storage: &dyn Storage) -> Result<Vec<TagMapping>> {
    let bytes = match storage.read_file(MAPPING, LIMIT) {
        Err(crate::Error::Io(error)) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(Vec::new());
        }
        bytes => bytes?,
    };
    let values: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap_or_default();
    let mut mappings = Vec::new();
    merge(
        &mut mappings,
        values
            .into_iter()
            .filter_map(|value| serde_json::from_value::<TagMapping>(value).ok())
            .filter(|mapping| art_named(&mapping.art)),
    );
    Ok(mappings)
}

/// Picture `art` from `tags/`, once its bytes match its name.
pub(crate) fn art(storage: &dyn Storage, art: &str) -> Result<Vec<u8>> {
    let extension = art.rsplit('.').next().unwrap_or_default();
    if !art_named(art) {
        return Err(io::Error::from(io::ErrorKind::InvalidInput).into());
    }
    let bytes = storage.read_file(&format!("{ART}/{art}"), LIMIT)?;
    if art_name(&bytes, extension) != art {
        return Err(io::Error::from(io::ErrorKind::InvalidData).into());
    }
    Ok(bytes)
}

/// Maps tag `name` with symbol `shape` to picture `bytes`, a PNG or SVG as `extension`
/// says, now; returns the mappings as they then stand.
pub(crate) fn map(
    storage: &dyn Storage,
    name: &str,
    shape: u16,
    bytes: &[u8],
    extension: &str,
) -> Result<Vec<TagMapping>> {
    if !matches!(extension, "png" | "svg") || bytes.len() > LIMIT {
        return Err(io::Error::from(io::ErrorKind::InvalidInput).into());
    }
    let mapping = TagMapping {
        name: name.to_owned(),
        shape,
        art: art_name(bytes, extension),
        mapped: crate::now(),
    };
    for folder in [FOLDER, ART] {
        match storage.create_directory(folder) {
            Err(crate::Error::Io(error)) if error.kind() == io::ErrorKind::AlreadyExists => {}
            created => created?,
        }
    }
    storage.hide(FOLDER)?;
    let picture = format!("{ART}/{}", mapping.art);
    if !storage.exists(&picture) {
        let written = temporary(&picture);
        storage.create(&written, bytes)?;
        // Another writer may have kept the same picture meanwhile.
        if let Err(error) = storage.rename(&written, &picture) {
            storage.delete(&written)?;
            if !storage.exists(&picture) {
                return Err(error);
            }
        }
    }
    for _ in 0..3 {
        let mut merged = mappings(storage)?;
        merge(&mut merged, [mapping.clone()]);
        let written = temporary(MAPPING);
        let json = serde_json::to_vec_pretty(&merged).map_err(io::Error::from)?;
        storage.create(&written, &json)?;
        storage.replace(&written, MAPPING)?;
        let kept = mappings(storage)?;
        let mut held = kept.clone();
        merge(&mut held, [mapping.clone()]);
        if held == kept {
            return Ok(kept);
        }
    }
    Err(io::Error::from(io::ErrorKind::ResourceBusy).into())
}

/// A name beside `path` no other writer picks.
fn temporary(path: &str) -> String {
    use std::hash::{BuildHasher, RandomState};
    let unique = RandomState::new().hash_one((crate::fs::process_id(), crate::now()));
    format!("{path}.{unique:016x}.tmp")
}

pub mod themes;

#[cfg(test)]
mod tests;
