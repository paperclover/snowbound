#![no_main]
use libfuzzer_sys::fuzz_target;
use onestore::{RevisionIndex, Store};

fuzz_target!(|data: &[u8]| {
    if let Ok(store) = Store::parse(data)
        && let Ok(index) = RevisionIndex::parse(&store)
    {
        let _ = index.validate_current();
        let _ = onestore::document::Document::parse(&index);
    }
});
