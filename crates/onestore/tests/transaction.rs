#[path = "support/ops.rs"]
mod ops;

use onestore::{
    CommitIo, CommitState, ExGuid, RevisionIndex, Stamp, Store, Transaction,
    document::{Document, Kind},
    op::{Op, PageOp},
};
use std::io;

/// Crosses the 65535 → 65536 counter carry on its next transaction.
const SOURCE: &[u8] =
    include_bytes!("../../../corpus/append/round-01/tx-65535/notebook/synthetic.one");

fn text(source: &[u8]) -> (ExGuid, ExGuid) {
    let store = Store::parse(source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    for (sid, space) in &document.spaces {
        let revision = &space.revisions[&space.contexts[&ExGuid::default()]];
        for (oid, node) in &revision.nodes {
            if matches!(node.kind, Kind::RichText { .. }) {
                return (*sid, *oid);
            }
        }
    }
    panic!("Missing text fixture")
}

/// The transaction typing `with` at the start of the text object, and the image it leaves.
fn typed(space: ExGuid, text: ExGuid, with: &str) -> (Transaction, Vec<u8>) {
    let op = PageOp::Text {
        text,
        range: 0..0,
        with: with.into(),
    };
    let transaction = ops::transaction(SOURCE, "Author", vec![Op::Page { space, op }])
        .unwrap()
        .unwrap();
    let mut image = SOURCE.to_vec();
    transaction.apply(&mut image).unwrap();
    (transaction, image)
}

/// A file that records how many bytes a commit reads and writes.
struct Counted {
    bytes: Vec<u8>,
    read: usize,
    written: usize,
}

impl CommitIo for Counted {
    fn read_at(&mut self, offset: u64, output: &mut [u8]) -> io::Result<usize> {
        let offset = offset as usize;
        let count = output.len().min(self.bytes.len().saturating_sub(offset));
        output[..count].copy_from_slice(&self.bytes[offset..offset + count]);
        self.read += count;
        Ok(count)
    }
    fn write_at(&mut self, offset: u64, bytes: &[u8]) -> io::Result<usize> {
        let offset = offset as usize;
        self.bytes
            .resize(self.bytes.len().max(offset + bytes.len()), 0);
        self.bytes[offset..offset + bytes.len()].copy_from_slice(bytes);
        self.written += bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn a_transaction_reads_the_header_and_writes_only_its_edit() {
    let (sid, oid) = text(SOURCE);
    let (transaction, written) = typed(sid, oid, "é");
    let mut image = SOURCE.to_vec();
    transaction.apply(&mut image).unwrap();
    assert_eq!(image, written);
    let mut file = Counted {
        bytes: SOURCE.to_vec(),
        read: 0,
        written: 0,
    };
    transaction.commit(&mut file).unwrap();
    assert_eq!(file.bytes, written);
    assert!(file.read <= 1024 + 2, "read {} bytes", file.read);
    let appended = written.len() - SOURCE.len();
    assert!(
        file.written < appended + 2048,
        "wrote {} bytes for {appended} appended",
        file.written
    );
    // The file has moved past the base: nothing is written, and applying needs the base.
    let (read, written) = (file.read, file.written);
    let error = transaction.commit(&mut file).unwrap_err();
    assert_eq!(error.state, CommitState::NotCommitted);
    assert_eq!(error.error.kind(), io::ErrorKind::ResourceBusy);
    assert_eq!(file.written, written);
    assert!(file.read - read <= 1024);
    assert!(transaction.apply(&mut image).is_err());
}

#[test]
fn stamps_name_the_committed_image() {
    let (sid, oid) = text(SOURCE);
    let (transaction, written) = typed(sid, oid, "x");
    let before = Stamp::of(SOURCE).unwrap();
    let after = Stamp::of(&written).unwrap();
    assert_ne!(before, after);
    assert_eq!(before.length, SOURCE.len() as u64);
    // An unpublished tail changes the length, so a commit built without it is refused.
    let mut tail = SOURCE.to_vec();
    tail.extend_from_slice(&[0; 8]);
    assert_ne!(Stamp::of(&tail).unwrap(), before);
    let mut file = Counted {
        bytes: tail,
        read: 0,
        written: 0,
    };
    let error = transaction.commit(&mut file).unwrap_err();
    assert_eq!(error.error.kind(), io::ErrorKind::ResourceBusy);
    assert_eq!(file.written, 0);
    assert!(Stamp::of(&SOURCE[..1023]).is_err());
}
