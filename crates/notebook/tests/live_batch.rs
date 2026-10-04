#![cfg(feature = "live")]

use notebook::{
    Replica,
    live::share::{HostedRemote, Sharing},
};
use onestore::op::{Edit, Op, PageOp};
use std::sync::{Arc, Barrier};

#[path = "support/live.rs"]
mod live;
use live::*;

#[test]
fn a_lost_batch_receipt_is_confirmed_without_repeating_its_edit() {
    use notebook::{PendingEdit, Remote};
    use onestore::{CommitError, CommitState, ExGuid, Stamp, Transaction};
    use std::{collections::BTreeMap, io};

    struct Lost(HostedRemote, bool);
    impl Remote for Lost {
        fn read(&mut self) -> io::Result<Vec<u8>> {
            self.0.read()
        }
        fn stamp(&mut self) -> io::Result<Stamp> {
            self.0.stamp()
        }
        fn publish(&mut self, transaction: &Transaction) -> Result<(), CommitError> {
            self.0.publish(transaction)
        }
        fn confirm(&mut self, stamp: &Stamp) -> Result<(), CommitError> {
            self.0.confirm(stamp)
        }
        fn accepts_edits(&self) -> bool {
            true
        }
        fn publish_edits(
            &mut self,
            transaction: &Transaction,
            edits: &[PendingEdit],
            revisions: &BTreeMap<ExGuid, ExGuid>,
        ) -> Result<(), CommitError> {
            self.0.publish_edits(transaction, edits, revisions)?;
            if std::mem::take(&mut self.1) {
                return Err(CommitError {
                    state: CommitState::Unknown,
                    error: io::ErrorKind::BrokenPipe.into(),
                });
            }
            Ok(())
        }
    }
    let directory = tempfile::tempdir().unwrap();
    let folder = notebook(directory.path());
    let url = relay(Default::default());
    let sharing = Sharing::new("").unwrap();
    let host = host(&folder, &directory.path().join("host"), &sharing, &url);
    let (guest, _) = guest("Alice", &code(&host), &url, &directory.path().join("alice"));
    let file = folder.join("Garden.one");
    let image = std::fs::read(&file).unwrap();
    let (space, text, _) = server::text(&image);
    let replica = Replica::open_or_create(directory.path().join("replica.sqlite"), None, || {
        Ok(image.clone())
    })
    .unwrap();
    let id = replica
        .apply(
            "Alice",
            Edit {
                at: 134_000_000_000_000_000,
                ops: vec![Op::Page {
                    space,
                    op: PageOp::Text {
                        text,
                        range: 13..13,
                        with: "X".into(),
                    },
                }],
            },
        )
        .unwrap();
    let mut remote = Lost(HostedRemote::new(&guest, "Garden.one"), true);
    assert!(matches!(
        replica.sync_once(&mut remote),
        Err(notebook::Error::Remote(CommitError {
            state: CommitState::Unknown,
            ..
        }))
    ));
    replica.sync_once(&mut remote).unwrap();
    assert!(matches!(
        replica.status(id).unwrap(),
        Some(notebook::EditStatus::Published { .. })
    ));
    assert_eq!(
        server::text(&std::fs::read(file).unwrap()).2,
        "Original textX"
    );
    assert!(replica.pending().unwrap().is_empty());
}

#[test]
fn concurrent_batches_keep_receipts_and_rebase_later_keystrokes() {
    let directory = tempfile::tempdir().unwrap();
    let folder = notebook(directory.path());
    let url = relay(Default::default());
    let sharing = Sharing::new("").unwrap();
    let host = host(&folder, &directory.path().join("host"), &sharing, &url);
    let code = code(&host);
    let (alice, _) = guest("Alice", &code, &url, &directory.path().join("alice"));
    let (bob, _) = guest("Bob", &code, &url, &directory.path().join("bob"));
    let file = folder.join("Garden.one");
    let original = std::fs::read(&file).unwrap();
    let (space, text, _) = server::text(&original);
    let edit = |with: &str, at| Edit {
        at: 134_000_000_000_000_000,
        ops: vec![Op::Page {
            space,
            op: PageOp::Text {
                text,
                range: at..at,
                with: with.into(),
            },
        }],
    };
    let a = Arc::new(
        Replica::open_or_create(directory.path().join("a.sqlite"), None, || {
            Ok(original.clone())
        })
        .unwrap(),
    );
    let b = Arc::new(
        Replica::open_or_create(directory.path().join("b.sqlite"), None, || {
            Ok(original.clone())
        })
        .unwrap(),
    );
    let first = a.apply("Alice", edit("A", 13)).unwrap();
    let second = b.apply("Bob", edit("B", 0)).unwrap();
    let barrier = Arc::new(Barrier::new(2));
    std::thread::scope(|scope| {
        for (replica, guest) in [(&a, &alice), (&b, &bob)] {
            let barrier = Arc::clone(&barrier);
            scope.spawn(move || {
                barrier.wait();
                replica
                    .sync_once(&mut HostedRemote::new(guest, "Garden.one"))
                    .unwrap();
            });
        }
    });
    assert!(matches!(
        a.status(first).unwrap(),
        Some(notebook::EditStatus::Published { .. })
    ));
    assert!(matches!(
        b.status(second).unwrap(),
        Some(notebook::EditStatus::Published { .. })
    ));
    let next = server::text(&a.snapshot().unwrap()).2.find('A').unwrap() as u32 + 1;
    let third = a.apply("Alice", edit("a", next)).unwrap();
    let mut remote = HostedRemote::new(&alice, "Garden.one");
    for _ in 0..5 {
        a.sync_once(&mut remote).unwrap();
        if matches!(
            a.status(third).unwrap(),
            Some(notebook::EditStatus::Published { .. })
        ) {
            break;
        }
    }
    let result = std::fs::read(&file).unwrap();
    let result_text = server::text(&result).2;
    assert!(result_text.contains("Aa"), "{result_text}");
    assert!(result_text.contains('B'), "{result_text}");
    let arena = onestore::Arena::default();
    let mut section = onestore::Section::open(&arena, result).unwrap();
    assert!(section.conflicts().unwrap().is_empty());
}
