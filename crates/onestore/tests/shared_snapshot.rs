#[path = "support/current.rs"]
mod current;
use current::current;

#[path = "support/checkpoint.rs"]
mod checkpoint;
#[path = "support/ops.rs"]
mod ops;
#[path = "support/trace.rs"]
mod trace;

use onestore::read_snapshot;
use onestore::{
    ExGuid, RevisionIndex, Store,
    document::{Document, Kind},
};
use std::{fs, io};
use trace::{Event, Trace};

/// Types `with` over `range` of the text object in `source` and commits it to `io`.
fn commit_text(
    io: &mut impl onestore::CommitIo,
    source: &[u8],
    space: ExGuid,
    text: ExGuid,
    range: std::ops::Range<u32>,
    with: &str,
) -> Result<(), onestore::CommitError> {
    let op = onestore::op::PageOp::Text {
        text,
        range,
        with: with.into(),
    };
    ops::transaction(source, "Author", vec![onestore::op::Op::Page { space, op }])
        .unwrap()
        .unwrap()
        .commit(io)
}

#[test]
fn storage_inspection_preserves_opaque_images_without_claiming_edit_readiness() {
    for path in [
        "native-encrypted/encrypted-01/notebook/synthetic.one",
        "native-protected-boundaries/notebook/synthetic.one",
        "native-encrypted/cold-encrypted-02/notebook/Open Notebook.one",
        "malformed/native-inflight.one",
    ] {
        let source = fs::read(format!("../../corpus/{path}")).unwrap();
        for block in [1, 17, 65536] {
            let mut read = |offset: u64, output: &mut [u8]| {
                let offset = usize::try_from(offset).unwrap();
                let count = output
                    .len()
                    .min(block)
                    .min(source.len().saturating_sub(offset));
                output[..count].copy_from_slice(&source[offset..offset + count]);
                Ok(count)
            };
            assert_eq!(
                onestore::read_storage_snapshot(&mut read, source.len()).unwrap(),
                Some(source.clone())
            );
            assert!(read_snapshot(&mut read, source.len()).unwrap().is_none());
        }
        let mut headers = 0;
        let changed = onestore::read_storage_snapshot(
            |offset, output| {
                let offset = usize::try_from(offset).unwrap();
                let count = output.len().min(source.len().saturating_sub(offset));
                output[..count].copy_from_slice(&source[offset..offset + count]);
                if offset == 0 {
                    headers += 1;
                    if headers == 2 {
                        output[0] ^= 1;
                    }
                }
                Ok(count)
            },
            source.len(),
        )
        .unwrap();
        assert!(changed.is_none());
        let mut broken = source.clone();
        let offset = usize::try_from(Store::parse(&source).unwrap().header.root.offset).unwrap();
        broken[offset] ^= 1;
        assert_eq!(
            onestore::read_storage_snapshot(
                |offset, output| {
                    let offset = usize::try_from(offset).unwrap();
                    let count = output.len().min(broken.len().saturating_sub(offset));
                    output[..count].copy_from_slice(&broken[offset..offset + count]);
                    Ok(count)
                },
                broken.len()
            )
            .unwrap_err()
            .kind(),
            std::io::ErrorKind::InvalidData
        );
    }
}

fn target(bytes: &[u8]) -> (ExGuid, ExGuid, u32) {
    let store = Store::parse(bytes).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let doc = Document::parse(&index).unwrap();
    doc.spaces
        .iter()
        .find_map(|(sid, space)| {
            space.revisions[&space.contexts[&ExGuid::default()]]
                .nodes
                .iter()
                .find_map(|(oid, node)| {
                    if let Kind::RichText { text, .. } = &node.kind
                        && (text.starts_with("Fictitious") || text.starts_with("Transaction"))
                    {
                        return Some((*sid, *oid, text.encode_utf16().count() as u32));
                    }
                    None
                })
        })
        .unwrap()
}

#[test]
fn version_cached_readers_cannot_miss_a_completed_publication() {
    for path in [
        "native/20260905-05/snapshots/03-format-unicode/notebook/synthetic.one",
        "append/round-01/tx-255/notebook/synthetic.one",
    ] {
        let source = fs::read(format!("../../corpus/{path}")).unwrap();
        let (sid, oid, end) = target(&source);
        let mut trace = Trace {
            bytes: source.clone(),
            events: Vec::new(),
        };
        commit_text(&mut trace, &source, sid, oid, end..end, " [cached reader]").unwrap();
        let final_content = current(&trace.bytes);
        let mut visible = source;
        for event in &trace.events {
            let Event::Write(offset, bytes) = event else {
                continue;
            };
            visible.resize(visible.len().max(offset + bytes.len()), 0);
            for (index, byte) in bytes.iter().enumerate() {
                visible[offset + index] = *byte;
                if (212..252).contains(&(offset + index)) {
                    let cached_content = current(&visible);
                    let refreshed = if visible[212..252] == trace.bytes[212..252] {
                        cached_content
                    } else {
                        current(&trace.bytes)
                    };
                    assert_eq!(
                        refreshed,
                        final_content,
                        "{path}: cached header at {}",
                        offset + index
                    );
                }
            }
        }
    }
}

#[test]
fn published_snapshots_survive_interleaved_commit_io() {
    let cases = [
        (
            "unicode",
            "native/20260905-05/snapshots/03-format-unicode/notebook/synthetic.one",
        ),
        (
            "attachment",
            "native/20260905-05/snapshots/06-attachment/notebook/synthetic.one",
        ),
        (
            "rollover-256",
            "append/round-01/tx-255/notebook/synthetic.one",
        ),
        (
            "rollover-65536",
            "append/round-01/tx-65535/notebook/synthetic.one",
        ),
        (
            "checkpoint",
            "native/20260905-05/snapshots/03-format-unicode/notebook/synthetic.one",
        ),
    ];
    for (name, path) in cases {
        let mut source = fs::read(format!("../../corpus/{path}")).unwrap();
        let (sid, oid, _) = target(&source);
        if name == "checkpoint" {
            source = checkpoint::pending(&source, sid, oid);
        }
        let (_, _, end) = target(&source);
        let before = current(&source);
        let mut trace = Trace {
            bytes: source.clone(),
            events: Vec::new(),
        };
        commit_text(&mut trace, &source, sid, oid, end..end, " [reader café 🦀]").unwrap();
        let after = current(&trace.bytes);
        assert_ne!(before, after);
        let mut writes = Vec::new();
        for event in &trace.events {
            if let Event::Write(offset, bytes) = event {
                let piece = if *offset < 1024 { 17 } else { 4096 };
                writes.extend(
                    bytes
                        .chunks(piece)
                        .enumerate()
                        .map(|(i, bytes)| (offset + i * piece, bytes)),
                );
            }
        }
        let mut accepted = [0; 2];
        let mut retried = 0;
        let mut interleaved = 0;
        for run in 0..writes.len() + 1 + 512 {
            let paused = run <= writes.len();
            let mut step = if paused { run } else { 0 };
            let mut visible = source.clone();
            for &(offset, bytes) in &writes[..step] {
                visible.resize(visible.len().max(offset + bytes.len()), 0);
                visible[offset..offset + bytes.len()].copy_from_slice(bytes);
            }
            let mut seed = run as u64 + 1;
            let mut read_calls = 0;
            let mut overlapped = false;
            let read_limit = [17, 193, 4096, 65536][run % 4];
            let result = read_snapshot(
                |offset, output| {
                    seed ^= seed << 13;
                    seed ^= seed >> 7;
                    seed ^= seed << 17;
                    if !paused {
                        let count = if seed.is_multiple_of(11) {
                            writes.len()
                        } else {
                            (seed % 4) as usize
                        };
                        let end = writes.len().min(step + count);
                        for &(at, bytes) in &writes[step..end] {
                            visible.resize(visible.len().max(at + bytes.len()), 0);
                            visible[at..at + bytes.len()].copy_from_slice(bytes);
                        }
                        overlapped |= read_calls > 0 && end > step;
                        step = end;
                    }
                    read_calls += 1;
                    let offset = offset as usize;
                    let count = output
                        .len()
                        .min(read_limit)
                        .min(visible.len().saturating_sub(offset));
                    if count != 0 {
                        output[..count].copy_from_slice(&visible[offset..offset + count]);
                    }
                    Ok(count)
                },
                trace.bytes.len(),
            );
            interleaved += usize::from(overlapped);
            if paused && (run == 0 || run == writes.len()) {
                assert!(
                    matches!(&result, Ok(Some(_))),
                    "{name}: quiescent run {run}"
                );
            }
            match result {
                Ok(Some(bytes)) => {
                    let checked = std::panic::catch_unwind(|| {
                        let observed = current(&bytes);
                        assert!(
                            observed == before || observed == after,
                            "{name}: run {run}, write step {step}"
                        );
                        if run == writes.len() {
                            assert_eq!(observed, after);
                        }
                        observed == after
                    });
                    match checked {
                        Ok(new) => accepted[usize::from(new)] += 1,
                        Err(failure) => {
                            let time = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap()
                                .as_nanos();
                            let path = std::path::PathBuf::from(format!(
                                "../../evidence/m9/read-interleaving-failure-{name}-{run}-{time}"
                            ));
                            fs::create_dir_all(&path).unwrap();
                            fs::write(path.join("source.one"), &source).unwrap();
                            fs::write(path.join("observed.one"), &bytes).unwrap();
                            fs::write(
                                path.join("replay.json"),
                                serde_json::to_vec(&serde_json::json!({
                                    "run": run, "read_limit": read_limit, "writes": writes,
                                }))
                                .unwrap(),
                            )
                            .unwrap();
                            eprintln!("Replay: {}", path.display());
                            std::panic::resume_unwind(failure);
                        }
                    }
                }
                Ok(None) => retried += 1,
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::UnexpectedEof | io::ErrorKind::InvalidData
                    ) =>
                {
                    retried += 1
                }
                Err(error) => panic!("{name}: run {run}: {error}"),
            }
        }
        assert!(
            accepted[0] > 0 && accepted[1] > 0 && interleaved > 0 && retried > 0,
            "{name}"
        );
        println!(
            "{name}: old={}, new={}, retry={retried}, overlap={interleaved}",
            accepted[0], accepted[1]
        );
    }
}

#[test]
fn snapshot_rejects_short_io_and_unbounded_allocation() {
    let source = onestore::create_section("test.one", "read", "test").unwrap();
    let mut calls = 0;
    let result = read_snapshot(
        |offset, output| {
            calls += 1;
            if calls == 1 {
                return Err(io::ErrorKind::Interrupted.into());
            }
            let offset = offset as usize;
            let count = output.len().min(7).min(source.len().saturating_sub(offset));
            if count != 0 {
                output[..count].copy_from_slice(&source[offset..offset + count]);
            }
            Ok(count)
        },
        source.len(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(current(&source), current(&result));
    assert!(calls > source.len() / 7);
    let error = read_snapshot(|_, output| Ok(output.len() + 1), source.len()).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    let error = read_snapshot(|_, _| Ok(0), source.len()).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
    let error = read_snapshot(
        |_, output| {
            output.copy_from_slice(&source[..1024]);
            Ok(1024)
        },
        1023,
    )
    .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
}

#[test]
fn native_tail_truncation_before_header_update_is_retried() {
    let compact = onestore::create_section("test.one", "Transaction retained", "test").unwrap();
    let mut expanded = compact.clone();
    expanded.resize(compact.len() + 216, 0);
    let length = expanded.len() as u64;
    expanded[196..204].copy_from_slice(&length.to_le_bytes());
    assert_eq!(current(&expanded), current(&compact));
    for block in [17, 193, 65536] {
        for trigger in [0, 1024usize.div_ceil(block)] {
            let mut visible = expanded.clone();
            let mut calls = 0;
            let result = read_snapshot(
                |offset, output| {
                    if calls == trigger {
                        visible.truncate(compact.len());
                    }
                    calls += 1;
                    let offset = offset as usize;
                    let count = output
                        .len()
                        .min(block)
                        .min(visible.len().saturating_sub(offset));
                    if count != 0 {
                        output[..count].copy_from_slice(&visible[offset..offset + count]);
                    }
                    Ok(count)
                },
                expanded.len(),
            )
            .unwrap();
            assert!(
                result.is_none(),
                "accepted a snapshot with a stale expected length"
            );
        }
    }
    let result = read_snapshot(
        |offset, output| {
            let offset = offset as usize;
            let count = output.len().min(compact.len().saturating_sub(offset));
            if count != 0 {
                output[..count].copy_from_slice(&compact[offset..offset + count]);
            }
            Ok(count)
        },
        expanded.len(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(result, compact);
}

#[test]
fn unpublished_append_remains_available_for_retry() {
    let source = onestore::create_section("test.one", "Transaction before", "test").unwrap();
    let (sid, oid, end) = target(&source);
    let mut trace = Trace {
        bytes: source.clone(),
        events: Vec::new(),
    };
    commit_text(&mut trace, &source, sid, oid, end..end, " abandoned").unwrap();
    let Event::Write(offset, append) = &trace.events[0] else {
        panic!()
    };
    assert_eq!(*offset, source.len());
    for length in [1, 17, append.len()] {
        let mut persisted = source.clone();
        persisted.extend_from_slice(&append[..length]);
        let snapshot = read_snapshot(
            |offset, output| {
                let offset = offset as usize;
                let count = output.len().min(persisted.len().saturating_sub(offset));
                output[..count].copy_from_slice(&persisted[offset..offset + count]);
                Ok(count)
            },
            persisted.len(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(snapshot, persisted);
        assert_eq!(current(&snapshot), current(&source));
        let mut retry = Trace {
            bytes: persisted,
            events: Vec::new(),
        };
        commit_text(&mut retry, &snapshot, sid, oid, end..end, " retry").unwrap();
        assert_ne!(current(&retry.bytes), current(&source));
    }
}

#[test]
fn header_comparison_cannot_replace_maintenance_exclusion() {
    let mut source = onestore::create_section("test.one", "AAAA BBBB", "test").unwrap();
    source[212..228].fill(9);
    source[236..252].fill(7);
    let encoded: Vec<_> = "AAAA BBBB"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    let offset = source
        .windows(encoded.len())
        .position(|bytes| bytes == encoded)
        .unwrap();
    let mut changed = source.clone();
    let replacement: Vec<_> = "ZZZZ YYYY"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    changed[offset..offset + replacement.len()].copy_from_slice(&replacement);
    let before = current(&source);
    let after = current(&changed);
    assert_ne!(before, after);
    let mut visible = &source;
    let result = read_snapshot(
        |at, output| {
            let at = at as usize;
            if at >= offset + 8 {
                visible = &changed;
            }
            let count = output.len().min(2).min(visible.len().saturating_sub(at));
            if count != 0 {
                output[..count].copy_from_slice(&visible[at..at + count]);
            }
            Ok(count)
        },
        source.len(),
    )
    .unwrap()
    .unwrap();
    let hybrid = current(&result);
    assert_ne!(hybrid, before);
    assert_ne!(hybrid, after);
}
