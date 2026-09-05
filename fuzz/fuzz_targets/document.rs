#![no_main]
use libfuzzer_sys::fuzz_target;
use onestore::{ObjectData, Reference, RevisionIndex, Store, document::Document};
use std::sync::LazyLock;

static SOURCES: LazyLock<Vec<Vec<u8>>> = LazyLock::new(|| {
    let mut sources = vec![
        include_bytes!("../../corpus/append/round-01/complex/notebook/synthetic.one").to_vec(),
        include_bytes!("../../corpus/m6/native-probes-01/notebook/synthetic.one").to_vec(),
        include_bytes!("../../corpus/m6/native-structure-01/notebook/synthetic.one").to_vec(),
        include_bytes!("../../corpus/m6/native-features-01/notebook/Features.one").to_vec(),
        include_bytes!("../../corpus/m6/native-template-controls-01/notebook/synthetic.one")
            .to_vec(),
        include_bytes!("../../corpus/m6/native-break-controls-02/notebook/synthetic.one").to_vec(),
        include_bytes!("../../corpus/m6/native-empty-link-01/notebook/synthetic.one").to_vec(),
        include_bytes!("../../corpus/m6/native-origin-controls-01/input/notebook/synthetic.one")
            .to_vec(),
        include_bytes!("../../corpus/m6/native-page-direction-03/notebook/synthetic.one").to_vec(),
        include_bytes!("../../corpus/m6/native-math-01/notebook/synthetic.one").to_vec(),
        include_bytes!("../../corpus/collaboration/round-01/offline/notebook/synthetic.one")
            .to_vec(),
    ];
    if let Some(paths) = std::env::var_os("ONESTORE_DOCUMENT_SEEDS") {
        sources.extend(std::env::split_paths(&paths).map(|path| std::fs::read(path).unwrap()));
    }
    sources
});
static RANGES: LazyLock<Vec<(usize, std::ops::Range<usize>, Vec<usize>)>> = LazyLock::new(|| {
    let mut ranges = Vec::new();
    for (source_index, source) in SOURCES.iter().enumerate() {
        let store = Store::parse(source).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let document = Document::parse(&index).unwrap();
        for (sid, space) in &document.spaces {
            for rid in space.revisions.keys() {
                let revision = index.resolve(*sid, *rid).unwrap();
                for oid in revision.reachable().unwrap() {
                    let object = &revision.objects[&oid];
                    if let ObjectData::Properties(bytes) = object.data {
                        let start = bytes.as_ptr().addr() - source.as_ptr().addr();
                        let hashes = store
                            .lists
                            .values()
                            .flat_map(|list| &list.nodes)
                            .filter_map(|node| {
                                if matches!(node.id, 0xc2 | 0xc4 | 0xc5)
                                    && let Some(Reference::Data(chunk)) = node.reference
                                    && chunk.offset == start as u64
                                    && chunk.length == bytes.len() as u64
                                {
                                    Some(
                                        node.payload.as_ptr().addr() - source.as_ptr().addr()
                                            + node.payload.len()
                                            - 16,
                                    )
                                } else {
                                    None
                                }
                            })
                            .collect();
                        if !ranges.iter().any(|(s, r, _)| {
                            *s == source_index && *r == (start..start + bytes.len())
                        }) {
                            ranges.push((source_index, start..start + bytes.len(), hashes));
                        }
                    }
                }
            }
        }
    }
    ranges
});

fuzz_target!(|data: &[u8]| {
    if data.len() < 4 {
        return;
    }
    let (source, range, hashes) =
        &RANGES[usize::from(u16::from_le_bytes([data[0], data[1]])) % RANGES.len()];
    let offset = usize::from(u16::from_le_bytes([data[2], data[3]])) % range.len();
    let mut bytes = SOURCES[*source].to_vec();
    let size = (range.len() - offset).min(data.len() - 4);
    bytes[range.start + offset..range.start + offset + size].copy_from_slice(&data[4..4 + size]);
    let digest = md5::compute(&bytes[range.clone()]).0;
    for offset in hashes {
        bytes[*offset..*offset + 16].copy_from_slice(&digest);
    }
    let store = Store::parse(&bytes).unwrap();
    if let Ok(index) = RevisionIndex::parse(&store) {
        if let Ok(document) = Document::parse(&index) {
            for space in document.spaces.values().flat_map(|s| s.revisions.values()) {
                for (id, node) in &space.nodes {
                    if matches!(node.kind, onestore::document::Kind::RichText { .. }) {
                        let _ = space.text_runs(*id);
                    }
                }
            }
        }
    }
});
