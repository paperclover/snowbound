#[path = "../tests/support/current.rs"]
mod current;
use current::current;

#[path = "../tests/support/trace.rs"]
mod trace;
use trace::{Event, Trace};

#[path = "../tests/support/checkpoint.rs"]
mod checkpoint;
#[path = "support/typing.rs"]
mod typing;

use onestore::{
    ExGuid, RevisionIndex, Store,
    document::{Document, Kind},
};
use std::{collections::BTreeMap, fs, path::PathBuf};

/// `source`, a table of contents, with the notebook recoloured to `color`.
fn recolored(source: &[u8], color: u32) -> Result<Vec<u8>, onestore::Error> {
    let mut image = source.to_vec();
    let edit = onestore::TocEdit::Color(color);
    if let Some(transaction) = onestore::edit_table_of_contents(source, &[edit])? {
        transaction.apply(&mut image)?;
    }
    Ok(image)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let destination = PathBuf::from(args.next().ok_or("Provide a new evidence directory")?);
    let option = args.next();
    if option.as_deref().is_some_and(|flag| flag != "--toc") || args.next().is_some() {
        return Err("Usage: power_loss NEW_DIRECTORY [--toc]".into());
    }
    fs::create_dir(&destination)?;
    let fixtures = [
        (
            "unicode",
            "corpus/native/20260905-05/snapshots/03-format-unicode/notebook/synthetic.one",
        ),
        (
            "rollover-256",
            "corpus/append/round-01/tx-255/notebook/synthetic.one",
        ),
        (
            "rollover-65536",
            "corpus/append/round-01/tx-65535/notebook/synthetic.one",
        ),
        (
            "attachment",
            "corpus/native/20260905-05/snapshots/06-attachment/notebook/synthetic.one",
        ),
        (
            "checkpoint",
            "corpus/native/20260905-05/snapshots/03-format-unicode/notebook/synthetic.one",
        ),
        (
            "toc",
            "corpus/native/20260905-05/snapshots/02-text/notebook/Open Notebook.onetoc2",
        ),
        (
            "toc-rollover",
            "corpus/native/20260905-05/snapshots/02-text/notebook/Open Notebook.onetoc2",
        ),
        (
            "toc-checkpoint",
            "corpus/native/20260905-05/snapshots/02-text/notebook/Open Notebook.onetoc2",
        ),
    ];
    let mut records = Vec::new();
    let mut saved = BTreeMap::new();
    for (name, path) in fixtures {
        if option.is_some() && !name.starts_with("toc") {
            continue;
        }
        println!("Checking {name}");
        let mut source = fs::read(path)?;
        let store = Store::parse(&source)?;
        let index = RevisionIndex::parse(&store)?;
        let document = Document::parse(&index)?;
        let toc = store.header.file_type == onestore::FileType::TableOfContents;
        // A table of contents changes the notebook's colour.
        let (sid, oid) = if toc {
            let root = &document.spaces[&document.root];
            let root = &root.revisions[&root.contexts[&ExGuid::default()]];
            (document.root, root.roots[&1])
        } else {
            document
                .spaces
                .iter()
                .find_map(|(sid, space)| {
                    let revision = &space.revisions[&space.contexts[&ExGuid::default()]];
                    revision
                        .nodes
                        .iter()
                        .find_map(|(oid, node)| match &node.kind {
                            Kind::RichText { text, .. }
                                if text.starts_with("Fictitious") || text == "Transaction 255" =>
                            {
                                Some((*sid, *oid))
                            }
                            _ => None,
                        })
                })
                .ok_or("Missing native fixture text")?
        };
        match name {
            "checkpoint" => source = checkpoint::pending(&source, sid, oid),
            "toc-checkpoint" => {
                for value in 0.. {
                    let store = Store::parse(&source)?;
                    let index = RevisionIndex::parse(&store)?;
                    let rid = index.spaces[&sid].labels[&(ExGuid::default(), 1)];
                    let depth = std::iter::successors(Some(rid), |id| {
                        index.spaces[&sid].revisions[id].dependency
                    })
                    .count();
                    if depth == 512 {
                        break;
                    }
                    source = recolored(&source, value)?;
                }
            }
            "toc-rollover" => loop {
                let count = Store::parse(&source)?.header.transaction_count;
                if count == 255 {
                    break;
                }
                assert!(count < 255);
                source = recolored(&source, count)?;
            },
            _ => {}
        }
        let apply = |io: &mut Trace,
                     bytes: &[u8],
                     recovering: bool|
         -> Result<(), Box<dyn std::error::Error>> {
            if toc {
                let color = if recovering { 0x665544 } else { 0x996633 };
                let edit = onestore::TocEdit::Color(color);
                let transaction =
                    onestore::edit_table_of_contents(bytes, &[edit])?.ok_or("No change")?;
                transaction.commit(io)?;
            } else {
                let store = Store::parse(bytes)?;
                let index = RevisionIndex::parse(&store)?;
                let doc = Document::parse(&index)?;
                let space = &doc.spaces[&sid];
                let revision = &space.revisions[&space.contexts[&ExGuid::default()]];
                let Kind::RichText { text, .. } = &revision.nodes[&oid].kind else {
                    unreachable!()
                };
                let end = u32::try_from(text.encode_utf16().count())?;
                let with = if recovering {
                    " [recovered]"
                } else {
                    " [durable café 🦀]"
                };
                let edit = typing::text(sid, oid, end..end, with);
                let transaction = typing::sealed(bytes, "Power loss", &edit)?.ok_or("No change")?;
                transaction.commit(io)?;
            }
            Ok(())
        };
        fs::write(destination.join(format!("{name}-source.bin")), &source)?;
        let before = current(&source);
        let mut trace = Trace {
            bytes: source.clone(),
            events: Vec::new(),
        };
        apply(&mut trace, &source, false)?;
        let after = current(&trace.bytes);
        assert_ne!(before, after);
        let mut durable = source.clone();
        let mut pending = Vec::new();
        let mut flushes = 0;
        for (step, event) in trace.events.iter().enumerate() {
            if let Event::Write(offset, data) = event {
                pending.push((*offset, data));
            }
            for policy in 0..6 {
                let mut image = durable.clone();
                // Unflushed writes may reach storage out of order and only in part.
                for (offset, data) in pending.iter().rev() {
                    for (i, byte) in data.iter().enumerate() {
                        let persist = match policy {
                            0 => false,
                            1 => true,
                            2 => i < data.len() / 2,
                            3 => i >= data.len() / 2,
                            4 => ((offset + i) / 512) % 2 == 0,
                            _ => (offset + i).wrapping_mul(0x9e3779b9).count_ones() % 2 == 0,
                        };
                        if persist {
                            image.resize(image.len().max(offset + i + 1), 0);
                            image[offset + i] = *byte;
                        }
                    }
                }
                let checked =
                    std::panic::catch_unwind(|| -> Result<bool, Box<dyn std::error::Error>> {
                        let observed = current(&image);
                        assert!(
                            observed == before || observed == after,
                            "{name} step {step} policy {policy}"
                        );
                        if flushes >= 3 {
                            assert_eq!(observed, after, "Acknowledged publication was lost");
                        }
                        let committed = observed == after;
                        let mut recovered = Trace {
                            bytes: image.clone(),
                            events: Vec::new(),
                        };
                        apply(&mut recovered, &image, true)?;
                        let recovered_store = Store::parse(&recovered.bytes)?;
                        let recovered_index = RevisionIndex::parse(&recovered_store)?;
                        let recovered_doc = Document::parse(&recovered_index)?;
                        let space = &recovered_doc.spaces[&sid];
                        let revision = &space.revisions[&space.contexts[&ExGuid::default()]];
                        match &revision.nodes[&oid].kind {
                            Kind::RichText { text, .. } => assert!(text.ends_with(" [recovered]")),
                            Kind::Toc { color, .. } => assert_eq!(*color, Some(0x665544)),
                            _ => unreachable!(),
                        }
                        Ok(committed)
                    });
                let committed = match checked {
                    Ok(Ok(committed)) => committed,
                    failure => {
                        fs::write(destination.join("failure.one"), &image)?;
                        fs::write(
                            destination.join("failure.json"),
                            serde_json::to_vec_pretty(
                                &serde_json::json!({"case": name, "step": step, "policy": policy, "completed_flushes": flushes}),
                            )?,
                        )?;
                        match failure {
                            Ok(Err(error)) => return Err(error),
                            Err(panic) => std::panic::resume_unwind(panic),
                            _ => unreachable!(),
                        }
                    }
                };
                let digest = format!("{:x}", md5::compute(&image));
                if matches!(event, Event::Flush)
                    && (policy == 0 || policy == 1)
                    && !saved.contains_key(&digest)
                {
                    let directory = format!("{name}-{step}-{policy}");
                    let notebook = destination.join(&directory).join("notebook");
                    fs::create_dir_all(&notebook)?;
                    if toc {
                        fs::write(notebook.join("Open Notebook.onetoc2"), &image)?;
                        fs::copy(
                            PathBuf::from(path).parent().unwrap().join("synthetic.one"),
                            notebook.join("synthetic.one"),
                        )?;
                    } else {
                        fs::write(notebook.join("synthetic.one"), &image)?;
                        let identity = Store::parse(&image)?.header.file_id;
                        fs::write(
                            notebook.join("Open Notebook.onetoc2"),
                            onestore::create_table_of_contents(
                                "Open Notebook.onetoc2",
                                &[("synthetic.one", identity)],
                            )?,
                        )?;
                    }
                    saved.insert(digest.clone(), directory);
                }
                records.push(serde_json::json!({"case":name,"step":step,"completed_flushes":flushes,"policy":policy,"committed":committed,"md5":digest}));
            }
            if matches!(event, Event::Flush) {
                for (offset, data) in pending.drain(..) {
                    durable.resize(durable.len().max(offset + data.len()), 0);
                    durable[offset..offset + data.len()].copy_from_slice(data);
                }
                flushes += 1;
                current(&durable);
            }
        }
        assert_eq!(current(&durable), after);
    }
    fs::write(
        destination.join("matrix.json"),
        serde_json::to_vec_pretty(&records)?,
    )?;
    fs::write(
        destination.join("native-cases.json"),
        serde_json::to_vec_pretty(&saved)?,
    )?;
    println!(
        "Passed {} persisted images and subsequent edits; {} native cases retained",
        records.len(),
        saved.len()
    );
    Ok(())
}
