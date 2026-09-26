#[path = "support/disk.rs"]
mod disk;
use disk::Disk;
use onestore::{
    Arena, ExGuid, RevisionIndex, Section, Store, Transaction,
    document::{Document, Kind},
    op::{Edit, Op, PageOp},
};
use std::fs;

#[test]
fn incomplete_native_save_is_rejected_before_an_edit() {
    let source = fs::read("../../corpus/malformed/native-inflight.one").unwrap();
    let store = Store::parse(&source).unwrap();
    let graph_error = RevisionIndex::parse(&store)
        .unwrap()
        .validate_current()
        .unwrap_err();
    let arena = Arena::default();
    let error = Section::open(&arena, source.clone()).err().unwrap();
    assert_eq!(error.to_string(), graph_error.to_string());
}

/// The text object holding `text` and its page space.
fn target(source: &[u8], text: &str) -> (ExGuid, ExGuid) {
    let store = Store::parse(source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    document
        .spaces
        .iter()
        .find_map(|(space, view)| {
            view.active()?.nodes.iter().find_map(|(id, node)| {
                matches!(&node.kind, Kind::RichText { text: stored, .. } if stored == text)
                    .then_some((*space, *id))
            })
        })
        .unwrap()
}

/// The text `object` holds in `image`, which must be valid.
fn text_of(image: &[u8], space: ExGuid, object: ExGuid) -> String {
    let store = Store::parse(image).unwrap();
    assert!(store.checksum_mismatches.is_empty());
    let index = RevisionIndex::parse(&store).unwrap();
    index.validate_current().unwrap();
    let document = Document::parse(&index).unwrap();
    let Kind::RichText { text, .. } = &document.spaces[&space].active().unwrap().nodes[&object].kind
    else {
        panic!()
    };
    text.clone()
}

#[test]
fn repeated_appends_cross_log_fragments_and_expose_counter_tears() {
    let initial =
        fs::read("../../corpus/native/20260905-05/snapshots/02-text/notebook/synthetic.one")
            .unwrap();
    let store = Store::parse(&initial).unwrap();
    let (space, object) = target(&initial, "Fictitious plain text.");
    let root = store.header.root;
    let log = store.header.transaction_log;
    let initial_count = store.header.transaction_count;
    let arena = Arena::default();
    let mut section = Section::open(&arena, initial.clone()).unwrap();
    let mut source = initial.clone();
    let mut length = "Fictitious plain text.".encode_utf16().count() as u32;
    let mut saw_counter_tear = false;
    for count in initial_count + 1..=258 {
        let value = format!("Transaction {count}");
        let op = PageOp::Text {
            text: object,
            range: 0..length,
            with: value.clone(),
        };
        length = value.encode_utf16().count() as u32;
        section
            .apply("Author", &Edit { at: 133_700_000_000_000_000 + u64::from(count), ops: vec![Op::Page { space, op }] })
            .unwrap();
        let transaction = section.seal().unwrap().unwrap();
        let mut written = source.clone();
        transaction.apply(&mut written).unwrap();
        let store = Store::parse(&written).unwrap();
        assert_eq!(store.header.root, root);
        assert_eq!(store.header.transaction_log, log);
        assert_eq!(store.header.transaction_count, count);
        assert_eq!(text_of(&written, space, object), value);
        let mut previous = written.clone();
        previous[..1024].copy_from_slice(&source[..1024]);
        let old = Store::parse(&previous).unwrap();
        assert_eq!(old.header.transaction_count, count - 1);
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
            if count == 256 && prefix == 97 {
                assert!(result.is_err());
                saw_counter_tear = true;
            } else {
                result.unwrap();
            }
        }
        if count == initial_count + 1 || count == 256 {
            check_crashes(&source, &transaction, space, object, &value);
        }
        if count == initial_count + 1 {
            let path = std::env::temp_dir().join(format!(
                "onestore-commit-{}-{}.one",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::write(&path, &source).unwrap();
            transaction.commit_file(&path).unwrap();
            let committed = fs::read(&path).unwrap();
            assert_eq!(committed, written);
            // The file moved past the transaction's base: nothing is written again.
            let stale = transaction.commit_file(&path).unwrap_err();
            assert_eq!(stale.state, onestore::CommitState::NotCommitted);
            assert_eq!(fs::read(&path).unwrap(), committed);
            fs::remove_file(path).unwrap();
        }
        source = written;
    }
    assert!(saw_counter_tear);
    assert!(section.image() == source);
}

fn check_crashes(
    source: &[u8],
    transaction: &Transaction,
    space: ExGuid,
    object: ExGuid,
    value: &str,
) {
    let before = text_of(source, space, object);
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
        transaction.commit(&mut successful).unwrap();
        assert_eq!(text_of(&successful.durable, space, object), value);
        for at in 1..=successful.operation {
            for seed in [0, 1, 42, u64::MAX] {
                let mut interrupted = disk(Some(at), seed);
                let failure = transaction.commit(&mut interrupted).unwrap_err();
                let persisted = text_of(&interrupted.durable, space, object);
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
