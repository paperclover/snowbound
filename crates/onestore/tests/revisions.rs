use onestore::{ExGuid, FileDataReference, ObjectData, PropertySets, RevisionIndex, Store, Value};
use std::fs;

const TABLE: &str = "../../corpus/native/20260905-05/snapshots/07-table/notebook/synthetic.one";

#[test]
fn persisted_identities_preserve_native_byte_order_and_canonical_form() {
    let text = "{00112233-4455-6677-8899-AABBCCDDEEFF},42";
    let id: ExGuid = text.parse().unwrap();
    assert_eq!(
        id.guid,
        [
            0x33, 0x22, 0x11, 0, 0x55, 0x44, 0x77, 0x66, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee,
            0xff
        ]
    );
    assert_eq!(id.n, 42);
    assert_eq!(text.to_lowercase().parse::<ExGuid>().unwrap(), id);
    assert_eq!(id.to_string(), text);
    let mut random = 7_u64;
    for n in 0..1024 {
        let guid = std::array::from_fn(|_| {
            random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
            random.to_le_bytes()[0]
        });
        let id = ExGuid { guid, n };
        assert_eq!(id.to_string().parse::<ExGuid>().unwrap(), id);
        assert_eq!(
            serde_json::from_str::<ExGuid>(&serde_json::to_string(&id).unwrap()).unwrap(),
            id
        );
    }
    for id in [
        ExGuid::default(),
        ExGuid {
            guid: [255; 16],
            n: u32::MAX,
        },
    ] {
        assert_eq!(id.to_string().parse::<ExGuid>().unwrap(), id);
    }
    for at in 0..text.len() {
        let mut changed = text.as_bytes().to_vec();
        changed[at] = b'?';
        assert!(
            String::from_utf8(changed)
                .unwrap()
                .parse::<ExGuid>()
                .is_err()
        );
    }
    for changed in [
        text.replace(",42", ",+42"),
        text.replace(",42", ",042"),
        text.replace(",42", ",4294967296"),
        text.replace(",42", ",-1"),
        text.replace("00", "+0"),
        text.replace("00", "é"),
        "{00000000-0000-0000-0000-000000000000},1".to_owned(),
        format!(" {text}"),
        format!("{text} "),
    ] {
        assert!(changed.parse::<ExGuid>().is_err(), "{changed}");
        assert!(serde_json::from_str::<ExGuid>(&serde_json::to_string(&changed).unwrap()).is_err());
    }
}

#[test]
fn native_encryption_remains_opaque() {
    let bytes =
        fs::read("../../corpus/native-encrypted/encrypted-01/notebook/synthetic.one").unwrap();
    let store = Store::parse(&bytes).unwrap();
    assert!(store.checksum_mismatches.is_empty());
    let index = RevisionIndex::parse(&store).unwrap();
    let mut encrypted = 0;
    for (osid, space) in &index.spaces {
        for (rid, revision) in &space.revisions {
            let resolved = index.resolve(*osid, *rid).unwrap();
            if revision.encrypted {
                encrypted += 1;
                assert!(
                    resolved
                        .objects
                        .values()
                        .any(|object| matches!(object.data, ObjectData::Encrypted(_)))
                );
                assert_eq!(
                    resolved.reachable().unwrap_err().message,
                    "Encrypted property references are unavailable"
                );
            }
        }
    }
    assert!(encrypted > 0);
}

#[test]
fn external_payload_names_cannot_escape_the_onefiles_directory() {
    for (reference, valid) in [
        ("<file>2a83ae62-6754-4383-8e2b-4033ff3cfba1.onebin", true),
        (
            "<file>../2a83ae62-6754-4383-8e2b-4033ff3cfba1.onebin",
            false,
        ),
        (
            "<file>C:\\2a83ae62-6754-4383-8e2b-4033ff3cfba1.onebin",
            false,
        ),
        (
            "<file>2a83ae62-6754-4383-8e2b-4033ff3cfba1.onebin:stream",
            false,
        ),
        ("<ifndf>{2a83ae62-6754-4383-8e2b-4033ff3cfba1}", true),
        ("<invfdo>", true),
        ("<invfdo>garbage", false),
    ] {
        let bytes: Vec<_> = reference
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        let object = onestore::Object {
            jcid: 0x80036,
            reference_count: 1,
            data: ObjectData::File {
                reference: &bytes,
                extension: &[],
            },
            global_ids: Default::default(),
        };
        assert_eq!(object.file_reference().is_ok(), valid, "{reference}");
    }
}

#[test]
fn active_revision_contains_current_text_and_exact_attachment_bytes() {
    let bytes = fs::read(TABLE).unwrap();
    let store = Store::parse(&bytes).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let mut strings = Vec::new();
    let mut attachments = Vec::new();
    let mut pages = 0;
    for (osid, space) in &index.spaces {
        let rid = space.labels[&(ExGuid::default(), 1)];
        let revision = index.resolve(*osid, rid).unwrap();
        for oid in revision.reachable().unwrap() {
            let object = &revision.objects[&oid];
            pages += usize::from(object.jcid == 0x6000b);
            if let Some(FileDataReference::Internal(guid)) = object.file_reference().unwrap()
                && object.jcid == 0x80036
            {
                attachments.push(store.file_data(guid).unwrap());
            }
            if object.jcid != 0x6000e {
                continue;
            }
            let ObjectData::Properties(data) = object.data else {
                panic!()
            };
            let props = PropertySets::parse(data).unwrap();
            let unicode = props.sets[0]
                .iter()
                .find(|property| property.id == 0x1c001c22);
            if let Some(property) = unicode {
                let Value::Bytes(data) = property.value else {
                    panic!()
                };
                let text = String::from_utf16(
                    &data
                        .chunks_exact(2)
                        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                        .collect::<Vec<_>>(),
                )
                .unwrap();
                strings.push(text.strip_suffix('\0').unwrap().to_owned());
            } else if let Some(property) = props.sets[0]
                .iter()
                .find(|property| property.id == 0x1c003498)
            {
                let Value::Bytes(data) = property.value else {
                    panic!()
                };
                strings.push(String::from_utf8(data.to_vec()).unwrap());
            }
        }
    }
    strings.sort();
    assert_eq!(
        strings,
        [
            "Fictitious positioned outline.",
            "Fictitious: café, 東京, مرحبا",
            "Left cell",
            "Right cell"
        ]
    );
    assert_eq!(pages, 1);
    assert_eq!(
        attachments,
        [fs::read("../../corpus/native/20260905-05/assets/fictitious-attachment.txt").unwrap()]
    );
}

#[test]
fn self_dependent_revision_is_rejected() {
    let mut bytes = fs::read(TABLE).unwrap();
    let store = Store::parse(&bytes).unwrap();
    let node = store
        .lists
        .values()
        .flat_map(|list| &list.nodes)
        .find(|node| node.id == 0x1e)
        .unwrap();
    let start = node.payload.as_ptr().addr() - bytes.as_ptr().addr();
    bytes.copy_within(start..start + 20, start + 20);
    let store = Store::parse(&bytes).unwrap();
    assert_eq!(
        RevisionIndex::parse(&store).unwrap_err().message,
        "Revision dependency is not an earlier revision"
    );
}

#[test]
fn damaged_read_only_object_is_rejected_by_its_hash() {
    let mut bytes = fs::read(TABLE).unwrap();
    let store = Store::parse(&bytes).unwrap();
    let node = store
        .lists
        .values()
        .flat_map(|list| &list.nodes)
        .find(|node| node.id == 0xc4)
        .unwrap();
    let onestore::Reference::Data(chunk) = node.reference.unwrap() else {
        panic!()
    };
    bytes[usize::try_from(chunk.offset).unwrap()] ^= 1;
    let store = Store::parse(&bytes).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    assert!(
        index
            .spaces
            .iter()
            .any(|(osid, space)| space.revisions.keys().any(|rid| {
                index
                    .resolve(*osid, *rid)
                    .is_err_and(|error| error.message == "Read-only object checksum mismatch")
            }))
    );
}

#[test]
fn reference_counts_and_graph_cycles_are_checked_independently_of_chunk_hashes() {
    let bytes = fs::read(TABLE).unwrap();
    let store = Store::parse(&bytes).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let space = &index.spaces[&index.root];
    let rid = space.labels[&(ExGuid::default(), 1)];
    let mut revision = index.resolve(index.root, rid).unwrap();
    revision.reachable().unwrap();
    let root = revision.roots[&1];
    revision.objects.get_mut(&root).unwrap().reference_count += 1;
    assert_eq!(
        revision.reachable().unwrap_err().message,
        "Stored object reference count disagrees with the reachable graph"
    );
    let mut data = 0x80000001_u32.to_le_bytes().to_vec();
    data.extend_from_slice(&(0x100 | root.n).to_le_bytes());
    data.extend_from_slice(&1_u16.to_le_bytes());
    data.extend_from_slice(&0x20000001_u32.to_le_bytes());
    let object = revision.objects.get_mut(&root).unwrap();
    object.data = ObjectData::Properties(&data);
    object.global_ids = std::sync::Arc::new([(1, root.guid)].into());
    assert_eq!(
        revision.reachable().unwrap_err().message,
        "Object references form a cycle"
    );
}
