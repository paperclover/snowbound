use super::{Error, Result, invalid};
use aes::cipher::{BlockModeDecrypt, KeyIvInit, block_padding::NoPadding};
use base64::{Engine, engine::general_purpose::STANDARD};
use sha1::{Digest, Sha1};
use subtle::ConstantTimeEq;
use zeroize::{Zeroize, Zeroizing};

const NS: &str = "http://schemas.microsoft.com/office/2006/encryption";
const PASSWORD_NS: &str = "http://schemas.microsoft.com/office/2006/keyEncryptor/password";

pub(super) struct Key {
    value: Zeroizing<[u8; 16]>,
    file_iv: [u8; 16],
}

fn child<'a, 'input>(
    node: roxmltree::Node<'a, 'input>,
    name: &str,
    ns: &str,
) -> Result<roxmltree::Node<'a, 'input>> {
    let mut matches = node.children().filter(|n| n.has_tag_name((ns, name)));
    let value = matches
        .next()
        .ok_or_else(|| invalid("Missing encryption metadata element"))?;
    if matches.next().is_some() {
        return Err(invalid("Repeated encryption metadata element"));
    }
    Ok(value)
}

fn decoded<const N: usize>(node: roxmltree::Node<'_, '_>, name: &str) -> Result<[u8; N]> {
    let value = node
        .attribute(name)
        .ok_or_else(|| invalid("Missing encryption metadata attribute"))?;
    let bytes = STANDARD
        .decode(value.split_ascii_whitespace().collect::<String>())
        .map_err(|_| invalid("Invalid encryption metadata base64"))?;
    bytes
        .try_into()
        .map_err(|_| invalid("Invalid encryption metadata byte length"))
}

fn profile(node: roxmltree::Node<'_, '_>) -> Result<()> {
    for (attribute, value) in [
        ("saltSize", 16),
        ("blockSize", 16),
        ("keyBits", 128),
        ("hashSize", 20),
    ] {
        if number(node, attribute)? != value {
            return Err(Error::Unsupported);
        }
    }
    for (attribute, value) in [
        ("cipherAlgorithm", "AES"),
        ("cipherChaining", "ChainingModeCBC"),
        ("hashAlgorithm", "SHA1"),
    ] {
        let actual = node
            .attribute(attribute)
            .ok_or_else(|| invalid("Missing encryption algorithm attribute"))?;
        if actual != value {
            return Err(Error::Unsupported);
        }
    }
    Ok(())
}

fn number(node: roxmltree::Node<'_, '_>, name: &str) -> Result<u32> {
    node.attribute(name)
        .ok_or_else(|| invalid("Missing encryption numeric attribute"))?
        .trim()
        .parse()
        .map_err(|_| invalid("Invalid encryption numeric attribute"))
}

fn decrypt(key: &[u8; 16], iv: &[u8; 16], bytes: &mut [u8]) -> Result<()> {
    cbc::Decryptor::<aes::Aes128>::new(key.into(), iv.into())
        .decrypt_padded::<NoPadding>(bytes)
        .map_err(|_| invalid("Encrypted payload is not block aligned"))?;
    Ok(())
}

impl Key {
    pub(super) fn open(data: &[u8], password: &str, rounds: &mut u64) -> Result<Self> {
        if data.len() > 65536 || password.len() > 65536 {
            return Err(Error::Limit);
        }
        let mut c = crate::bytes::Cursor {
            bytes: data,
            offset: 0,
        };
        if u32::from_le_bytes(c.read()?) != 3 {
            return Err(Error::Unsupported);
        }
        let length = u32::from_le_bytes(c.read()?) as usize;
        let offset = u32::from_le_bytes(c.read()?) as usize;
        let inner = u32::from_le_bytes(c.read()?) as usize;
        if length != data.len() || offset != 16 || inner != data.len() - 16 {
            return Err(invalid("Inconsistent encryption metadata framing"));
        }
        if c.read::<8>()? != [4, 0, 4, 0, 64, 0, 0, 0] {
            return Err(Error::Unsupported);
        }
        let text = std::str::from_utf8(c.bytes)
            .map_err(|_| invalid("Encryption metadata is not UTF-8"))?;
        let xml = roxmltree::Document::parse_with_options(
            text,
            roxmltree::ParsingOptions {
                nodes_limit: 64,
                ..Default::default()
            },
        )
        .map_err(|_| invalid("Invalid encryption metadata XML"))?;
        let root = xml.root_element();
        if !root.has_tag_name((NS, "encryption")) {
            return Err(Error::Unsupported);
        }
        let data_key = child(root, "keyData", NS)?;
        let encryptors = child(root, "keyEncryptors", NS)?;
        if root.children().filter(|n| n.is_element()).count() != 2
            || encryptors.children().filter(|n| n.is_element()).count() != 1
        {
            return Err(Error::Unsupported);
        }
        let encryptor = child(encryptors, "keyEncryptor", NS)?;
        if encryptor.attribute("uri") != Some(PASSWORD_NS)
            || encryptor.children().filter(|n| n.is_element()).count() != 1
        {
            return Err(Error::Unsupported);
        }
        let wrapped = child(encryptor, "encryptedKey", PASSWORD_NS)?;
        profile(data_key)?;
        profile(wrapped)?;
        let count = number(wrapped, "spinCount")?;
        *rounds = rounds.checked_sub(u64::from(count)).ok_or(Error::Limit)?;
        let salt = decoded::<16>(wrapped, "saltValue")?;
        let mut verifier = Zeroizing::new(decoded::<16>(wrapped, "encryptedVerifierHashInput")?);
        let mut expected = Zeroizing::new(decoded::<32>(wrapped, "encryptedVerifierHashValue")?);
        let mut value = Zeroizing::new(decoded::<16>(wrapped, "encryptedKeyValue")?);
        let mut hash = Sha1::new();
        hash.update(salt);
        for word in password.encode_utf16() {
            hash.update(word.to_le_bytes());
        }
        let mut seed = Zeroizing::new(<[u8; 20]>::from(hash.finalize()));
        for index in 0..count {
            let mut hash = Sha1::new();
            hash.update(index.to_le_bytes());
            hash.update(seed.as_ref());
            *seed = hash.finalize().into();
        }
        for (label, bytes) in [
            (
                [0xfe, 0xa7, 0xd2, 0x76, 0x3b, 0x4b, 0x9e, 0x79],
                verifier.as_mut_slice(),
            ),
            (
                [0xd7, 0xaa, 0x0f, 0x6d, 0x30, 0x61, 0x34, 0x4e],
                expected.as_mut_slice(),
            ),
            (
                [0x14, 0x6e, 0x0b, 0xe7, 0xab, 0xac, 0xd0, 0xd6],
                value.as_mut_slice(),
            ),
        ] {
            let mut hash = Sha1::new();
            hash.update(seed.as_ref());
            hash.update(label);
            let derived = Zeroizing::new(<[u8; 20]>::from(hash.finalize()));
            decrypt(derived[..16].try_into().unwrap(), &salt, bytes)?;
        }
        let actual = Zeroizing::new(<[u8; 20]>::from(Sha1::digest(verifier.as_slice())));
        if !bool::from(actual.as_slice().ct_eq(&expected[..20])) {
            return Err(Error::PasswordMismatch);
        }
        let mut hash = Sha1::new();
        hash.update(decoded::<16>(data_key, "saltValue")?);
        hash.update(0_u32.to_le_bytes());
        let file_iv = hash.finalize()[..16].try_into().unwrap();
        Ok(Self { value, file_iv })
    }

    pub(super) fn property(&self, input: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
        let mut c = crate::bytes::Cursor {
            bytes: input,
            offset: 0,
        };
        crate::properties::reference_streams(&mut c)?;
        let prefix = c.offset;
        let length = u32::from_le_bytes(c.read()?) as usize;
        let encrypted = c.take(length)?;
        if c.bytes.len() > 7 || c.bytes.iter().any(|b| *b != 0) {
            return Err(invalid("Invalid encrypted property alignment"));
        }
        let (iv, body) = encrypted
            .split_first_chunk::<16>()
            .ok_or_else(|| invalid("Missing encrypted property IV"))?;
        let mut clear = Zeroizing::new(body.to_vec());
        decrypt(&self.value, iv, &mut clear)?;
        let padding = clear
            .first_chunk::<2>()
            .map(|b| usize::from(u16::from_le_bytes(*b)))
            .ok_or_else(|| invalid("Missing encrypted property padding count"))?;
        if padding >= 16 || clear.len() < 2 + padding {
            return Err(invalid("Invalid encrypted property padding count"));
        }
        let mut output = Zeroizing::new(Vec::with_capacity(prefix + clear.len() - 2 - padding));
        output.extend_from_slice(&input[..prefix]);
        output.extend_from_slice(&clear[2..clear.len() - padding]);
        crate::PropertySets::parse(&output)?;
        Ok(output)
    }

    pub(super) fn file(&self, input: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
        if input.is_empty() {
            return Ok(Zeroizing::new(Vec::new()));
        }
        let mut clear = Zeroizing::new(input.to_vec());
        decrypt(&self.value, &self.file_iv, &mut clear)?;
        let length = clear
            .first_chunk::<8>()
            .map(|b| u64::from_le_bytes(*b))
            .ok_or_else(|| invalid("Missing encrypted file length"))?;
        let length = usize::try_from(length)
            .map_err(|_| invalid("Encrypted file length exceeds address space"))?;
        if length > clear.len() - 8 || clear.len() - 8 - length >= 16 {
            return Err(invalid("Invalid encrypted file length"));
        }
        clear.copy_within(8..8 + length, 0);
        clear[length..].zeroize();
        clear.truncate(length);
        Ok(clear)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ObjectData, Reference, RevisionIndex, Store};

    #[test]
    fn native_frames_and_bounded_malformed_inputs() {
        let root = std::path::Path::new("../../corpus/native-encrypted");
        let bytes = std::fs::read(root.join("encrypted-01/notebook/synthetic.one")).unwrap();
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join("manifest.json")).unwrap()).unwrap();
        let password = manifest["password"].as_str().unwrap();
        let store = Store::parse(&bytes).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let node = index
            .spaces
            .values()
            .next()
            .unwrap()
            .revisions
            .values()
            .next()
            .unwrap()
            .nodes
            .first()
            .unwrap();
        let Some(Reference::Data(chunk)) = node.reference else {
            panic!("Missing native key")
        };
        let metadata = store.encryption_key(chunk).unwrap();
        for length in 0..metadata.len() {
            assert!(Key::open(&metadata[..length], password, &mut 0).is_err());
        }
        let key = Key::open(metadata, password, &mut 100000).unwrap();
        let xml = std::str::from_utf8(&metadata[24..]).unwrap();
        let frame = |xml: &str| {
            let mut value = metadata[..24].to_vec();
            value.extend_from_slice(xml.as_bytes());
            let length = u32::try_from(value.len()).unwrap();
            value[4..8].copy_from_slice(&length.to_le_bytes());
            value[12..16].copy_from_slice(&(length - 16).to_le_bytes());
            value
        };
        assert!(matches!(
            Key::open(
                &frame(&xml.replace("keyBits=\"128\"", "keyBits=\"256\"")),
                password,
                &mut 100000
            ),
            Err(Error::Unsupported)
        ));
        assert!(matches!(
            Key::open(
                &frame(&xml.replace("keyBits=\"128\"", "")),
                password,
                &mut 100000
            ),
            Err(Error::Invalid(_))
        ));
        let tree = roxmltree::Document::parse(xml).unwrap();
        let salt = child(tree.root_element(), "keyData", NS)
            .unwrap()
            .attribute("saltValue")
            .unwrap();
        let spaced = xml
            .replace("keyBits=\"128\"", "keyBits=\" +0128 \"")
            .replace("spinCount=\"100000\"", "spinCount=\" 0100000 \"")
            .replace(salt, &format!(" {}\n{} ", &salt[..8], &salt[8..]));
        let spaced = Key::open(&frame(&spaced), password, &mut 100000).unwrap();
        for (sid, space) in &index.spaces {
            for rid in space.labels.values() {
                let revision = index.resolve(*sid, *rid).unwrap();
                for object in revision.objects.values() {
                    if let ObjectData::Encrypted(bytes) = object.data {
                        let mut cursor = crate::bytes::Cursor { bytes, offset: 0 };
                        crate::properties::reference_streams(&mut cursor).unwrap();
                        let length = u32::from_le_bytes(cursor.read().unwrap()) as usize;
                        let end = cursor.offset + length;
                        assert!(key.property(bytes).is_ok());
                        assert_eq!(
                            *key.property(bytes).unwrap(),
                            *spaced.property(bytes).unwrap()
                        );
                        for cut in 0..end {
                            assert!(key.property(&bytes[..cut]).is_err());
                        }
                        let mut changed = bytes.to_vec();
                        changed.push(1);
                        assert!(key.property(&changed).is_err());
                    }
                }
            }
        }
        let mut state = 0x4851_e529_b30d_620f_u64;
        for length in 0..1024 {
            let mut bytes = vec![0; length];
            for byte in &mut bytes {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                *byte = state.to_le_bytes()[0];
            }
            let _ = key.property(&bytes);
            let _ = key.file(&bytes);
            assert!(Key::open(&bytes, password, &mut 0).is_err());
        }
        for index in 0..metadata.len() {
            let mut changed = metadata.to_vec();
            changed[index] ^= 0x80;
            assert!(Key::open(&changed, password, &mut 0).is_err());
        }
    }
}
