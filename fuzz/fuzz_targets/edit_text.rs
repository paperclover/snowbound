#![no_main]
use libfuzzer_sys::fuzz_target;
use onestore::{
    CommitState, ExGuid, RevisionIndex, Store,
    document::{Document, Kind},
};
use std::sync::LazyLock;

#[path = "../../crates/onestore/tests/support/checkpoint.rs"]
mod checkpoint;
#[path = "../../crates/onestore/tests/support/disk.rs"]
mod disk;
use disk::Disk;

const SOURCE: &[u8] = include_bytes!(
    "../../corpus/native/20260905-05/snapshots/03-format-unicode/notebook/synthetic.one"
);
static CASES: LazyLock<[(Vec<u8>, ExGuid, ExGuid); 7]> = LazyLock::new(|| {
    let fixtures: [(&[u8], &str); 6] = [
        (SOURCE, "Fictitious"),
        (
            include_bytes!(
                "../../corpus/native/20260905-05/snapshots/02-text/notebook/synthetic.one"
            ),
            "Fictitious",
        ),
        (
            include_bytes!("../../corpus/m6/native-empty-link-01/notebook/synthetic.one"),
            "",
        ),
        (
            include_bytes!("../../corpus/m6/native-structure-01/notebook/synthetic.one"),
            "Native feature probes",
        ),
        (
            include_bytes!("../../corpus/m7/automatic-titles/widths-and-limits.one"),
            "Right narrow.",
        ),
        (
            include_bytes!("../../corpus/m7/automatic-titles/widths-and-limits.one"),
            "Left wide.",
        ),
    ];
    let [ordinary, legacy, empty, title, rtl_title, rtl_body] = fixtures.map(|(source, prefix)| {
        let store = Store::parse(source).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let document = Document::parse(&index).unwrap();
        for (sid, space) in &document.spaces {
            let revision = &space.revisions[&space.contexts[&ExGuid::default()]];
            for (oid, node) in &revision.nodes {
                if matches!(&node.kind, Kind::RichText { text, boilerplate: false, .. }
                    if if prefix.is_empty() { text.is_empty() } else { text.starts_with(prefix) })
                {
                    return (source.to_vec(), *sid, *oid);
                }
            }
        }
        panic!("Missing text fixture")
    });
    let checkpoint = (
        checkpoint::pending(&ordinary.0, ordinary.1, ordinary.2, 0x14001d7a),
        ordinary.1,
        ordinary.2,
    );
    [
        ordinary, legacy, empty, checkpoint, title, rtl_title, rtl_body,
    ]
});

fn characters(source: &[u8], sid: ExGuid, oid: ExGuid) -> Vec<(char, String)> {
    let store = Store::parse(source).unwrap();
    assert!(store.checksum_mismatches.is_empty());
    let index = RevisionIndex::parse(&store).unwrap();
    index.validate_current().unwrap();
    let document = Document::parse(&index).unwrap();
    let space = &document.spaces[&sid];
    let revision = &space.revisions[&space.contexts[&ExGuid::default()]];
    let node = &revision.nodes[&oid];
    if node.extra[0].iter().any(|field| field.id == 0x88001cb4) {
        let Kind::RichText { text, .. } = &node.kind else {
            panic!()
        };
        let text = text.trim_start().split('\r').next().unwrap();
        let Kind::Metadata { title, .. } = &revision.nodes[&revision.roots[&2]].kind else {
            panic!()
        };
        let pages: Vec<_> = document
            .pages()
            .unwrap()
            .into_iter()
            .filter(|(id, _)| *id == sid)
            .collect();
        let [(_, page)] = pages.as_slice() else {
            panic!()
        };
        let Kind::Page {
            alternate_title, ..
        } = &revision.nodes[page].kind
        else {
            panic!()
        };
        assert_eq!(
            title.as_deref().unwrap(),
            if text.is_empty() {
                alternate_title.as_deref().unwrap_or("")
            } else {
                text
            }
        );
        if !text.is_empty() {
            assert!(alternate_title.as_deref().unwrap_or("").is_empty());
        }
    }
    let runs = revision.text_runs(oid).unwrap();
    let mut characters: Vec<_> = runs
        .iter()
        .flat_map(|run| {
            run.text
                .chars()
                .map(move |c| (c, format!("{:?}", run.format)))
        })
        .collect();
    // The terminal marker retains insertion formatting when the final run is empty.
    characters.push(('\0', format!("{:?}", runs.last().unwrap().format)));
    characters
}

fuzz_target!(|input: &[u8]| {
    let (source, sid, oid) = &CASES[usize::from(input.last().copied().unwrap_or(0)) % CASES.len()];
    let (sid, oid) = (*sid, *oid);
    let mut persisted_source = source.clone();
    let mut caches = std::array::from_fn::<_, 12, _>(|_| source.clone());
    for step in input.chunks_exact(8).take(16) {
        let actor = usize::from(step[6]) % caches.len();
        if step[7] % 3 == 0 {
            caches[actor].clone_from(&persisted_source);
            continue;
        }
        let source = &caches[actor];
        let current = characters(&persisted_source, sid, oid);
        let before = characters(source, sid, oid);
        let a = usize::from(step[0]) % before.len();
        let b = usize::from(step[1]) % before.len();
        let start = a.min(b);
        let end = a.max(b);
        let offset = before[..start]
            .iter()
            .map(|(c, _)| c.len_utf16() as u32)
            .sum::<u32>();
        let removed = before[start..end]
            .iter()
            .map(|(c, _)| c.len_utf16() as u32)
            .sum::<u32>();
        let replacement = ["", "a", "🦀e\u{301}", "日本語", "\n", "\0"][usize::from(step[2]) % 6];
        let mut expected = before.clone();
        let format = before[start].1.clone();
        if before[start..end]
            .iter()
            .map(|(c, _)| *c)
            .collect::<String>()
            != replacement
        {
            expected.splice(start..end, replacement.chars().map(|c| (c, format.clone())));
        }
        let mut storage = Disk {
            visible: persisted_source.clone(),
            durable: persisted_source.clone(),
            operation: 0,
            fail_at: (step[3] & 1 != 0)
                .then_some(source.len().div_ceil(193) + usize::from(step[4]) + 1),
            write_limit: usize::from(step[5]) + 1,
            random: u64::from_le_bytes(step.try_into().unwrap()),
        };
        let result = onestore::commit_text(
            &mut storage,
            source,
            sid,
            oid,
            offset..offset + removed,
            replacement,
        );
        let persisted = characters(&storage.durable, sid, oid);
        if !replacement.contains(['\n', '\0']) {
            assert!(
                result
                    .as_ref()
                    .err()
                    .is_none_or(|error| error.error.kind() != std::io::ErrorKind::InvalidData),
                "Valid ordinary text edit was rejected: {result:?}"
            );
        }
        match result {
            Ok(()) => assert_eq!(persisted, expected),
            Err(error) => match error.state {
                CommitState::NotCommitted => assert_eq!(persisted, current),
                CommitState::Committed => assert_eq!(persisted, expected),
                CommitState::Unknown => assert!(persisted == current || persisted == expected),
            },
        }
        persisted_source = storage.durable;
    }
});
