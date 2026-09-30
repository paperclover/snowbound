use onestore::{ObjectData, PropertySets, RevisionIndex, Store, Value};
use std::{env, fs};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let want: Vec<u32> = env::var("IDS")
        .unwrap_or_default()
        .split(',')
        .filter(|s| !s.is_empty())
        .map(|s| u32::from_str_radix(s.trim_start_matches("0x"), 16).unwrap())
        .collect();
    for path in env::args_os().skip(1) {
        let bytes = fs::read(&path)?;
        let store = Store::parse(&bytes)?;
        let index = RevisionIndex::parse(&store)?;
        println!("== {path:?}");
        for space in index.spaces.keys() {
            let Ok(resolved) = index.resolve_active(*space) else { continue };
            for (id, object) in &resolved.objects {
                let ObjectData::Properties(data) = object.data else { continue };
                let sets = PropertySets::parse(data)?;
                let root = &sets.sets[0];
                if !want.is_empty() && !root.iter().any(|p| want.contains(&p.id)) {
                    continue;
                }
                if env::var("LINE").is_ok() {
                    let get = |want: u32| root.iter().find(|p| p.id == want).and_then(|p| match p.value { Value::Bytes(b) => Some(b), _ => None });
                    let text = get(0x1c003498).map(|b| String::from_utf8_lossy(b).into_owned()).or_else(|| get(0x1c001c22).map(|b| String::from_utf16_lossy(&b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect::<Vec<_>>()))).unwrap_or_default();
                    let ids: Vec<String> = root.iter().map(|p| format!("{:x}", p.id)).collect();
                    println!("{:#x} cc8={:02x?} media={} text={text:?} ids={}", object.jcid, get(0x1c001cc8), get(0x1c001c98).is_some(), ids.join(","));
                    continue;
                }
                println!("{space:?} {id:?} jcid {:#x}", object.jcid);
                for p in root {
                    match p.value {
                        Value::Bytes(b) => {
                            let text = if p.id == 0x1c001c22 {
                                String::from_utf16_lossy(&b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect::<Vec<_>>())
                            } else if p.id == 0x1c003498 {
                                String::from_utf8_lossy(b).into_owned()
                            } else {
                                String::new()
                            };
                            println!("  {:#010x} {:02x?} {text}", p.id, &b[..b.len().min(48)])
                        }
                        Value::NoData => println!("  {:#010x} -", p.id),
                        ref v => println!("  {:#010x} {v:?}", p.id),
                    }
                }
            }
        }
    }
    Ok(())
}
