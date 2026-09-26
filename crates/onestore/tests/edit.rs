#[path = "support/checkpoint.rs"]
mod checkpoint;
#[path = "support/disk.rs"]
mod disk;
#[path = "support/trace.rs"]
mod trace;

use disk::Disk;
use onestore::{
    CommitState, ExGuid, RevisionIndex, Store,
    document::{Document, Kind},
};

const SOURCE: &[u8] = include_bytes!(
    "../../../corpus/native/20260905-05/snapshots/03-format-unicode/notebook/synthetic.one"
);

fn target(source: &[u8]) -> (ExGuid, ExGuid) {
    let store = Store::parse(source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    for (sid, space) in &document.spaces {
        let revision = &space.revisions[&space.contexts[&ExGuid::default()]];
        for (oid, node) in &revision.nodes {
            if matches!(&node.kind, Kind::RichText { text, .. } if text.starts_with("Fictitious:"))
            {
                return (*sid, *oid);
            }
        }
    }
    panic!("Missing native text fixture")
}

fn text_runs(source: &[u8], sid: ExGuid, oid: ExGuid) -> serde_json::Value {
    let store = Store::parse(source).unwrap();
    assert!(store.checksum_mismatches.is_empty());
    let index = RevisionIndex::parse(&store).unwrap();
    index.validate_current().unwrap();
    let document = Document::parse(&index).unwrap();
    let space = &document.spaces[&sid];
    serde_json::to_value(
        space.revisions[&space.contexts[&ExGuid::default()]]
            .text_runs(oid)
            .unwrap(),
    )
    .unwrap()
}

fn assert_refreshed(source: &[u8], confirmed: &[u8]) {
    assert_eq!(&source[..212], &confirmed[..212]);
    assert_eq!(&source[252..], &confirmed[252..]);
    let before = Store::parse(source).unwrap().header;
    let after = Store::parse(confirmed).unwrap().header;
    assert_ne!(before.version_id, after.version_id);
    assert_ne!(before.deny_read_id, after.deny_read_id);
    assert_eq!(after.generation, before.generation + 1);
}

#[test]
fn prepared_publication_preserves_its_identity_through_every_io_failure() {
    let source =
        onestore::create_section("prepared.one", "Fictitious: café 🦀", "Fixture").unwrap();
    let (sid, oid) = target(&source);
    let source = checkpoint::pending(&source, sid, oid, 0x14001d7a);
    let edit = onestore::PreparedEdit::text(&source, sid, oid, 0..0, "Prepared 🐈 ").unwrap();
    let store = Store::parse(edit.as_bytes()).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let planned = index.spaces[&sid].labels[&(ExGuid::default(), 1)];
    let persisted: ExGuid =
        serde_json::from_str(&serde_json::to_string(&planned).unwrap()).unwrap();
    let before_store = Store::parse(&source).unwrap();
    let before_index = RevisionIndex::parse(&before_store).unwrap();
    assert!(!before_index.spaces[&sid].revisions.contains_key(&persisted));
    let before = text_runs(&source, sid, oid);
    let after = text_runs(edit.as_bytes(), sid, oid);
    for write_limit in [17, 4096] {
        let disk = |fail_at| Disk {
            visible: source.clone(),
            durable: source.clone(),
            operation: 0,
            fail_at,
            write_limit,
            random: 911,
        };
        let mut success = disk(None);
        edit.commit(&mut success).unwrap();
        assert_eq!(success.durable, edit.as_bytes());
        let operations = success.operation;
        let error = edit.commit(&mut success).unwrap_err();
        assert_eq!(error.state, CommitState::NotCommitted);
        assert_eq!(error.error.kind(), std::io::ErrorKind::ResourceBusy);
        assert_eq!(success.durable, edit.as_bytes());
        for at in 1..=operations {
            let mut interrupted = disk(Some(at));
            let error = edit.commit(&mut interrupted).unwrap_err();
            let observed = text_runs(&interrupted.durable, sid, oid);
            let store = Store::parse(&interrupted.durable).unwrap();
            let index = RevisionIndex::parse(&store).unwrap();
            let present = index.spaces[&sid].revisions.contains_key(&persisted);
            assert_eq!(observed, if present { &after } else { &before }.clone());
            match error.state {
                CommitState::NotCommitted => assert!(!present),
                CommitState::Committed => assert!(present),
                CommitState::Unknown => {}
            }
            if present {
                let snapshot = interrupted.durable.clone();
                interrupted.visible.clone_from(&snapshot);
                interrupted.fail_at = None;
                interrupted.write_limit = 17;
                onestore::confirm_snapshot(&mut interrupted, &snapshot).unwrap();
                assert_refreshed(&snapshot, &interrupted.durable);
            }
        }
    }
}

#[test]
fn snapshot_confirmation_needs_no_surviving_edit_target() {
    let mut disk = Disk {
        visible: SOURCE.to_vec(),
        durable: Vec::new(),
        operation: 0,
        fail_at: None,
        write_limit: 17,
        random: 911,
    };
    assert!(
        onestore::replace_text(SOURCE, ExGuid::default(), ExGuid::default(), 0..0, "").is_err()
    );
    onestore::confirm_snapshot(&mut disk, SOURCE).unwrap();
    assert_refreshed(SOURCE, &disk.durable);
    let flush = disk.operation;
    for (failure, state) in [
        (1, CommitState::NotCommitted),
        (flush, CommitState::Unknown),
    ] {
        disk.visible = SOURCE.to_vec();
        disk.durable.clear();
        disk.operation = 0;
        disk.fail_at = Some(failure);
        assert_eq!(
            onestore::confirm_snapshot(&mut disk, SOURCE)
                .unwrap_err()
                .state,
            state
        );
    }
}

#[test]
fn confirmation_notifies_cached_readers_after_interrupted_version_publication() {
    let source =
        onestore::create_section("confirmation.one", "Fictitious: before", "Fixture").unwrap();
    let (sid, oid) = target(&source);
    let mut snapshot = onestore::replace_text(&source, sid, oid, 0..0, "Recovered ").unwrap();
    snapshot[212..252].copy_from_slice(&source[212..252]);
    let expected = text_runs(&snapshot, sid, oid);
    assert_ne!(expected, text_runs(&source, sid, oid));
    for write_limit in [1, 17, 40] {
        let disk = |fail_at| Disk {
            visible: snapshot.clone(),
            durable: snapshot.clone(),
            operation: 0,
            fail_at,
            write_limit,
            random: 911,
        };
        let mut success = disk(None);
        onestore::confirm_snapshot(&mut success, &snapshot).unwrap();
        assert_refreshed(&snapshot, &success.durable);
        for failure in 1..=success.operation {
            let mut interrupted = disk(Some(failure));
            let error = onestore::confirm_snapshot(&mut interrupted, &snapshot).unwrap_err();
            assert_ne!(error.state, CommitState::Committed);
            assert_eq!(text_runs(&interrupted.durable, sid, oid), expected);
            assert_eq!(&interrupted.durable[..212], &snapshot[..212]);
            assert_eq!(&interrupted.durable[252..], &snapshot[252..]);
        }
    }
}

#[test]
fn confirming_visible_text_requires_flush_without_another_revision() {
    let (sid, oid) = target(SOURCE);
    let visible = onestore::replace_text(SOURCE, sid, oid, 0..0, "Recovered ").unwrap();
    let mut disk = Disk {
        visible: visible.clone(),
        durable: SOURCE.to_vec(),
        operation: 0,
        fail_at: None,
        write_limit: 0,
        random: 1,
    };
    onestore::commit_text(&mut disk, &visible, sid, oid, 0..0, "").unwrap();
    assert_eq!(disk.visible, visible);
    assert_eq!(disk.durable, visible);
    let flush = disk.operation;
    for (failure, state) in [
        (1, CommitState::NotCommitted),
        (flush, CommitState::Unknown),
    ] {
        disk.durable = SOURCE.to_vec();
        disk.operation = 0;
        disk.fail_at = Some(failure);
        let error = onestore::commit_text(&mut disk, &visible, sid, oid, 0..0, "").unwrap_err();
        assert_eq!(error.state, state);
        assert_eq!(disk.visible, visible);
    }
}

#[test]
fn confirmation_rejects_a_changed_header_or_length_before_any_write() {
    let (sid, oid) = target(SOURCE);
    let mut source = SOURCE.to_vec();
    source.resize(3 * 1024 * 1024 + 131, 0);
    let mut disk = trace::Trace {
        bytes: source.clone(),
        events: Vec::new(),
    };
    onestore::commit_text(&mut disk, &source, sid, oid, 0..0, "").unwrap();
    assert_eq!(disk.events.len(), 1);
    if let trace::Event::Write(offset, bytes) = &disk.events[0] {
        panic!("Confirmation wrote {} bytes at {offset}", bytes.len());
    }
    for changed in [96, 128, 212, 1023] {
        disk.bytes.clone_from(&source);
        disk.bytes[changed] ^= 1;
        disk.events.clear();
        let error = onestore::commit_text(&mut disk, &source, sid, oid, 0..0, "").unwrap_err();
        assert_eq!(error.state, CommitState::NotCommitted);
        assert_eq!(error.error.kind(), std::io::ErrorKind::ResourceBusy);
        assert!(disk.events.is_empty());
    }
    // Committed content changes only with the header, so the body is never compared.
    for changed in [1024, 1048576, source.len() - 1] {
        disk.bytes.clone_from(&source);
        disk.bytes[changed] ^= 1;
        disk.events.clear();
        onestore::commit_text(&mut disk, &source, sid, oid, 0..0, "").unwrap();
        assert!(matches!(disk.events[..], [trace::Event::Flush]));
    }
    for length in [source.len() - 1, source.len() + 1] {
        disk.bytes.clone_from(&source);
        disk.bytes.resize(length, 0);
        disk.events.clear();
        let error = onestore::commit_text(&mut disk, &source, sid, oid, 0..0, "").unwrap_err();
        assert_eq!(error.state, CommitState::NotCommitted);
        assert!(disk.events.is_empty());
    }
}

fn assert_other_objects_preserved(
    before: &onestore::ResolvedRevision<'_>,
    after: &onestore::ResolvedRevision<'_>,
    edited: ExGuid,
) {
    use onestore::{ObjectData, PropertySets};
    let ObjectData::Properties(bytes) = after.objects[&edited].data else {
        panic!()
    };
    let target = PropertySets::parse(bytes).unwrap();
    let modified = &target.sets[0]
        .iter()
        .find(|field| field.id == 0x14001d7a)
        .unwrap()
        .value;
    let reachable = before.reachable().unwrap();
    assert_eq!(before.objects.len(), after.objects.len());
    for (id, object) in &before.objects {
        if *id == edited {
            continue;
        }
        let current = &after.objects[id];
        assert_eq!(object.jcid, current.jcid);
        // A rewritten object keeps the table entries it names.
        assert!(
            current
                .global_ids
                .iter()
                .all(|(entry, guid)| object.global_ids.get(entry) == Some(guid))
        );
        let mut pending = vec![*id];
        let mut visited = std::collections::BTreeSet::new();
        while let Some(next) = pending.pop() {
            if !visited.insert(next) {
                continue;
            }
            pending.extend(before.objects[&next].references().unwrap().objects);
        }
        let ancestor = reachable.contains(id) && visited.contains(&edited);
        let mut allowed = Vec::new();
        if ancestor {
            allowed.push(0x14001d7a);
        }
        if ancestor && object.jcid == 0x6000b {
            allowed.push(0x1c001d3c);
        }
        if before.roots.get(&2) == Some(id) {
            allowed.push(0x1c001cf3);
        }
        if allowed.is_empty() {
            assert_eq!(object.data, current.data);
            continue;
        }
        let [old, new] = [object.data, current.data].map(|data| {
            let ObjectData::Properties(bytes) = data else {
                panic!()
            };
            PropertySets::parse(bytes).unwrap()
        });
        if ancestor {
            let old_time = old.sets[0].iter().find(|field| field.id == 0x14001d7a);
            let new_time = new.sets[0].iter().find(|field| field.id == 0x14001d7a);
            assert_eq!(old_time.is_some(), new_time.is_some());
            if let Some(time) = new_time {
                assert_eq!(&time.value, modified);
            }
        }
        let [old, new] = [old, new].map(|mut fields| {
            fields.sets[0].retain(|field| !allowed.contains(&field.id));
            fields.sets
        });
        assert_eq!(old, new);
    }
}

#[test]
fn text_splices_preserve_formats_objects_and_history_in_one_transaction() {
    let (sid, oid) = target(SOURCE);
    let old = Store::parse(SOURCE).unwrap();
    let old_index = RevisionIndex::parse(&old).unwrap();
    let before = text_runs(SOURCE, sid, oid);
    for replacement in ["", "🦀", "A longer replacement with 日本語 and e\u{301}"] {
        let edited = onestore::replace_text(SOURCE, sid, oid, 0..10, replacement).unwrap();
        let store = Store::parse(&edited).unwrap();
        assert_eq!(
            store.header.transaction_count,
            old.header.transaction_count + 1
        );
        let index = RevisionIndex::parse(&store).unwrap();
        for (space_id, space) in &old_index.spaces {
            for rid in space.revisions.keys() {
                let previous = old_index.resolve(*space_id, *rid).unwrap();
                let preserved = index.resolve(*space_id, *rid).unwrap();
                assert_eq!(previous.roots, preserved.roots);
                for (id, object) in previous.objects {
                    assert_eq!(object.data, preserved.objects[&id].data);
                }
            }
            let previous = old_index
                .resolve(*space_id, space.labels[&(ExGuid::default(), 1)])
                .unwrap();
            let current = index
                .resolve(
                    *space_id,
                    index.spaces[space_id].labels[&(ExGuid::default(), 1)],
                )
                .unwrap();
            assert_eq!(previous.objects.len(), current.objects.len());
            if *space_id == sid {
                assert_other_objects_preserved(&previous, &current, oid);
            } else {
                for (id, object) in previous.objects {
                    assert_eq!(object.data, current.objects[&id].data);
                }
            }
        }
        let mut expected = before.clone();
        expected[0]["text"] = format!(
            "{replacement}{}",
            &before[0]["text"].as_str().unwrap()[10..]
        )
        .into();
        assert_eq!(text_runs(&edited, sid, oid), expected);
    }
}

#[test]
fn invalid_text_edits_never_touch_storage() {
    let (sid, oid) = target(SOURCE);
    let source = onestore::replace_text(SOURCE, sid, oid, 0..0, "🦀").unwrap();
    for (range, replacement) in [
        (1..1, "x"),
        (0..999, "x"),
        (0..0, "\n"),
        (0..0, "\0"),
        (0..0, "\u{fffc}"),
    ] {
        let mut disk = Disk {
            visible: source.clone(),
            durable: source.clone(),
            operation: 0,
            fail_at: None,
            write_limit: 1024,
            random: 1,
        };
        let error =
            onestore::commit_text(&mut disk, &source, sid, oid, range, replacement).unwrap_err();
        assert_eq!(error.state, CommitState::NotCommitted);
        assert_eq!(disk.operation, 0);
        assert_eq!(disk.durable, source);
    }
}

#[test]
fn cross_run_splices_match_a_character_model_and_preserve_history() {
    let (sid, oid) = target(SOURCE);
    let characters = |source: &[u8]| {
        let runs = text_runs(source, sid, oid);
        let runs = runs.as_array().unwrap();
        let mut result = Vec::new();
        for run in runs {
            result.extend(
                run["text"]
                    .as_str()
                    .unwrap()
                    .chars()
                    .map(|c| (c, run["format"].clone())),
            );
        }
        result.push(('\0', runs.last().unwrap()["format"].clone()));
        result
    };
    let before = characters(SOURCE);
    let offsets: Vec<_> = std::iter::once(0)
        .chain(before[..before.len() - 1].iter().scan(0, |at, (c, _)| {
            *at += c.len_utf16() as u32;
            Some(*at)
        }))
        .collect();
    let store = Store::parse(SOURCE).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    for a in 0..before.len() {
        for b in a..before.len() {
            let replacement = ["", "🦀e\u{301}", "日本語"][b % 3];
            let mut expected = before.clone();
            expected.splice(a..b, replacement.chars().map(|c| (c, before[a].1.clone())));
            let edited =
                onestore::replace_text(SOURCE, sid, oid, offsets[a]..offsets[b], replacement)
                    .unwrap();
            assert_eq!(characters(&edited), expected, "characters {a}..{b}");
            let current_store = Store::parse(&edited).unwrap();
            assert_eq!(
                current_store.header.transaction_count,
                store.header.transaction_count
                    + usize::from(a != b || !replacement.is_empty()) as u32
            );
            let current = RevisionIndex::parse(&current_store).unwrap();
            for (space, history) in &index.spaces {
                for revision in history.revisions.keys() {
                    let old = index.resolve(*space, *revision).unwrap();
                    let retained = current.resolve(*space, *revision).unwrap();
                    assert_eq!(old.roots, retained.roots);
                    for (id, object) in old.objects {
                        assert_eq!(object.data, retained.objects[&id].data);
                    }
                }
            }
        }
    }
    let cleared =
        onestore::replace_text(SOURCE, sid, oid, 0..*offsets.last().unwrap(), "").unwrap();
    let typed = onestore::replace_text(&cleared, sid, oid, 0..0, "new").unwrap();
    let format = &before.last().unwrap().1;
    assert_eq!(
        characters(&typed),
        "new\0"
            .chars()
            .map(|c| (c, format.clone()))
            .collect::<Vec<_>>()
    );
    let text: String = before[..before.len() - 1].iter().map(|(c, _)| *c).collect();
    assert_eq!(
        onestore::replace_text(SOURCE, sid, oid, 0..*offsets.last().unwrap(), &text).unwrap(),
        SOURCE
    );
}

#[test]
#[ignore = "exports cross-run edits for independent native validation"]
fn export_native_cross_run_edits() {
    use std::{fs, path::PathBuf};
    let output = PathBuf::from(std::env::var_os("ONESTORE_CROSS_RUN_OUTPUT").unwrap());
    assert!(output.is_absolute());
    fs::create_dir(&output).unwrap();
    let mut manifest = Vec::new();
    for (name, fixture, range, replacement, retype) in [
        ("partial", "native-paragraph", 2..16, "中🦀", false),
        ("table", "native-cell", 0..24, "Across 🐈 ", false),
        ("clear", "native-paragraph", 0..u32::MAX, "", false),
        (
            "replace",
            "native-paragraph",
            0..u32::MAX,
            "Replacement e\u{301}🦀",
            false,
        ),
        ("retype", "native-paragraph", 0..u32::MAX, "", true),
        ("boundary", "native-paragraph", 4..8, "Middle", false),
    ] {
        let input = format!("../../corpus/formatted-insertion/{fixture}/candidate/synthetic.one");
        let source = fs::read(&input).unwrap();
        let store = Store::parse(&source).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let document = Document::parse(&index).unwrap();
        let (sid, oid, length) = document
            .spaces
            .iter()
            .find_map(|(sid, space)| {
                let view = &space.revisions[&space.contexts[&ExGuid::default()]];
                view.nodes.iter().find_map(|(oid, node)| {
                    let Kind::RichText { text, .. } = &node.kind else {
                        return None;
                    };
                    text.starts_with("Bold ")
                        .then(|| (*sid, *oid, text.encode_utf16().count() as u32))
                })
            })
            .unwrap();
        let range = range.start..range.end.min(length);
        let mut edited =
            onestore::replace_text(&source, sid, oid, range.clone(), replacement).unwrap();
        if retype {
            edited = onestore::replace_text(&edited, sid, oid, 0..0, "Retyped 🦀").unwrap();
        }
        let candidate = output.join(name).join("candidate");
        fs::create_dir_all(&candidate).unwrap();
        fs::write(candidate.join("synthetic.one"), &edited).unwrap();
        manifest.push(serde_json::json!({"name":name,"source":input,"space":sid,"object":oid,"range":range,"replacement":replacement,"retype":retype}));
    }
    fs::write(
        output.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();
}

#[test]
fn insertion_at_a_style_boundary_uses_the_following_style() {
    let (sid, oid) = target(SOURCE);
    let before = text_runs(SOURCE, sid, oid);
    let edited = onestore::replace_text(SOURCE, sid, oid, 18..18, "🦀").unwrap();
    let mut expected = before.clone();
    expected[1]["text"] = format!("🦀{}", before[1]["text"].as_str().unwrap()).into();
    assert_eq!(text_runs(&edited, sid, oid), expected);
}

#[test]
fn automatic_titles_follow_native_line_and_utf16_limits() {
    for (text, expected) in [
        (
            "  first\tline \rsecond".to_string(),
            "first\tline".to_string(),
        ),
        ("A".repeat(500), "A".repeat(255)),
        (
            format!("{}🦀after", "A".repeat(254)),
            format!("{}🦀", "A".repeat(254)),
        ),
        (" \t  ".to_string(), "".to_string()),
    ] {
        let source = onestore::create_section("title.one", &text, "Title fixture").unwrap();
        let check = |bytes: &[u8], expected: &str| {
            let store = Store::parse(bytes).unwrap();
            let index = RevisionIndex::parse(&store).unwrap();
            let document = Document::parse(&index).unwrap();
            let (sid, page) = document.pages().unwrap()[0];
            let space = &document.spaces[&sid];
            let revision = &space.revisions[&space.contexts[&ExGuid::default()]];
            let Kind::Metadata { title, .. } = &revision.nodes[&revision.roots[&2]].kind else {
                panic!()
            };
            let Kind::Page {
                alternate_title, ..
            } = &revision.nodes[&page].kind
            else {
                panic!()
            };
            assert_eq!(title.as_deref(), Some(expected));
            assert_eq!(alternate_title.as_deref(), Some(expected));
            let (oid, _) = revision
                .nodes
                .iter()
                .find(|(_, n)| matches!(n.kind, Kind::RichText { .. }))
                .unwrap();
            (sid, *oid)
        };
        let (sid, oid) = check(&source, &expected);
        let cleared =
            onestore::replace_text(&source, sid, oid, 0..text.encode_utf16().count() as u32, "")
                .unwrap();
        check(&cleared, "");
        let written = onestore::replace_text(&cleared, sid, oid, 0..0, "  new 🦀 name  ").unwrap();
        check(&written, "new 🦀 name");
    }
}

#[test]
fn automatic_titles_follow_native_rtl_and_attachment_order() {
    let mut checked = 0;
    for path in [
        "../../corpus/m7/automatic-titles/rtl-and-attachments.one",
        "../../corpus/m7/automatic-titles/widths-and-limits.one",
    ] {
        let source = std::fs::read(path).unwrap();
        let store = Store::parse(&source).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let document = Document::parse(&index).unwrap();
        for (sid, page) in document.pages().unwrap() {
            let space = &document.spaces[&sid];
            let rid = space.contexts[&ExGuid::default()];
            let revision = &space.revisions[&rid];
            let Kind::Page { rtl, .. } = &revision.nodes[&page].kind else {
                panic!()
            };
            if *rtl != Some(true)
                && !revision
                    .nodes
                    .values()
                    .any(|n| matches!(n.kind, Kind::Attachment { .. }))
            {
                continue;
            }
            let Kind::Metadata {
                title: Some(title), ..
            } = &revision.nodes[&revision.roots[&2]].kind
            else {
                panic!()
            };
            let original = index.resolve(sid, rid).unwrap();
            for (oid, node) in &revision.nodes {
                let Kind::RichText {
                    text,
                    boilerplate: false,
                    ..
                } = &node.kind
                else {
                    continue;
                };
                if node.extra[0].iter().any(|field| field.id == 0x88001cb4) {
                    continue;
                }
                let changed = onestore::replace_text(&source, sid, *oid, 0..0, "Edited ").unwrap();
                let store = Store::parse(&changed).unwrap();
                let current = RevisionIndex::parse(&store).unwrap();
                let after = Document::parse(&current).unwrap();
                let space = &after.spaces[&sid];
                let revision = &space.revisions[&space.contexts[&ExGuid::default()]];
                let Kind::Metadata {
                    title: Some(actual),
                    ..
                } = &revision.nodes[&revision.roots[&2]].kind
                else {
                    panic!()
                };
                let expected = if text == title {
                    format!("Edited {title}")
                } else {
                    title.clone()
                };
                assert_eq!(actual, &expected);
                let active = current
                    .resolve(sid, current.spaces[&sid].labels[&(ExGuid::default(), 1)])
                    .unwrap();
                assert_other_objects_preserved(&original, &active, *oid);
                checked += 1;
            }
        }
    }
    assert_eq!(checked, 10);
}

#[test]
fn an_emptied_final_run_retains_its_insertion_style() {
    let (sid, oid) = target(SOURCE);
    let before = text_runs(SOURCE, sid, oid);
    let erased = onestore::replace_text(SOURCE, sid, oid, 22..27, "").unwrap();
    let edited = onestore::replace_text(&erased, sid, oid, 22..22, "a").unwrap();
    let mut expected = before.clone();
    expected[3]["text"] = "a".into();
    assert_eq!(text_runs(&edited, sid, oid), expected);
}

#[test]
fn title_text_and_navigation_caches_publish_together() {
    let source =
        std::fs::read("../../corpus/m6/native-structure-01/notebook/synthetic.one").unwrap();
    let store = Store::parse(&source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let (sid, page) = document
        .pages()
        .unwrap()
        .into_iter()
        .find(|(sid, _)| {
            let space = &document.spaces[sid];
            space.revisions[&space.contexts[&ExGuid::default()]]
                .nodes
                .values()
                .any(|node| {
                    matches!(node.kind, Kind::RichText { .. })
                        && node.extra[0].iter().any(|field| field.id == 0x88001cb4)
                })
        })
        .unwrap();
    let rid = document.spaces[&sid].contexts[&ExGuid::default()];
    let revision = &document.spaces[&sid].revisions[&rid];
    let metadata = revision.roots[&2];
    let (oid, node) = revision
        .nodes
        .iter()
        .find(|(_, node)| {
            matches!(node.kind, Kind::RichText { .. })
                && node.extra[0].iter().any(|field| field.id == 0x88001cb4)
        })
        .unwrap();
    let Kind::RichText { text, runs, .. } = &node.kind else {
        panic!()
    };
    assert_eq!(runs.len(), 1);
    let end = text.encode_utf16().count() as u32;
    let state = |bytes: &[u8]| {
        let store = Store::parse(bytes).unwrap();
        assert!(store.checksum_mismatches.is_empty());
        let index = RevisionIndex::parse(&store).unwrap();
        index.validate_current().unwrap();
        let document = Document::parse(&index).unwrap();
        let space = &document.spaces[&sid];
        let current = &space.revisions[&space.contexts[&ExGuid::default()]];
        serde_json::json!([
            &current.nodes[oid].kind,
            &current.nodes[&metadata].kind,
            &current.nodes[&page].kind,
            current
                .nodes
                .iter()
                .map(|(id, node)| (*id, node.modified))
                .collect::<std::collections::BTreeMap<_, _>>(),
        ])
    };
    let checkpoint = checkpoint::pending(&source, sid, *oid, 0x14001d7a);
    for (source, write_limit) in [(source.as_slice(), 17), (checkpoint.as_slice(), 257)] {
        let before = state(source);
        for replacement in ["Renamed 🦀 日本語", ""] {
            let edit =
                onestore::PreparedEdit::text(source, sid, *oid, 0..end, replacement).unwrap();
            let disk = |fail_at| Disk {
                visible: source.to_vec(),
                durable: source.to_vec(),
                operation: 0,
                fail_at,
                write_limit,
                random: 42,
            };
            let mut success = disk(None);
            edit.commit(&mut success).unwrap();
            let after = state(&success.durable);
            assert_eq!(after[0]["text"], replacement);
            assert_eq!(
                after[1]["title"],
                if replacement.is_empty() {
                    "Black highlight, automatic text"
                } else {
                    replacement
                }
            );
            assert_eq!(
                after[2]["alternate_title"],
                if replacement.is_empty() {
                    serde_json::json!("Black highlight, automatic text")
                } else {
                    serde_json::json!("")
                }
            );
            let written = Store::parse(&success.durable).unwrap();
            assert_eq!(
                written.header.transaction_count,
                Store::parse(source).unwrap().header.transaction_count + 1
            );
            let updated = RevisionIndex::parse(&written).unwrap();
            let old = index.resolve(sid, rid).unwrap();
            let current = updated
                .resolve(sid, updated.spaces[&sid].labels[&(ExGuid::default(), 1)])
                .unwrap();
            for (id, object) in &old.objects {
                assert_eq!(
                    object.data,
                    updated.resolve(sid, rid).unwrap().objects[id].data
                );
            }
            assert_other_objects_preserved(&old, &current, *oid);
            for at in source.len().div_ceil(193)..=success.operation {
                let mut interrupted = disk(Some(at));
                let error = edit.commit(&mut interrupted).unwrap_err();
                let observed = state(&interrupted.durable);
                match error.state {
                    CommitState::NotCommitted => assert_eq!(observed, before),
                    CommitState::Committed => assert_eq!(observed, after),
                    CommitState::Unknown => assert!(observed == before || observed == after),
                }
            }
        }
    }
}

#[test]
fn native_conflict_pages_are_readable_but_not_random_edit_targets() {
    let source =
        std::fs::read("../../corpus/m6/live-collaboration-15/vm-restarted/notebook/synthetic.one")
            .unwrap();
    let store = Store::parse(&source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let pages = document.pages().unwrap();
    assert_eq!(pages.len(), 1);
    let main = &document.spaces[&pages[0].0];
    let main = &main.revisions[&main.contexts[&ExGuid::default()]];
    let conflict_spaces = &main.nodes[&main.roots[&1]].spaces;
    assert_eq!(conflict_spaces.len(), 1);
    let mut conflicts = 0;
    for sid in conflict_spaces {
        assert!(pages.iter().all(|(page_space, _)| page_space != sid));
        let space = &document.spaces[sid];
        let revision = &space.revisions[&space.contexts[&ExGuid::default()]];
        for (oid, node) in &revision.nodes {
            if !matches!(node.kind, Kind::RichText { .. }) {
                continue;
            }
            assert!(onestore::replace_text(&source, *sid, *oid, 0..0, "edit").is_err());
            conflicts += 1;
        }
    }
    assert!(conflicts > 0);
}

#[test]
fn interrupted_text_commits_never_publish_mismatched_run_boundaries() {
    let (sid, oid) = target(SOURCE);
    let checkpoint = checkpoint::pending(SOURCE, sid, oid, 0x14001d7a);
    for source in [SOURCE, checkpoint.as_slice()] {
        let make_disk = |fail_at, random| Disk {
            visible: source.to_vec(),
            durable: source.to_vec(),
            operation: 0,
            fail_at,
            write_limit: usize::MAX,
            random,
        };
        let before = text_runs(source, sid, oid);
        let mut success = make_disk(None, 1);
        onestore::commit_text(&mut success, source, sid, oid, 0..23, "🐈 mixed edit").unwrap();
        let after = text_runs(&success.durable, sid, oid);
        assert_ne!(before, after);
        for at in source.len().div_ceil(193)..=success.operation {
            for seed in [1, 42] {
                let mut disk = make_disk(Some(at), seed);
                let error =
                    onestore::commit_text(&mut disk, source, sid, oid, 0..23, "🐈 mixed edit")
                        .unwrap_err();
                let observed = text_runs(&disk.durable, sid, oid);
                match error.state {
                    CommitState::NotCommitted => assert_eq!(observed, before),
                    CommitState::Committed => assert_eq!(observed, after),
                    CommitState::Unknown => assert!(observed == before || observed == after),
                }
            }
        }
    }
}

#[test]
fn empty_and_legacy_native_text_gain_unicode_without_losing_existing_properties() {
    use onestore::{ObjectData, PropertySets, Value};
    for (path, original) in [
        (
            "../../corpus/m6/native-empty-link-01/notebook/synthetic.one",
            "",
        ),
        (
            "../../corpus/native/20260905-05/snapshots/02-text/notebook/synthetic.one",
            "Fictitious plain text.",
        ),
    ] {
        let source = std::fs::read(path).unwrap();
        let store = Store::parse(&source).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let document = Document::parse(&index).unwrap();
        let mut targets = Vec::new();
        for (sid, space) in &document.spaces {
            let revision = &space.revisions[&space.contexts[&ExGuid::default()]];
            for (oid, node) in &revision.nodes {
                if matches!(&node.kind, Kind::RichText { text, boilerplate: false, .. } if text == original)
                {
                    targets.push((*sid, *oid));
                }
            }
        }
        assert_eq!(targets.len(), 1);
        let (sid, oid) = targets[0];
        let old_rid = index.spaces[&sid].labels[&(ExGuid::default(), 1)];
        let old = index.resolve(sid, old_rid).unwrap();
        let ObjectData::Properties(blob) = old.objects[&oid].data else {
            panic!()
        };
        let properties = PropertySets::parse(blob).unwrap();
        assert!(!properties.sets[0].iter().any(|p| p.id == 0x1c001c22));
        let expected = format!("🦀 café {original}");
        let written = onestore::replace_text(&source, sid, oid, 0..0, "🦀 café ").unwrap();
        let after_store = Store::parse(&written).unwrap();
        assert!(after_store.checksum_mismatches.is_empty());
        let after = RevisionIndex::parse(&after_store).unwrap();
        after.validate_current().unwrap();
        let current = after
            .resolve(sid, after.spaces[&sid].labels[&(ExGuid::default(), 1)])
            .unwrap();
        let ObjectData::Properties(blob) = current.objects[&oid].data else {
            panic!()
        };
        let updated = PropertySets::parse(blob).unwrap();
        assert_eq!(updated.sets[0].len(), properties.sets[0].len() + 1);
        for property in &properties.sets[0] {
            if property.id != 0x14001d7a {
                assert!(updated.sets[0].iter().any(|p| p == property));
            }
        }
        let encoded: Vec<_> = expected
            .encode_utf16()
            .chain([0])
            .flat_map(u16::to_le_bytes)
            .collect();
        assert!(
            updated.sets[0]
                .iter()
                .any(|p| p.id == 0x1c001c22 && p.value == Value::Bytes(&encoded))
        );
        assert_eq!(
            after.resolve(sid, old_rid).unwrap().objects[&oid].data,
            old.objects[&oid].data
        );
        assert_other_objects_preserved(&old, &current, oid);
        let before = text_runs(&source, sid, oid);
        let observed = text_runs(&written, sid, oid);
        let mut intended = before.clone();
        intended[0]["text"] = expected.into();
        assert_eq!(observed, intended);
        let disk = |fail_at, random| Disk {
            visible: source.clone(),
            durable: source.clone(),
            operation: 0,
            fail_at,
            write_limit: 17,
            random,
        };
        let mut success = disk(None, 1);
        onestore::commit_text(&mut success, &source, sid, oid, 0..0, "🦀 café ").unwrap();
        for at in source.len().div_ceil(193)..=success.operation {
            for seed in [1, 42] {
                let mut interrupted = disk(Some(at), seed);
                let error =
                    onestore::commit_text(&mut interrupted, &source, sid, oid, 0..0, "🦀 café ")
                        .unwrap_err();
                let actual = text_runs(&interrupted.durable, sid, oid);
                match error.state {
                    CommitState::NotCommitted => assert_eq!(actual, before),
                    CommitState::Committed => assert_eq!(actual, observed),
                    CommitState::Unknown => assert!(actual == before || actual == observed),
                }
            }
        }
    }
}
