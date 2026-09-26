//! Changes one stored property value: a text object's text (`6000e 1c001c22`, UTF-16LE,
//! a trailing NUL dropped) as a text op, the notebook's colour on the table of contents root
//! (`20001 14001cbe`) as a TOC edit, each appending a revision; any other value of the same
//! length only in a copy, patched where it is stored, as a fixture that is no edit.

#[path = "../src/flush.rs"]
mod flush;
#[path = "support/typing.rs"]
mod typing;

use onestore::{
    ExGuid, ObjectData, PropertySets, RevisionIndex, Store, TocEdit, Transaction, Value,
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
    let transaction: Option<Transaction> = match (jcid, id) {
        (0x6000e, 0x1c001c22) => {
            let units = |bytes: &[u8]| -> Vec<u16> {
                let mut units: Vec<u16> = bytes
                    .chunks_exact(2)
                    .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                    .collect();
                if units.last() == Some(&0) {
                    units.pop();
                }
                units
            };
            let length = u32::try_from(units(&expected).len())?;
            let text = String::from_utf16(&units(&value))?;
            let edit = typing::text(space, object, 0..length, &text);
            typing::sealed(&bytes, "Property editor", &edit)?
        }
        (0x20001, 0x14001cbe) => {
            if index.resolve_active(space)?.roots.get(&1) != Some(&object) {
                return Err("Only the notebook's colour, on the TOC root, changes".into());
            }
            let color = u32::from_le_bytes(value.as_slice().try_into()?);
            onestore::edit_table_of_contents(&bytes, &[TocEdit::Color(color)])?
        }
        _ if args[1] != "--in-place" && value.len() == expected.len() => {
            let revision = index.resolve_active(space)?;
            let ObjectData::Properties(data) = revision.objects[&object].data else {
                return Err("The object stores no properties".into());
            };
            let properties = PropertySets::parse(data)?;
            let Some(Value::Bytes(stored)) = properties.sets[0]
                .iter()
                .find(|property| property.id == id)
                .map(|property| &property.value)
            else {
                return Err("The property stores no bytes".into());
            };
            let at = stored.as_ptr().addr() - bytes.as_ptr().addr();
            let mut patched = bytes.clone();
            patched[at..at + value.len()].copy_from_slice(&value);
            write(&args[1], &patched)?;
            return Ok(());
        }
        _ => return Err("Only text and TOC colours change as edits; other values are patched into a copy of equal length".into()),
    };
    let transaction = transaction.ok_or("The value is already stored")?;
    if args[1] == "--in-place" {
        transaction.commit_file(&args[0])?;
        return Ok(());
    }
    let mut written = bytes.clone();
    transaction.apply(&mut written)?;
    write(&args[1], &written)
}

fn write(path: &str, written: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    output.write_all(written)?;
    flush::flush(&output)?;
    Ok(())
}
