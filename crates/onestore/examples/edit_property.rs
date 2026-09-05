#[path = "../src/flush.rs"]
mod flush;

use onestore::{
    ExGuid, ObjectData, PropertySets, RevisionIndex, Store, Value, commit_file_property,
    replace_property_bytes,
};
use std::{env, fs, io::Write};

fn hex(text: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    if !text.is_ascii() || !text.len().is_multiple_of(2) {
        return Err("Expected an even number of hexadecimal digits".into());
    }
    text.as_bytes()
        .chunks_exact(2)
        .map(|pair| Ok(u8::from_str_radix(std::str::from_utf8(pair)?, 16)?))
        .collect()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = env::args().skip(1).collect();
    if args.len() != 6 && args.len() != 8 {
        return Err(
            "Usage: edit_property INPUT OUTPUT|--in-place JCID PROPERTY EXPECTED_HEX VALUE_HEX [SPACE OBJECT]"
                .into(),
        );
    }
    let jcid = u32::from_str_radix(&args[2], 16)?;
    let id = u32::from_str_radix(&args[3], 16)?;
    let expected = hex(&args[4])?;
    let value = hex(&args[5])?;
    let bytes = onestore::read_file(&args[0])?;
    let store = Store::parse(&bytes)?;
    let index = RevisionIndex::parse(&store)?;
    index.validate_current()?;
    let mut selected = None;
    for (osid, space) in &index.spaces {
        let Some(&rid) = space.labels.get(&(ExGuid::default(), 1)) else {
            continue;
        };
        let revision = index.resolve(*osid, rid)?;
        for oid in revision.reachable()? {
            if args.len() == 8 && (args[6] != osid.to_string() || args[7] != oid.to_string()) {
                continue;
            }
            let object = &revision.objects[&oid];
            if object.jcid == jcid
                && let ObjectData::Properties(data) = object.data
            {
                for property in &PropertySets::parse(data)?.sets[0] {
                    if property.id == id
                        && property.value == Value::Bytes(&expected)
                        && selected.replace((*osid, oid)).is_some()
                    {
                        return Err("Multiple matching objects".into());
                    }
                }
            }
        }
    }
    let (space, object) = selected.ok_or("No matching object")?;
    if args[1] == "--in-place" {
        commit_file_property(&args[0], &bytes, space, object, id, &value)?;
        return Ok(());
    }
    let written = replace_property_bytes(&bytes, space, object, id, &value)?;
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&args[1])?;
    output.write_all(&written)?;
    flush::flush(&output)?;
    Ok(())
}
