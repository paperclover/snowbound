use onestore::{ObjectData, PropertySets, RevisionIndex, Store, Value, document::Document};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let bytes = std::fs::read(&args[0])?;
    let store = Store::parse(&bytes)?;
    let index = RevisionIndex::parse(&store)?;
    let pages = Document::parse(&index).and_then(|document| document.pages()).unwrap_or_default();
    let space = if args[1] == "root" { index.root } else { pages[args[1].parse::<usize>()?].0 };
    println!("space {space}; pages {:?}", pages.iter().map(|p| p.0).collect::<Vec<_>>());
    let raw = index.resolve_active(space)?;
    for (id, object) in &raw.objects {
        println!("{id} jcid {:#x}", object.jcid);
        if let ObjectData::Properties(data) = object.data {
            let sets = PropertySets::parse(data)?;
            for (n, set) in sets.sets.iter().enumerate() {
                for p in set {
                    let v = match &p.value {
                        Value::NoData => "-".to_string(),
                        Value::Bytes(b) => b.iter().map(|x| format!("{x:02x}")).collect(),
                        Value::References { stream, compact_ids } => format!("{stream:?} {}", compact_ids.iter().map(|x| format!("{x:02x}")).collect::<String>()),
                        Value::Sets(r) => format!("sets {r:?}"),
                    };
                    println!("   [{n}] {:#010x} {v}", p.id);
                }
            }
        }
    }
    Ok(())
}
