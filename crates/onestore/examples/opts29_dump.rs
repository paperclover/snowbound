use onestore::{ObjectData, PropertySets, RevisionIndex, Store, Value};
use std::{env, fs};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    for path in env::args_os().skip(1) {
        let bytes = fs::read(&path)?;
        let store = Store::parse(&bytes)?;
        let index = RevisionIndex::parse(&store)?;
        println!("== {path:?}");
        for space in index.spaces.keys() {
            let Ok(resolved) = index.resolve_active(*space) else { continue };
            println!("-- space {space:?}");
            for (id, object) in &resolved.objects {
                let ObjectData::Properties(data) = object.data else { continue };
                let sets = PropertySets::parse(data)?;
                println!("{id:?} jcid={:#x}", object.jcid);
                for p in &sets.sets[0] {
                    let v = match &p.value {
                        Value::Bytes(b) if b.len() <= 64 => format!("{:02x?}", b),
                        Value::Bytes(b) => format!("{} bytes", b.len()),
                        other => format!("{other:?}").chars().take(120).collect(),
                    };
                    println!("   {:#010x} {v}", p.id);
                }
            }
        }
    }
    Ok(())
}
