//! Where section replicas live in the app's cache: a folder per notebook location, then a
//! file per section identity. OneNote lets a notebook folder be copied with its sections
//! keeping their identities, so identity alone cannot tell a copy's replica from the
//! original's; the location keeps each copy's queue its own.
//!
//! A location is a string naming where a notebook or lone section is reached: the canonical
//! path of a local folder or file (`local`), or `smb` for one on a share.

use crate::fs;
use onestore::{Header, Stamp};
use std::{
    io,
    path::{Path, PathBuf},
};

/// The file in a replica folder naming the location its replicas belong to.
const LOCATION: &str = "location";

/// The folder in `cache` holding the replicas of the sections at `location`.
pub fn folder(cache: &Path, location: &str) -> PathBuf {
    cache
        .join("replicas")
        .join(hex(&<sha2::Sha256 as sha2::Digest>::digest(location)[..16]))
}

/// The location of the local folder or file at `path`, which may have just been moved away:
/// its canonical path, or its folder's with its name.
pub fn local(path: &Path) -> io::Result<String> {
    let canonical =
        fs::canonicalize(path).or_else(|error| match (path.parent(), path.file_name()) {
            (Some(parent), Some(name)) => Ok(fs::canonicalize(parent)?.join(name)),
            _ => Err(error),
        })?;
    Ok(canonical.to_string_lossy().into_owned())
}

/// The location of the notebook folder `root` in `share` on the server at `address`, as its
/// host names them.
pub fn smb(address: &str, share: &str, root: &str) -> String {
    let root = root.replace('\\', "/");
    format!("smb://{address}/{share}/{}", root.trim_matches('/'))
}

/// Moves the replicas of the sections at `from` to `to`, as when the app renames or moves a
/// notebook; each replica `to` already holds for the same section stays. Every replica moved
/// must be closed.
pub fn moved(cache: &Path, from: &str, to: &str) -> io::Result<()> {
    let old = folder(cache, from);
    if fs::metadata(&old).is_err() {
        return Ok(());
    }
    let new = claimed(cache, to)?;
    for entry in fs::read_dir(&old)? {
        let name = entry?.file_name();
        if Path::new(&name).extension() == Some("sqlite".as_ref())
            && fs::metadata(new.join(&name)).is_err()
        {
            adopt(&old.join(&name), &new.join(&name))?;
        }
    }
    tidy(&old);
    Ok(())
}

/// Deletes what the cache keeps of the notebook at `location`, as when the app deletes the
/// notebook: its replicas, edits waiting in them included, its sections' copies' and its
/// listing. Every replica must be closed.
pub fn forget(cache: &Path, location: &str) -> io::Result<()> {
    let within = format!("{location}/");
    if let Ok(entries) = fs::read_dir(cache.join("replicas")) {
        for entry in entries {
            let folder = entry?.path();
            let ours = fs::read_to_string(folder.join(LOCATION))
                .is_ok_and(|label| label == location || label.starts_with(&within));
            if ours {
                fs::remove_dir_all(&folder)?;
            }
        }
    }
    match fs::remove_file(crate::session::listing(cache, location)) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// The folder for `location`, made and labelled with it where it was not.
fn claimed(cache: &Path, location: &str) -> io::Result<PathBuf> {
    let folder = folder(cache, location);
    let label = folder.join(LOCATION);
    if fs::metadata(&label).is_err() {
        fs::create_dir_all(&folder)?;
        fs::write(&label, location)?;
    }
    Ok(folder)
}

/// The folder for `location`, having moved into it a replica for each of `sections` it lacks
/// from where one was left behind:
///
/// - the cache's top or its `smb` folder, where replicas were named by identity alone before
///   locations named folders: taken by the first location to open the section, as that
///   replica served whichever copy opened then;
/// - the folder of a local location that no longer exists, as a notebook moved outside the
///   app leaves: taken only when the section file, as `stamp` reads it, stands as the
///   replica's base or has moved on from it (a later generation). A replica whose base is
///   newer than the file, or a different state of the same generation, belongs to another
///   copy of the section and stays.
///
/// A replica another process holds stays where it is.
pub(crate) fn claim(
    cache: &Path,
    location: &str,
    sections: &[[u8; 16]],
    stamp: impl Fn(&[u8; 16]) -> Option<Stamp>,
) -> io::Result<PathBuf> {
    let folder = claimed(cache, location)?;
    let missing: Vec<&[u8; 16]> = sections
        .iter()
        .filter(|identity| fs::metadata(crate::session::replica_file(&folder, identity)).is_err())
        .collect();
    if missing.is_empty() {
        return Ok(folder);
    }
    let orphans = orphans(cache);
    for identity in missing {
        let legacy = [cache.to_owned(), cache.join("smb")]
            .into_iter()
            .map(|legacy| (crate::session::replica_file(&legacy, identity), true));
        let left = orphans
            .iter()
            .map(|orphan| (crate::session::replica_file(orphan, identity), false));
        for (replica, legacy) in legacy.chain(left) {
            if fs::metadata(&replica).is_err() {
                continue;
            }
            let adoptable = match crate::closed(&replica).and_then(|held| crate::peek(&held)) {
                Ok((base, _)) => {
                    legacy || stamp(identity).is_some_and(|file| follows(&file, &base))
                }
                Err(error) if error.busy() => false,
                // An older schema, which opening converts, predates locations.
                Err(_) => legacy,
            };
            if adoptable {
                adopt(&replica, &crate::session::replica_file(&folder, identity))?;
                if let Some(parent) = replica.parent().filter(|_| !legacy) {
                    tidy(parent);
                }
                break;
            }
        }
    }
    Ok(folder)
}

/// The replica folders of local locations that no longer exist.
fn orphans(cache: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(cache.join("replicas")) else {
        return Vec::new();
    };
    entries
        .filter_map(|entry| {
            let folder = entry.ok()?.path();
            let location = fs::read_to_string(folder.join(LOCATION)).ok()?;
            let path = Path::new(&location);
            (path.is_absolute() && fs::metadata(path).is_err()).then_some(folder)
        })
        .collect()
}

/// Whether the section file standing as `file` may take a replica whose base is `base`.
fn follows(file: &Stamp, base: &Stamp) -> bool {
    file == base
        || matches!(
            (Header::parse(&file.header), Header::parse(&base.header)),
            (Ok(file), Ok(base)) if file.generation > base.generation
        )
}

/// Moves the replica at `from`, with the write-ahead log a crash may have left, to `to`.
fn adopt(from: &Path, to: &Path) -> io::Result<()> {
    for suffix in ["-wal", "-shm", ""] {
        let (mut source, mut target) = (from.as_os_str().to_owned(), to.as_os_str().to_owned());
        source.push(suffix);
        target.push(suffix);
        match fs::rename(&source, &target) {
            Err(error) if !suffix.is_empty() && error.kind() == io::ErrorKind::NotFound => {}
            moved => moved?,
        }
    }
    Ok(())
}

/// Removes a replica folder that holds no replica.
fn tidy(folder: &Path) {
    let empty = fs::read_dir(folder).is_ok_and(|mut entries| {
        entries.all(|entry| entry.is_ok_and(|entry| entry.file_name() == LOCATION))
    });
    if empty {
        let _ = fs::remove_file(folder.join(LOCATION));
        let _ = fs::remove_dir(folder);
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
