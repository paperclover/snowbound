//! Signs releases with the ed25519 key whose public half the app embeds
//! (`release-key.pub`): `release_sign new KEY` makes a key, readable only by its owner;
//! `release_sign KEY FILE...` prints each file's signature, in hex, one per line.

use ring::signature::{Ed25519KeyPair, KeyPair};
use std::io::Write;

const PUBLIC: &str = include_str!("../release-key.pub");

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [new, key] if new == "new" => {
            let document = Ed25519KeyPair::generate_pkcs8(&ring::rand::SystemRandom::new())
                .map_err(|_| "Cannot generate a key")?;
            if let Some(folder) = std::path::Path::new(key).parent() {
                std::fs::create_dir_all(folder)?;
            }
            let mut file = std::fs::OpenOptions::new();
            file.write(true).create_new(true);
            #[cfg(unix)]
            std::os::unix::fs::OpenOptionsExt::mode(&mut file, 0o600);
            file.open(key)?.write_all(document.as_ref())?;
            let pair = Ed25519KeyPair::from_pkcs8(document.as_ref()).map_err(|_| "Bad key")?;
            println!("{}", hex(pair.public_key().as_ref()));
        }
        [key, files @ ..] if !files.is_empty() => {
            let pair = Ed25519KeyPair::from_pkcs8(&std::fs::read(key)?)
                .map_err(|_| format!("{key} is not an ed25519 key"))?;
            if hex(pair.public_key().as_ref()) != PUBLIC.trim() {
                return Err(format!("{key} is not the key release-key.pub names").into());
            }
            for file in files {
                println!("{}", hex(pair.sign(&std::fs::read(file)?).as_ref()));
            }
        }
        _ => return Err("Usage: release_sign new KEY | release_sign KEY FILE...".into()),
    }
    Ok(())
}
