#[path = "support/typing.rs"]
mod typing;

use onestore::*;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let source =
        std::fs::read("corpus/native/20260905-05/snapshots/02-text/notebook/synthetic.one")?;
    let store = Store::parse(&source)?;
    let index = RevisionIndex::parse(&store)?;
    let document = document::Document::parse(&index)?;
    let (osid, oid) = document
        .spaces
        .iter()
        .find_map(|(sid, space)| {
            space.active()?.nodes.iter().find_map(|(oid, node)| {
                matches!(&node.kind, document::Kind::RichText { text, .. } if text == "Fictitious plain text.")
                    .then_some((*sid, *oid))
            })
        })
        .ok_or("Missing native fixture text")?;
    let end = "Fictitious plain text.".encode_utf16().count() as u32;
    let edit = typing::text(osid, oid, 0..end, "A durable edit.");
    let transaction = typing::sealed(&source, "Header faults", &edit)?.ok_or("No change")?;
    let mut output = source.clone();
    transaction.apply(&mut output)?;
    let mut invalid = Vec::new();
    for prefix in 0..=1024 {
        let mut torn = output.clone();
        torn[prefix..1024].copy_from_slice(&source[prefix..1024]);
        let result = Store::parse(&torn).and_then(|store| {
            let index = RevisionIndex::parse(&store)?;
            index.validate_current()
        });
        if result.is_err() {
            invalid.push(prefix);
        }
    }
    println!(
        "Header prefix tears: {} invalid out of 1025; offsets {:?}",
        invalid.len(),
        invalid
    );
    Ok(())
}
