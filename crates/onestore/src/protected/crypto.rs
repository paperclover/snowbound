use super::{Error, Result, invalid};
use aes::cipher::{BlockModeDecrypt, BlockModeEncrypt, KeyIvInit, block_padding::NoPadding};
use base64::{Engine, engine::general_purpose::STANDARD};
use sha1::{Digest, Sha1};
use subtle::ConstantTimeEq;
use zeroize::{Zeroize, Zeroizing};

const NS: &str = "http://schemas.microsoft.com/office/2006/encryption";
const PASSWORD_NS: &str = "http://schemas.microsoft.com/office/2006/keyEncryptor/password";

#[derive(Clone)]
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

fn encrypt(key: &[u8; 16], iv: &[u8; 16], bytes: &mut [u8]) {
    let length = bytes.len();
    cbc::Encryptor::<aes::Aes128>::new(key.into(), iv.into())
        .encrypt_padded::<NoPadding>(bytes, length)
        .expect("block aligned");
}

/// Password-hash rounds OneNote 2010 writes.
const SPINS: u32 = 100_000;
/// MS-OFFCRYPTO's block keys for the verifier input, its hash and the wrapped key.
const VERIFIER_INPUT: [u8; 8] = [0xfe, 0xa7, 0xd2, 0x76, 0x3b, 0x4b, 0x9e, 0x79];
const VERIFIER_VALUE: [u8; 8] = [0xd7, 0xaa, 0x0f, 0x6d, 0x30, 0x61, 0x34, 0x4e];
const KEY_VALUE: [u8; 8] = [0x14, 0x6e, 0x0b, 0xe7, 0xab, 0xac, 0xd0, 0xd6];

/// The iterated password hash: SHA-1 of salt and UTF-16LE password, then `count` rounds of
/// SHA-1 over the round number and the previous hash.
fn seed(salt: &[u8; 16], password: &str, count: u32) -> Zeroizing<[u8; 20]> {
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
    seed
}

/// The AES key a block key derives from the password hash.
fn block(seed: &[u8; 20], label: [u8; 8]) -> Zeroizing<[u8; 16]> {
    let mut hash = Sha1::new();
    hash.update(seed);
    hash.update(label);
    let derived = Zeroizing::new(<[u8; 20]>::from(hash.finalize()));
    Zeroizing::new(derived[..16].try_into().unwrap())
}

/// The IV of payloads: SHA-1 of the key data's salt and block 0.
fn file_iv(salt: &[u8; 16]) -> [u8; 16] {
    let mut hash = Sha1::new();
    hash.update(salt);
    hash.update(0_u32.to_le_bytes());
    hash.finalize()[..16].try_into().unwrap()
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
        let seed = seed(&salt, password, count);
        for (label, bytes) in [
            (VERIFIER_INPUT, verifier.as_mut_slice()),
            (VERIFIER_VALUE, expected.as_mut_slice()),
            (KEY_VALUE, value.as_mut_slice()),
        ] {
            decrypt(&block(&seed, label), &salt, bytes)?;
        }
        let actual = Zeroizing::new(<[u8; 20]>::from(Sha1::digest(verifier.as_slice())));
        if !bool::from(actual.as_slice().ct_eq(&expected[..20])) {
            return Err(Error::PasswordMismatch);
        }
        Ok(Self {
            value,
            file_iv: file_iv(&decoded::<16>(data_key, "saltValue")?),
        })
    }

    /// A fresh key for `password` and the encryption data that opens it, as OneNote 2010
    /// writes them: random salts, key and verifier, 100,000 SHA-1 rounds, no integrity block.
    pub(super) fn create(password: &str) -> Result<(Self, Vec<u8>)> {
        if password.len() > 65536 {
            return Err(Error::Limit);
        }
        let random = |bytes: &mut [u8]| {
            getrandom::fill(bytes).map_err(|_| invalid("System random source failed"))
        };
        let (mut data_salt, mut salt) = ([0; 16], [0; 16]);
        let mut value = Zeroizing::new([0; 16]);
        let mut verifier = Zeroizing::new([0; 16]);
        random(&mut data_salt)?;
        random(&mut salt)?;
        random(value.as_mut_slice())?;
        random(verifier.as_mut_slice())?;
        let seed = seed(&salt, password, SPINS);
        let mut hash = Zeroizing::new([0; 32]);
        hash[..20].copy_from_slice(&Sha1::digest(verifier.as_slice()));
        let mut wrapped_value = *value;
        let mut wrapped_verifier = *verifier;
        let mut wrapped_hash = *hash;
        encrypt(&block(&seed, VERIFIER_INPUT), &salt, &mut wrapped_verifier);
        encrypt(&block(&seed, VERIFIER_VALUE), &salt, &mut wrapped_hash);
        encrypt(&block(&seed, KEY_VALUE), &salt, &mut wrapped_value);
        let profile = r#"saltSize="16" blockSize="16" keyBits="128" hashSize="20" cipherAlgorithm="AES" cipherChaining="ChainingModeCBC" hashAlgorithm="SHA1""#;
        let xml = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n\
             <encryption xmlns=\"{NS}\" xmlns:p=\"{PASSWORD_NS}\">\
             <keyData {profile} saltValue=\"{}\"/>\
             <keyEncryptors><keyEncryptor uri=\"{PASSWORD_NS}\">\
             <p:encryptedKey spinCount=\"{SPINS}\" {profile} saltValue=\"{}\" \
             encryptedVerifierHashInput=\"{}\" encryptedVerifierHashValue=\"{}\" \
             encryptedKeyValue=\"{}\"/></keyEncryptor></keyEncryptors></encryption>",
            STANDARD.encode(data_salt),
            STANDARD.encode(salt),
            STANDARD.encode(wrapped_verifier),
            STANDARD.encode(wrapped_hash),
            STANDARD.encode(wrapped_value),
        );
        let length = u32::try_from(24 + xml.len()).map_err(|_| Error::Limit)?;
        let mut data = Vec::with_capacity(length as usize);
        for word in [3, length, 16, length - 16] {
            data.extend_from_slice(&word.to_le_bytes());
        }
        data.extend_from_slice(&[4, 0, 4, 0, 64, 0, 0, 0]);
        data.extend_from_slice(xml.as_bytes());
        Ok((
            Self {
                value,
                file_iv: file_iv(&data_salt),
            },
            data,
        ))
    }

    /// The AES key the section's objects and payloads are encrypted under.
    pub(super) fn value(&self) -> &[u8; 16] {
        &self.value
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

    /// The stored form of a plaintext property object, the inverse of `property`.
    pub(super) fn seal_property(&self, clear: &[u8], iv: [u8; 16]) -> Result<Vec<u8>> {
        let mut c = crate::bytes::Cursor {
            bytes: clear,
            offset: 0,
        };
        crate::properties::reference_streams(&mut c)?;
        let prefix = c.offset;
        let padding = (16 - (2 + clear.len() - prefix) % 16) % 16;
        // Sized once: a buffer abandoned by growth would keep plaintext.
        let mut body = Zeroizing::new(Vec::with_capacity(2 + clear.len() - prefix + padding));
        body.extend_from_slice(&(padding as u16).to_le_bytes());
        body.extend_from_slice(&clear[prefix..]);
        let length = body.len() + padding;
        body.resize(length, 0);
        getrandom::fill(&mut body[length - padding..])
            .map_err(|_| invalid("System random source failed"))?;
        encrypt(&self.value, &iv, &mut body);
        let mut output = clear[..prefix].to_vec();
        output.extend_from_slice(&((16 + body.len()) as u32).to_le_bytes());
        output.extend_from_slice(&iv);
        output.extend_from_slice(&body);
        output.resize(output.len().next_multiple_of(8), 0);
        Ok(output)
    }

    /// The stored form of a file payload, the inverse of `file`: its length, the bytes and
    /// random padding to the block, as OneNote pads.
    pub(super) fn seal_file(&self, clear: &[u8]) -> Result<Vec<u8>> {
        if clear.is_empty() {
            return Ok(Vec::new());
        }
        let length = (8 + clear.len()).next_multiple_of(16);
        let mut output = Vec::with_capacity(length);
        output.extend_from_slice(&(clear.len() as u64).to_le_bytes());
        output.extend_from_slice(clear);
        let end = output.len();
        output.resize(length, 0);
        getrandom::fill(&mut output[end..]).map_err(|_| invalid("System random source failed"))?;
        encrypt(&self.value, &self.file_iv, &mut output);
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
                        let iv = bytes[cursor.offset..cursor.offset + 16].try_into().unwrap();
                        // Native padding is arbitrary; it only reaches the last block.
                        let sealed = key
                            .seal_property(&key.property(bytes).unwrap(), iv)
                            .unwrap();
                        assert_eq!(sealed.len(), bytes.len());
                        assert_eq!(sealed[..end - 16], bytes[..end - 16]);
                        assert_eq!(
                            *key.property(&sealed).unwrap(),
                            *key.property(bytes).unwrap()
                        );
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
