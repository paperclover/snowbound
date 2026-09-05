#[path = "../src/flush.rs"]
mod flush;

use onestore::{
    ExGuid, RevisionIndex, Store,
    document::{Document, Kind},
};
use std::{
    collections::BTreeSet,
    env, fs,
    io::Write,
    time::{SystemTime, UNIX_EPOCH},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = env::args().skip(1).collect();
    if !(3..=4).contains(&args.len()) {
        return Err("Usage: random_edit FILE OUTPUT|--in-place SEED [PAGE_ID]".into());
    }
    let started = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
    let seed: u64 = args[2].parse()?;
    let mut state = seed;
    let mut next = || {
        state = state.wrapping_add(0x9e3779b97f4a7c15);
        let mut value = state;
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d049bb133111eb);
        value ^ (value >> 31)
    };
    let source = onestore::read_file(&args[0])?;
    let store = Store::parse(&source)?;
    let index = RevisionIndex::parse(&store)?;
    index.validate_current()?;
    let document = Document::parse(&index)?;
    let mut candidates = Vec::new();
    for (sid, page) in document.pages()? {
        if args.get(3).is_some_and(|id| *id != page.to_string()) {
            continue;
        }
        let space = &document.spaces[&sid];
        let revision = &space.revisions[&space.contexts[&ExGuid::default()]];
        let mut pending = vec![page];
        let mut seen = BTreeSet::new();
        while let Some(oid) = pending.pop() {
            if !seen.insert(oid) {
                continue;
            }
            let node = &revision.nodes[&oid];
            pending.extend(
                node.children
                    .iter()
                    .chain(&node.content)
                    .chain(&node.structure)
                    .copied(),
            );
            if let Kind::RichText {
                runs,
                boilerplate: false,
                ..
            } = &node.kind
            {
                for (run, resolved) in runs.iter().zip(revision.text_runs(oid)?) {
                    if [
                        resolved.format.hidden,
                        resolved.format.hyperlink,
                        resolved.format.math,
                        resolved.format.embedded_object,
                    ]
                    .contains(&Some(true))
                    {
                        continue;
                    }
                    candidates.push((sid, page, oid, run.start, resolved.text));
                }
            }
        }
    }
    for i in (1..candidates.len()).rev() {
        candidates.swap(i, (next() % (i as u64 + 1)) as usize);
    }
    let mut rejected = Vec::new();
    for (sid, page, oid, offset, text) in candidates {
        let mut boundaries = vec![0];
        for character in text.chars() {
            boundaries.push(boundaries.last().unwrap() + character.len_utf16() as u32);
        }
        let first = (next() % boundaries.len() as u64) as usize;
        let second = (next() % boundaries.len() as u64) as usize;
        let mode = next() % 3;
        let start = if mode == 0 { first } else { first.min(second) };
        let end = if mode == 0 { first } else { first.max(second) };
        let insertion = match next() % 4 {
            0 => " revised ",
            1 => " café ",
            2 => " 東京 🦀 ",
            _ => " e\u{301} ",
        };
        let replacement = if mode == 1 && start != end {
            ""
        } else {
            insertion
        };
        let end = if text
            .chars()
            .skip(start)
            .take(end - start)
            .eq(replacement.chars())
        {
            start
        } else {
            end
        };
        let range = offset + boundaries[start]..offset + boundaries[end];
        let written = match onestore::replace_text(&source, sid, oid, range.clone(), replacement) {
            Ok(written) => written,
            Err(error) => {
                rejected.push(error.to_string());
                continue;
            }
        };
        let space = &document.spaces[&sid];
        let revision = &space.revisions[&space.contexts[&ExGuid::default()]];
        let Kind::RichText { runs, .. } = &revision.nodes[&oid].kind else {
            unreachable!()
        };
        let selected = runs
            .iter()
            .rposition(|run| run.start <= range.start && range.end <= run.end)
            .unwrap();
        let actual = revision.text_runs(oid)?;
        let mut record = serde_json::json!({"seed": seed, "source": args[0], "page": page, "space": sid,
            "object": oid, "range": [range.start, range.end], "replacement": replacement,
            "run_before": actual[selected].text, "run_start": runs[selected].start, "source_md5": format!("{:x}", md5::compute(&source)),
            "started_ms": started, "rejected_candidates": rejected});
        let failed = if args[1] == "--in-place" {
            match onestore::commit_file_text(&args[0], &source, sid, oid, range, replacement) {
                Ok(()) => {
                    record["state"] = "Committed".into();
                    false
                }
                Err(error) => {
                    record["state"] = format!("{:?}", error.state).into();
                    record["error"] = error.error.to_string().into();
                    true
                }
            }
        } else {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&args[1])?;
            file.write_all(&written)?;
            flush::flush(&file)?;
            record["state"] = "Created".into();
            record["output"] = args[1].clone().into();
            false
        };
        record["finished_ms"] =
            u64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis())?.into();
        println!("{}", serde_json::to_string(&record)?);
        if failed {
            std::process::exit(2);
        }
        return Ok(());
    }
    Err(format!(
        "No supported text edit on the selected page: {}",
        rejected.join("; ")
    )
    .into())
}
