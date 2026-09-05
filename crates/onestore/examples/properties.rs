use onestore::{PropertySets, Reference, Store};
use std::{env, fs};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    for path in env::args_os().skip(1) {
        let bytes = fs::read(&path)?;
        let store = Store::parse(&bytes)?;
        let mut count = 0;
        for list in store.lists.values() {
            for node in &list.nodes {
                if node.id == 0x7c {
                    return Err("Encrypted object space requires revision-aware inspection".into());
                }
                if matches!(
                    node.id,
                    0xa4 | 0xa5 | 0xc4 | 0xc5 | 0x2d | 0x2e | 0x41 | 0x42
                ) && let Some(Reference::Data(chunk)) = node.reference
                {
                    let start = usize::try_from(chunk.offset)?;
                    let end = start
                        .checked_add(usize::try_from(chunk.length)?)
                        .ok_or("Property range exceeds address space")?;
                    let properties = PropertySets::parse(
                        bytes
                            .get(start..end)
                            .ok_or("Property range extends outside the file")?,
                    )
                    .map_err(|error| {
                        format!(
                            "{:?}, node {:#x}, data {start:#x}: {error}",
                            path, node.offset
                        )
                    })?;
                    count += properties.sets.iter().map(Vec::len).sum::<usize>();
                }
            }
        }
        println!("{:?}: {count} properties", path);
    }
    Ok(())
}
