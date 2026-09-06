use onestore::{ExGuid, RevisionIndex, Store, document::Document};
use std::collections::BTreeMap;

pub fn current(bytes: &[u8]) -> BTreeMap<ExGuid, String> {
    let store = Store::parse(bytes).unwrap();
    assert!(store.checksum_mismatches.is_empty());
    let index = RevisionIndex::parse(&store).unwrap();
    index.validate_current().unwrap();
    Document::parse(&index).unwrap();
    index
        .spaces
        .iter()
        .map(|(sid, space)| {
            let rid = space.labels[&(ExGuid::default(), 1)];
            let revision = index.resolve(*sid, rid).unwrap();
            for object in revision.objects.values() {
                if let Some(onestore::FileDataReference::Internal(guid)) =
                    object.file_reference().unwrap()
                {
                    store.file_data(guid).unwrap();
                }
            }
            (*sid, format!("{revision:?}"))
        })
        .collect()
}
