use onestore::*;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let source =
        std::fs::read("corpus/native/20260905-05/snapshots/02-text/notebook/synthetic.one")?;
    let store = Store::parse(&source)?;
    let index = RevisionIndex::parse(&store)?;
    let mut target = None;
    for (osid, space) in &index.spaces {
        let revision = index.resolve(*osid, space.labels[&(ExGuid::default(), 1)])?;
        for (oid, object) in revision.objects {
            if let ObjectData::Properties(data) = object.data {
                for p in &PropertySets::parse(data)?.sets[0] {
                    if p.value == Value::Bytes(b"Fictitious plain text.") {
                        target = Some((*osid, oid, p.id));
                    }
                }
            }
        }
    }
    let (osid, oid, property) = target.unwrap();
    let output = replace_property_bytes(&source, osid, oid, property, b"A durable edit.")?;
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
