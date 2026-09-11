//! Builds a new external-payload test fixture from a native capture with reserved fragment space.
use onestore::{FileDataReference, RevisionIndex, Store};
use std::{fs, path::PathBuf};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 2 {
        return Err("Provide a native source section and a new fixture directory".into());
    }
    let source = fs::read(&args[0])?;
    let destination = PathBuf::from(&args[1]);
    fs::create_dir(&destination)?;
    fs::create_dir(destination.join("synthetic_onefiles"))?;
    let store = Store::parse(&source)?;
    assert!(store.checksum_mismatches.is_empty());
    RevisionIndex::parse(&store)?.validate_current()?;
    let mut output = source.clone();
    let mut changed = 0;
    for list in store.lists.values() {
        for chunk in &list.fragments {
            let start = chunk.offset as usize;
            let end = start + chunk.length as usize - 20;
            let mut body = Vec::new();
            let mut touched = false;
            for node in list
                .nodes
                .iter()
                .filter(|node| node.offset >= start + 16 && node.offset < end)
            {
                let raw = u32::from_le_bytes(source[node.offset..node.offset + 4].try_into()?);
                let size = ((raw >> 10) & 0x1fff) as usize;
                let mut encoded = source[node.offset..node.offset + size].to_vec();
                if matches!(node.id, 0x72 | 0x73) {
                    let offset = 4 + 4 + 4 + if node.id == 0x72 { 1 } else { 4 };
                    let count =
                        u32::from_le_bytes(encoded[offset..offset + 4].try_into()?) as usize;
                    let text = String::from_utf16(
                        &encoded[offset + 4..offset + 4 + count * 2]
                            .chunks_exact(2)
                            .map(|v| u16::from_le_bytes(v.try_into().unwrap()))
                            .collect::<Vec<_>>(),
                    )?;
                    if let FileDataReference::Internal(guid) = text.parse()? {
                        let filename = format!(
                            "{}.onebin",
                            text.strip_prefix("<ifndf>{")
                                .unwrap()
                                .strip_suffix('}')
                                .unwrap()
                        );
                        let payload = store.file_data(guid)?;
                        let path = destination.join("synthetic_onefiles").join(&filename);
                        if path.exists() {
                            assert_eq!(fs::read(&path)?, payload);
                        } else {
                            fs::write(path, payload)?;
                        }
                        let reference = format!("<file>{filename}");
                        let data: Vec<_> = reference
                            .encode_utf16()
                            .flat_map(u16::to_le_bytes)
                            .collect();
                        encoded.splice(offset + 4..offset + 4 + count * 2, data.iter().copied());
                        encoded[offset..offset + 4]
                            .copy_from_slice(&(data.len() as u32 / 2).to_le_bytes());
                        assert!(encoded.len() <= 0x1fff);
                        let header = (raw & !(0x1fff << 10)) | ((encoded.len() as u32) << 10);
                        encoded[..4].copy_from_slice(&header.to_le_bytes());
                        changed += 1;
                        touched = true;
                    }
                }
                body.extend(encoded);
            }
            if touched {
                assert!(
                    body.len() + 4 <= end - start - 16,
                    "Native fragment has insufficient reserved space"
                );
                body.extend_from_slice(&(0xff_u32 | (4 << 10)).to_le_bytes());
                output[start + 16..end].fill(0);
                output[start + 16..start + 16 + body.len()].copy_from_slice(&body);
            }
        }
    }
    assert!(changed > 0);
    let converted = Store::parse(&output)?;
    assert!(converted.checksum_mismatches.is_empty());
    RevisionIndex::parse(&converted)?.validate_current()?;
    fs::write(destination.join("synthetic.one"), &output)?;
    fs::write(
        destination.join("Open Notebook.onetoc2"),
        onestore::create_table_of_contents(
            "Open Notebook.onetoc2",
            &[("synthetic.one", converted.header.file_id)],
        )?,
    )?;
    println!(
        "Converted {changed} file-data declarations without changing payload identities or bytes"
    );
    Ok(())
}
