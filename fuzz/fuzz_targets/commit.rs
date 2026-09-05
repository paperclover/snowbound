#![no_main]
use libfuzzer_sys::fuzz_target;
use onestore::{CommitState, ExGuid, ObjectData, PropertySets, RevisionIndex, Store, Value};
use std::sync::LazyLock;

#[path = "../../crates/onestore/tests/support/disk.rs"]
mod disk;
use disk::Disk;

const SOURCES: [&[u8]; 2] = [
    include_bytes!("../../corpus/native/20260905-05/snapshots/02-text/notebook/synthetic.one"),
    include_bytes!("../../corpus/append/round-01/tx-255/notebook/synthetic.one"),
];
static TARGET: LazyLock<(ExGuid, ExGuid, u32)> = LazyLock::new(|| {
    let store = Store::parse(SOURCES[0]).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    for (osid, space) in &index.spaces {
        let revision = index
            .resolve(*osid, space.labels[&(ExGuid::default(), 1)])
            .unwrap();
        for (oid, object) in revision.objects {
            if let ObjectData::Properties(blob) = object.data {
                for property in &PropertySets::parse(blob).unwrap().sets[0] {
                    if property.value == Value::Bytes(b"Fictitious plain text.") {
                        return (*osid, oid, property.id);
                    }
                }
            }
        }
    }
    panic!("Missing native seed property")
});

fn current(bytes: &[u8]) -> Vec<u8> {
    let store = Store::parse(bytes).unwrap();
    assert!(store.checksum_mismatches.is_empty());
    let index = RevisionIndex::parse(&store).unwrap();
    index.validate_current().unwrap();
    let (osid, oid, property) = *TARGET;
    let revision = index
        .resolve(osid, index.spaces[&osid].labels[&(ExGuid::default(), 1)])
        .unwrap();
    let ObjectData::Properties(blob) = revision.objects[&oid].data else {
        panic!()
    };
    let properties = PropertySets::parse(blob).unwrap();
    let Value::Bytes(value) = properties.sets[0]
        .iter()
        .find(|p| p.id == property)
        .unwrap()
        .value
    else {
        panic!()
    };
    value.to_vec()
}

fuzz_target!(|input: &[u8]| {
    if input.len() < 12 {
        return;
    }
    let mut source = SOURCES[usize::from(input[0].is_multiple_of(4))].to_vec();
    let (osid, oid, property) = *TARGET;
    for step in 0..=input[1] % 4 {
        let mut value = input[12..].to_vec();
        value.push(step);
        let before = current(&source);
        let mut storage = Disk {
            visible: source.clone(),
            durable: source.clone(),
            operation: 0,
            fail_at: Some(usize::from(u16::from_le_bytes([input[2], input[3]]))),
            write_limit: usize::from(input[4]) + 1,
            random: u64::from_le_bytes(input[4..12].try_into().unwrap()),
        };
        let result =
            onestore::commit_property_bytes(&mut storage, &source, osid, oid, property, &value);
        let persisted = current(&storage.durable);
        match result {
            Ok(()) => assert_eq!(persisted, value),
            Err(error) => match error.state {
                CommitState::NotCommitted => assert_eq!(persisted, before),
                CommitState::Committed => assert_eq!(persisted, value),
                CommitState::Unknown => assert!(persisted == before || persisted == value),
            },
        }
        source = storage.durable;
    }
});
