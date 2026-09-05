use onestore::{Reference, Store};
use std::{collections::BTreeSet, fs};

#[test]
#[ignore = "Generates the local fuzz corpus"]
fn seed_property_fuzzer() {
    let bytes =
        fs::read("../../corpus/native/20260905-05/snapshots/07-table/notebook/synthetic.one")
            .unwrap();
    let store = Store::parse(&bytes).unwrap();
    let mut seeds = BTreeSet::new();
    for node in store.lists.values().flat_map(|list| &list.nodes) {
        if matches!(node.id, 0xa4 | 0xa5 | 0xc4 | 0xc5)
            && let Some(Reference::Data(chunk)) = node.reference
        {
            let start = usize::try_from(chunk.offset).unwrap();
            let end = start + usize::try_from(chunk.length).unwrap();
            seeds.insert(&bytes[start..end]);
        }
    }
    fs::create_dir_all("../../fuzz/corpus/properties").unwrap();
    for (index, seed) in seeds.iter().enumerate() {
        fs::write(format!("../../fuzz/corpus/properties/native-{index}"), seed).unwrap();
    }
}
