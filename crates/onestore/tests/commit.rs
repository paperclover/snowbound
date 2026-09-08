#[path = "support/disk.rs"]
mod disk;
use disk::Disk;
use onestore::{
    ExGuid, ObjectData, PropertySets, RevisionIndex, Store, Value, replace_property_bytes,
};
use std::fs;

#[test]
fn incomplete_native_save_is_rejected_before_storage_io() {
    let source = fs::read("../../corpus/malformed/native-inflight.one").unwrap();
    let store = Store::parse(&source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let graph_error = index.validate_current().unwrap_err();
    let mut checked = 0;
    for (osid, space) in &index.spaces {
        let revision = index
            .resolve(*osid, space.labels[&(ExGuid::default(), 1)])
            .unwrap();
        for oid in revision.reachable().unwrap() {
            let object = &revision.objects[&oid];
            if object.jcid != 0x6000e {
                continue;
            }
            let ObjectData::Properties(data) = object.data else {
                continue;
            };
            for property in &PropertySets::parse(data).unwrap().sets[0] {
                if property.value != Value::Bytes(b"Rust same paragraph.") {
                    continue;
                }
                let mut disk = Disk {
                    visible: source.clone(),
                    durable: source.clone(),
                    operation: 0,
                    fail_at: None,
                    write_limit: 1024,
                    random: 1,
                };
                let error = onestore::commit_property_bytes(
                    &mut disk,
                    &source,
                    *osid,
                    oid,
                    property.id,
                    b"Rust same paragraph.",
                )
                .unwrap_err();
                assert_eq!(error.state, onestore::CommitState::NotCommitted);
                assert_eq!(error.error.to_string(), graph_error.to_string());
                assert_eq!(disk.operation, 0);
                checked += 1;
            }
        }
    }
    assert_eq!(checked, 1);
}

#[test]
fn repeated_appends_cross_log_fragments_and_expose_counter_tears() {
    let mut source =
        fs::read("../../corpus/native/20260905-05/snapshots/02-text/notebook/synthetic.one")
            .unwrap();
    let store = Store::parse(&source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let mut target = None;
    for (osid, space) in &index.spaces {
        let revision = index
            .resolve(*osid, space.labels[&(ExGuid::default(), 1)])
            .unwrap();
        for (oid, object) in revision.objects {
            if let ObjectData::Properties(data) = object.data {
                for property in &PropertySets::parse(data).unwrap().sets[0] {
                    if property.value == Value::Bytes(b"Fictitious plain text.") {
                        target = Some((*osid, oid, property.id));
                    }
                }
            }
        }
    }
    let (osid, oid, property) = target.unwrap();
    let root = store.header.root;
    let log = store.header.transaction_log;
    let initial_count = store.header.transaction_count;
    let mut saw_counter_tear = false;
    for transaction in initial_count + 1..=258 {
        let value = format!("Transaction {transaction}");
        let written =
            replace_property_bytes(&source, osid, oid, property, value.as_bytes()).unwrap();
        let store = Store::parse(&written).unwrap();
        assert_eq!(store.header.root, root);
        assert_eq!(store.header.transaction_log, log);
        assert_eq!(store.header.transaction_count, transaction);
        assert!(store.checksum_mismatches.is_empty());
        RevisionIndex::parse(&store)
            .unwrap()
            .validate_current()
            .unwrap();
        let mut previous = written.clone();
        previous[..1024].copy_from_slice(&source[..1024]);
        let old = Store::parse(&previous).unwrap();
        assert_eq!(old.header.transaction_count, transaction - 1);
        assert!(old.checksum_mismatches.is_empty());
        RevisionIndex::parse(&old)
            .unwrap()
            .validate_current()
            .unwrap();
        for prefix in 96..=100 {
            let mut torn = written.clone();
            torn[prefix..1024].copy_from_slice(&source[prefix..1024]);
            let result = Store::parse(&torn)
                .and_then(|store| RevisionIndex::parse(&store)?.validate_current());
            if transaction == 256 && prefix == 97 {
                assert!(result.is_err());
                saw_counter_tear = true;
            } else {
                result.unwrap();
            }
        }
        if transaction == initial_count + 1 || transaction == 256 {
            check_crashes(&source, osid, oid, property, value.as_bytes());
        }
        if transaction == initial_count + 1 {
            let path = std::env::temp_dir().join(format!(
                "onestore-commit-{}-{}.one",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::write(&path, &source).unwrap();
            onestore::commit_file_property(&path, &source, osid, oid, property, value.as_bytes())
                .unwrap();
            let committed = fs::read(&path).unwrap();
            let parsed = Store::parse(&committed).unwrap();
            assert_eq!(parsed.header.transaction_count, transaction);
            RevisionIndex::parse(&parsed)
                .unwrap()
                .validate_current()
                .unwrap();
            let stale =
                onestore::commit_file_property(&path, &source, osid, oid, property, b"stale")
                    .unwrap_err();
            assert_eq!(stale.state, onestore::CommitState::NotCommitted);
            assert_eq!(fs::read(&path).unwrap(), committed);
            onestore::commit_file_property(
                &path,
                &committed,
                osid,
                oid,
                property,
                value.as_bytes(),
            )
            .unwrap();
            assert_eq!(fs::read(&path).unwrap(), committed);
            fs::remove_file(path).unwrap();
        }
        source = written;
    }
    assert!(saw_counter_tear);
}

fn check_crashes(source: &[u8], osid: ExGuid, oid: ExGuid, property: u32, value: &[u8]) {
    let current_value = |data: &[u8]| {
        let store = Store::parse(data).unwrap();
        assert!(store.checksum_mismatches.is_empty());
        let index = RevisionIndex::parse(&store).unwrap();
        index.validate_current().unwrap();
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
    };
    let before = current_value(source);
    for limit in [17, 1024] {
        let disk = |fail_at, random| Disk {
            visible: source.to_vec(),
            durable: source.to_vec(),
            operation: 0,
            fail_at,
            write_limit: limit,
            random,
        };
        let mut successful = disk(None, 1);
        onestore::commit_property_bytes(&mut successful, source, osid, oid, property, value)
            .unwrap();
        assert_eq!(current_value(&successful.durable), value);
        for at in 1..=successful.operation {
            for seed in [0, 1, 42, u64::MAX] {
                let mut interrupted = disk(Some(at), seed);
                let failure = onestore::commit_property_bytes(
                    &mut interrupted,
                    source,
                    osid,
                    oid,
                    property,
                    value,
                )
                .unwrap_err();
                let persisted = current_value(&interrupted.durable);
                match failure.state {
                    onestore::CommitState::NotCommitted => assert_eq!(persisted, before),
                    onestore::CommitState::Committed => assert_eq!(persisted, value),
                    onestore::CommitState::Unknown => {
                        assert!(persisted == before || persisted == value)
                    }
                }
            }
        }
    }
}
#[test]
#[cfg(any(unix, windows))]
fn bounded_file_reads_reject_partial_images_and_release_the_owner() {
    use std::io::Write;
    let path = std::env::temp_dir().join(format!(
        "onestore-bounded-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let mut file = fs::File::options()
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    assert!(onestore::read_file_limited(&path, 0).unwrap().is_empty());
    let bytes: Vec<_> = (0..10_000).map(|i| (i % 251) as u8).collect();
    file.write_all(&bytes).unwrap();
    drop(file);
    for limit in [0, 1, 100, bytes.len() - 1] {
        assert_eq!(
            onestore::read_file_limited(&path, limit)
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::FileTooLarge
        );
        assert_eq!(
            onestore::read_file_limited(&path, bytes.len()).unwrap(),
            bytes
        );
    }
    assert_eq!(onestore::read_file(&path).unwrap(), bytes);
    assert_eq!(fs::read(&path).unwrap(), bytes);
    fs::remove_file(path).unwrap();
}
