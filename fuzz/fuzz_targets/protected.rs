#![no_main]
use libfuzzer_sys::fuzz_target;
use onestore::{
    Arena, Reference, RevisionIndex, Section, Store,
    protected::{Key, Limits, UnlockedSection},
};
use std::{ops::Range, sync::LazyLock};

type Source = (Vec<u8>, String, Vec<Range<usize>>, Key);

static SOURCES: LazyLock<Vec<Source>> = LazyLock::new(|| {
    [
        (&include_bytes!("../../corpus/native-encrypted/encrypted-01/notebook/synthetic.one")[..], include_str!("../../corpus/native-encrypted/manifest.json")),
        (&include_bytes!("../../corpus/native-protected-boundaries/notebook/synthetic.one")[..], include_str!("../../corpus/native-protected-boundaries/manifest.json")),
    ].into_iter().map(|(bytes, manifest)| {
        let manifest: serde_json::Value = serde_json::from_str(manifest).unwrap();
        let store = Store::parse(bytes).unwrap();
        let mut ranges: Vec<_> = store.lists.values().flat_map(|list| &list.nodes).filter_map(|node| {
            let Reference::Data(chunk) = node.reference? else { return None; };
            let start = usize::try_from(chunk.offset).unwrap();
            let end = start + usize::try_from(chunk.length).unwrap();
            (start < end).then_some(start..end)
        }).collect();
        ranges.sort_by_key(|range| (range.start, range.end));
        ranges.dedup();
        let password = manifest["password"].as_str().unwrap().to_owned();
        let key = Key::open(bytes, &password).unwrap();
        (bytes.to_vec(), password, ranges, key)
    }).collect()
});

fuzz_target!(|data: &[u8]| {
    if data.len() < 8 {
        return;
    }
    let (source, password, ranges, key) = &SOURCES[usize::from(data[0]) % SOURCES.len()];
    let mut bytes = source.clone();
    let mut password = password.clone();
    let mode = data[1] % 3;
    if mode == 2 {
        password.push_str(&String::from_utf8_lossy(&data[8..]));
    } else {
        let range = if mode == 0 {
            0..bytes.len()
        } else {
            ranges[usize::from(u16::from_le_bytes([data[2], data[3]])) % ranges.len()].clone()
        };
        let offset = u32::from_le_bytes(data[4..8].try_into().unwrap()) as usize % range.len();
        let count = (range.len() - offset).min(data.len() - 8);
        bytes[range.start + offset..range.start + offset + count]
            .copy_from_slice(&data[8..8 + count]);
    }
    // The kept-open section decodes the same bytes under the section's key.
    if let Ok(mut section) = Section::unlock(&Arena::default(), bytes.clone(), key) {
        for (space, ..) in section.pages().unwrap_or_default() {
            let _ = section.page(space);
        }
    }
    let Ok(store) = Store::parse(&bytes) else {
        return;
    };
    let Ok(index) = RevisionIndex::parse(&store) else {
        return;
    };
    let result = UnlockedSection::open(
        &index,
        &password,
        Limits {
            kdf_rounds: 200000,
            decoded_bytes: 4 * 1024 * 1024,
            object_visits: 20000,
        },
    );
    if mode == 2 && data.len() > 8 {
        assert!(result.is_err());
    }
    if let Ok(unlocked) = result {
        let document = unlocked.document().unwrap();
        let _ = document.pages();
        for revision in document.spaces.values().flat_map(|s| s.revisions.values()) {
            for (id, node) in &revision.nodes {
                if matches!(node.kind, onestore::document::Kind::RichText { .. }) {
                    let _ = revision.text_runs(*id);
                }
            }
        }
    }
});
