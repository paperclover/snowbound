use onestore::Store;
use std::{env, fs};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    for path in env::args_os().skip(1) {
        let bytes = fs::read(&path)?;
        let store = Store::parse(&bytes)?;
        let nodes: usize = store.lists.values().map(|list| list.nodes.len()).sum();
        println!(
            "{:?}: {:?}, {} transactions, {} lists, {nodes} nodes, {} checksum mismatches",
            path,
            store.header.file_type,
            store.header.transaction_count,
            store.lists.len(),
            store.checksum_mismatches.len()
        );
    }
    Ok(())
}
