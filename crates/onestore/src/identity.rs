//! A file's identity and place, rewritten in an image held whole: OneNote 2010 saves a
//! section as a copy, and writes a package's files, outside any notebook, and gives each
//! file it unpacks an identity of its own (`corpus/notebook-package`).

use crate::{Error, Header, create::placement, write::fresh_guid};

type Result<T> = std::result::Result<T, Error>;

/// Gives the file `image` holds a new `guidFile` and version (`guidFileVersion`,
/// `nFileVersionGeneration`, `guidDenyReadFileVersion`), returning the identity.
pub fn reidentify(image: &mut [u8]) -> Result<[u8; 16]> {
    let header = Header::parse(image)?;
    let identity = fresh_guid()?;
    let generation = header.generation.checked_add(1).ok_or(Error {
        offset: 228,
        message: "File generation counter is exhausted",
    })?;
    image[16..32].copy_from_slice(&identity);
    image[212..228].copy_from_slice(&fresh_guid()?);
    image[228..236].copy_from_slice(&generation.to_le_bytes());
    image[236..252].copy_from_slice(&fresh_guid()?);
    Ok(identity)
}

/// Places the file `image` holds as `place` does a file on disk; without `at`, outside any
/// notebook, its `guidAncestor` and `crcName` zero.
pub fn place_image(image: &mut [u8], at: Option<([u8; 16], &str)>) -> Result<()> {
    Header::parse(image)?;
    image[128..148]
        .copy_from_slice(&at.map_or([0; 20], |(ancestor, name)| placement(ancestor, name)));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_takes_a_new_identity_and_leaves_its_notebook() {
        let mut image = crate::create_empty_section("A.one", None).unwrap();
        let before = Header::parse(&image).unwrap();
        place_image(&mut image, Some(([7; 16], "A.one"))).unwrap();
        assert_eq!(Header::parse(&image).unwrap().ancestor, [7; 16]);
        let identity = reidentify(&mut image).unwrap();
        place_image(&mut image, None).unwrap();
        let after = Header::parse(&image).unwrap();
        assert_eq!(after.file_id, identity);
        assert_ne!(after.file_id, before.file_id);
        assert_ne!(after.version_id, before.version_id);
        assert_eq!(after.generation, before.generation + 1);
        assert_eq!((after.ancestor, after.name_crc), ([0; 16], 0));
        crate::Store::parse(&image).unwrap();
    }
}
