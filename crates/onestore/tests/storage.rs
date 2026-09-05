use onestore::{Chunk, FileDataReference, Reference, RevisionIndex, Store};
use std::{fs, path::Path};

const TEXT: &str = "../../corpus/native/20260905-05/snapshots/02-text/notebook/synthetic.one";

#[test]
fn native_file_data_alignment_is_relative_to_the_object() {
    let source =
        fs::read("../../corpus/m6/native-template-controls-01/notebook/synthetic.one").unwrap();
    let store = Store::parse(&source).unwrap();
    let node = store
        .lists
        .values()
        .flat_map(|l| &l.nodes)
        .find(|n| n.id == 0x94)
        .unwrap();
    let guid = node.payload[node.payload.len() - 16..].try_into().unwrap();
    let Some(Reference::Data(chunk)) = node.reference else {
        panic!("Missing native image container")
    };
    assert_eq!(chunk.offset % 8, 4);
    let payload = store.file_data(guid).unwrap();
    assert_eq!(payload.len(), 10056);
    assert_eq!(
        format!("{:x}", md5::compute(payload)),
        "e1b57a8851177dd25dc05b50b904656a"
    );
    let blob = store.chunk_data(chunk).unwrap();
    for alignment in 0..8 {
        let mut bytes = source.clone();
        let offset = bytes.len().next_multiple_of(8) + alignment;
        bytes.resize(offset, 0);
        bytes.extend_from_slice(blob);
        let mut relocated = Store::parse(&bytes).unwrap();
        let node = relocated
            .lists
            .values_mut()
            .flat_map(|l| &mut l.nodes)
            .find(|n| n.id == 0x94)
            .unwrap();
        node.reference = Some(Reference::Data(Chunk {
            offset: offset as u64,
            length: chunk.length,
        }));
        assert_eq!(
            relocated.file_data(guid).unwrap(),
            payload,
            "alignment {alignment}"
        );
    }
    let mut corrupt = source.clone();
    corrupt[(chunk.offset + chunk.length - 1) as usize] ^= 1;
    assert!(Store::parse(&corrupt).unwrap().file_data(guid).is_err());
}

fn files(path: &Path, result: &mut Vec<std::path::PathBuf>) {
    for entry in fs::read_dir(path).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files(&path, result);
        } else if path
            .extension()
            .is_some_and(|ext| ext == "one" || ext == "onetoc2")
        {
            result.push(path);
        }
    }
}

#[test]
fn native_corpus_has_committed_lists_and_matching_checksums() {
    let mut paths = Vec::new();
    for path in [
        "../../corpus/native",
        "../../corpus/native-ink",
        "../../corpus/native-delete",
    ] {
        files(Path::new(path), &mut paths);
    }
    assert!(paths.len() >= 35);
    for path in paths {
        let bytes = fs::read(&path).unwrap();
        let store =
            Store::parse(&bytes).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        assert!(store.checksum_mismatches.is_empty(), "{}", path.display());
        assert!(!store.lists.is_empty());
        let index = RevisionIndex::parse(&store)
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        index
            .validate_current()
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        for (osid, space) in &index.spaces {
            for rid in space.revisions.keys() {
                let revision = index
                    .resolve(*osid, *rid)
                    .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
                revision
                    .reachable()
                    .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
                for object in revision.objects.values() {
                    if let Some(FileDataReference::Internal(guid)) =
                        object.file_reference().unwrap()
                    {
                        store.file_data(guid).unwrap();
                    }
                }
            }
        }
    }
}

#[test]
fn compressed_nil_references_are_normalized_before_scaling() {
    let source = fs::read(TEXT).unwrap();
    let store = Store::parse(&source).unwrap();
    let node = store
        .lists
        .values()
        .flat_map(|list| &list.nodes)
        .find(|node| node.id == 0x84)
        .unwrap();
    for (format, width) in [(0_u32, 8_usize), (1, 4), (2, 2), (3, 4)] {
        let mut bytes = source.clone();
        let header = u32::from_le_bytes(bytes[node.offset..node.offset + 4].try_into().unwrap());
        let changed = (header & !((3 << 23) | (3 << 25))) | (format << 23) | (2 << 25);
        bytes[node.offset..node.offset + 4].copy_from_slice(&changed.to_le_bytes());
        bytes[node.offset + 4..node.offset + 4 + width].fill(0xff);
        bytes[node.offset + 4 + width] = 0;
        let parsed = Store::parse(&bytes).unwrap();
        let changed = parsed
            .lists
            .values()
            .flat_map(|list| &list.nodes)
            .find(|candidate| candidate.offset == node.offset)
            .unwrap();
        assert_eq!(
            changed.reference,
            Some(onestore::Reference::Data(onestore::Chunk {
                offset: u64::MAX,
                length: 0
            }))
        );
    }
}

#[test]
fn native_log_prefixes_define_readable_states() {
    let mut bytes =
        fs::read("../../corpus/native/20260905-05/snapshots/00-empty/notebook/synthetic.one")
            .unwrap();
    let transactions = Store::parse(&bytes).unwrap().header.transaction_count;
    assert_eq!(transactions, 4);
    for count in 1..=transactions {
        bytes[96..100].copy_from_slice(&count.to_le_bytes());
        let store =
            Store::parse(&bytes).unwrap_or_else(|error| panic!("transaction {count}: {error}"));
        assert!(store.checksum_mismatches.is_empty());
    }
}

#[test]
fn unused_links_and_tail_bytes_are_ignored() {
    let mut bytes = fs::read(TEXT).unwrap();
    let store = Store::parse(&bytes).unwrap();
    let expected: Vec<_> = store
        .lists
        .iter()
        .map(|(id, list)| (*id, list.nodes.len()))
        .collect();
    let tails: Vec<_> = store
        .lists
        .values()
        .map(|list| *list.fragments.last().unwrap())
        .collect();
    for tail in tails {
        let end = usize::try_from(tail.offset + tail.length).unwrap();
        bytes[end - 20..end - 8].fill(0xfe);
    }
    bytes.extend_from_slice(&[0xff; 4096]);
    let store = Store::parse(&bytes).unwrap();
    assert_eq!(
        store
            .lists
            .iter()
            .map(|(id, list)| (*id, list.nodes.len()))
            .collect::<Vec<_>>(),
        expected
    );
}

#[test]
fn malformed_structure_is_rejected() {
    let source = fs::read(TEXT).unwrap();
    let root = usize::try_from(Store::parse(&source).unwrap().header.root.offset).unwrap();
    for (offset, replacement) in [
        (96, vec![0; 4]),
        (160, vec![0xff; 12]),
        (root, vec![0; 8]),
        (root + 12, 1_u32.to_le_bytes().to_vec()),
        (root + 16, vec![0; 4]),
    ] {
        let mut bytes = source.clone();
        bytes[offset..offset + replacement.len()].copy_from_slice(&replacement);
        assert!(Store::parse(&bytes).is_err(), "offset {offset:#x}");
    }
    for end in 0..1024 {
        assert!(Store::parse(&source[..end]).is_err());
    }
}

#[test]
fn bit_changes_in_header_and_first_chunks_do_not_panic() {
    let mut bytes = fs::read(TEXT).unwrap();
    for index in 0..4096 {
        for bit in 0..8 {
            bytes[index] ^= 1 << bit;
            let _ = Store::parse(&bytes);
            bytes[index] ^= 1 << bit;
        }
    }
}

#[test]
fn checksum_damage_is_reported_without_discarding_committed_content() {
    let mut bytes = fs::read(TEXT).unwrap();
    bytes[2060] ^= 1;
    let store = Store::parse(&bytes).unwrap();
    assert_eq!(store.checksum_mismatches[0], 2056);
    assert_eq!(store.header.transaction_count, 16);
}

#[test]
fn native_toc_continuation_excludes_the_flushed_entry_from_crc() {
    let path = "../../corpus/m6/native-structure-01/notebook/Open Notebook.onetoc2";
    let mut bytes = fs::read(path).unwrap();
    let store = Store::parse(&bytes).unwrap();
    assert_eq!(store.header.transaction_log.length, 44);
    assert_eq!(
        u64::from_le_bytes(bytes[0x640..0x648].try_into().unwrap()),
        0xac4
    );
    assert!(store.checksum_mismatches.is_empty());
    assert_eq!(&bytes[0xacc..0xad4], &[1, 0, 0, 0, 0x29, 0x9d, 0xf0, 0x68]);
    bytes[0xad0] ^= 1;
    assert_eq!(Store::parse(&bytes).unwrap().checksum_mismatches, [0xacc]);
}
