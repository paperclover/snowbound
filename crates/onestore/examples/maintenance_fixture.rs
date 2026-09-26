#[path = "support/typing.rs"]
mod typing;

use onestore::{
    ExGuid, RevisionIndex, Store,
    document::{Document, Kind},
};
use std::{fs, io, path::PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let output = PathBuf::from(
        args.next()
            .ok_or(io::Error::from(io::ErrorKind::InvalidInput))?,
    );
    let current = args
        .next()
        .map(|value| value.into_string())
        .transpose()
        .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    if args.next().is_some() {
        return Err(io::Error::from(io::ErrorKind::InvalidInput).into());
    }
    fs::create_dir(&output)?;
    let mut bytes = onestore::create_section("synthetic.one", "Maintenance baseline.", "Fixture")?;
    for revision in 0..25 {
        let store = Store::parse(&bytes)?;
        let index = RevisionIndex::parse(&store)?;
        let document = Document::parse(&index)?;
        let (sid, oid, end) = document
            .spaces
            .iter()
            .find_map(|(sid, space)| {
                space.revisions[&space.contexts[&ExGuid::default()]]
                    .nodes
                    .iter()
                    .find_map(|(oid, node)| {
                        if let Kind::RichText { text, .. } = &node.kind {
                            return Some((*sid, *oid, text.encode_utf16().count() as u32));
                        }
                        None
                    })
            })
            .ok_or(io::Error::from(io::ErrorKind::InvalidData))?;
        let text = if revision == 24 {
            current
                .clone()
                .unwrap_or_else(|| "Maintenance current.".to_owned())
        } else {
            format!("Revision {revision}: {}", "x".repeat(65536))
        };
        let edit = typing::text(sid, oid, 0..end, &text);
        if let Some(transaction) = typing::sealed(&bytes, "Fixture", &edit)? {
            transaction.apply(&mut bytes)?;
        }
    }
    let store = Store::parse(&bytes)?;
    let toc = onestore::create_table_of_contents(
        "Open Notebook.onetoc2",
        &[("synthetic.one", store.header.file_id)],
    )?;
    fs::write(output.join("synthetic.one"), &bytes)?;
    fs::write(output.join("Open Notebook.onetoc2"), toc)?;
    println!(
        "{}",
        serde_json::json!({"bytes":bytes.len(),"transactions":store.header.transaction_count})
    );
    Ok(())
}
