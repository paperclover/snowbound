#![no_main]
use libfuzzer_sys::fuzz_target;
use onestore::{
    ExGuid, ObjectData, PropertySets, RevisionIndex, Store, Value, replace_property_bytes,
};
use std::sync::LazyLock;

const SOURCE: &[u8] =
    include_bytes!("../../corpus/native/20260905-05/snapshots/02-text/notebook/synthetic.one");
static TARGET: LazyLock<(ExGuid, ExGuid, u32)> = LazyLock::new(|| {
    let store = Store::parse(SOURCE).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    for (osid, space) in &index.spaces {
        let revision = index
            .resolve(*osid, space.labels[&(ExGuid::default(), 1)])
            .unwrap();
        for (oid, object) in revision.objects {
            if let ObjectData::Properties(data) = object.data {
                for property in &PropertySets::parse(data).unwrap().sets[0] {
                    if property.value == Value::Bytes(b"Fictitious plain text.") {
                        return (*osid, oid, property.id);
                    }
                }
            }
        }
    }
    panic!("Missing native seed property")
});

fuzz_target!(|value: &[u8]| {
    let (osid, oid, property) = *TARGET;
    let bytes = replace_property_bytes(SOURCE, osid, oid, property, value).unwrap();
    let mut before_commit = bytes.clone();
    before_commit[..1024].copy_from_slice(&SOURCE[..1024]);
    let old = Store::parse(&before_commit).unwrap();
    assert!(old.checksum_mismatches.is_empty());
    RevisionIndex::parse(&old)
        .unwrap()
        .validate_current()
        .unwrap();
    let store = Store::parse(&bytes).unwrap();
    assert!(store.checksum_mismatches.is_empty());
    let index = RevisionIndex::parse(&store).unwrap();
    let rid = index.spaces[&osid].labels[&(ExGuid::default(), 1)];
    let current = index.resolve(osid, rid).unwrap();
    let ObjectData::Properties(data) = current.objects[&oid].data else {
        panic!()
    };
    let properties = PropertySets::parse(data).unwrap();
    assert_eq!(
        properties.sets[0]
            .iter()
            .find(|candidate| candidate.id == property)
            .unwrap()
            .value,
        Value::Bytes(value)
    );
});
