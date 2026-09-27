#![no_main]
use libfuzzer_sys::fuzz_target;
use onestore::{
    CommitState, ExGuid, RevisionIndex, Store,
    document::{Document, Kind},
    op::{Op, PageOp},
};
use std::sync::LazyLock;

#[path = "../../crates/onestore/tests/support/disk.rs"]
mod disk;
#[path = "../../crates/onestore/tests/support/ops.rs"]
mod ops;
use disk::Disk;

const SOURCES: [&[u8]; 2] = [
    include_bytes!("../../corpus/native/20260905-05/snapshots/02-text/notebook/synthetic.one"),
    include_bytes!("../../corpus/append/round-01/tx-255/notebook/synthetic.one"),
];
static TARGET: LazyLock<(ExGuid, ExGuid)> = LazyLock::new(|| {
    let store = Store::parse(SOURCES[0]).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    for (sid, space) in &document.spaces {
        let revision = &space.revisions[&space.contexts[&ExGuid::default()]];
        for (oid, node) in &revision.nodes {
            if matches!(&node.kind, Kind::RichText { text, .. } if text == "Fictitious plain text.")
            {
                return (*sid, *oid);
            }
        }
    }
    panic!("Missing native seed text")
});

fn current(bytes: &[u8]) -> String {
    let store = Store::parse(bytes).unwrap();
    assert!(store.checksum_mismatches.is_empty());
    let index = RevisionIndex::parse(&store).unwrap();
    index.validate_current().unwrap();
    let document = Document::parse(&index).unwrap();
    let (sid, oid) = *TARGET;
    let space = &document.spaces[&sid];
    let revision = &space.revisions[&space.contexts[&ExGuid::default()]];
    let Kind::RichText { text, .. } = &revision.nodes[&oid].kind else {
        panic!()
    };
    text.clone()
}

fuzz_target!(|input: &[u8]| {
    if input.len() < 12 {
        return;
    }
    let mut source = SOURCES[usize::from(input[0].is_multiple_of(4))].to_vec();
    let (sid, oid) = *TARGET;
    for step in 0..=input[1] % 4 {
        let value: String = input[12..]
            .iter()
            .chain([&step])
            .map(|byte| char::from(b'a' + byte % 26))
            .collect();
        let before = current(&source);
        let op = PageOp::Text {
            text: oid,
            range: 0..before.encode_utf16().count() as u32,
            with: value.clone(),
        };
        let Some(transaction) =
            ops::transaction(&source, "Fuzz", vec![Op::Page { space: sid, op }]).unwrap()
        else {
            assert_eq!(before, value);
            continue;
        };
        let mut storage = Disk {
            visible: source.clone(),
            durable: source.clone(),
            operation: 0,
            fail_at: Some(usize::from(u16::from_le_bytes([input[2], input[3]]))),
            write_limit: usize::from(input[4]) + 1,
            random: u64::from_le_bytes(input[4..12].try_into().unwrap()),
        };
        let result = transaction.commit(&mut storage);
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
