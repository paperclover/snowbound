use onestore::{IdStream, PropertySets, Reference, Store, Value};
use std::fs;

#[test]
fn native_property_chunks_preserve_unicode_and_ascii_text() {
    let bytes =
        fs::read("../../corpus/native/20260905-05/snapshots/07-table/notebook/synthetic.one")
            .unwrap();
    let store = Store::parse(&bytes).unwrap();
    let mut strings = Vec::new();
    for node in store.lists.values().flat_map(|list| &list.nodes) {
        if matches!(node.id, 0xa4 | 0xa5 | 0xc4 | 0xc5)
            && let Some(Reference::Data(chunk)) = node.reference
        {
            let start = usize::try_from(chunk.offset).unwrap();
            let end = start + usize::try_from(chunk.length).unwrap();
            let object = PropertySets::parse(&bytes[start..end]).unwrap();
            for property in object.sets.iter().flatten() {
                if let Value::Bytes(data) = property.value {
                    if property.id == 0x1c001c22 {
                        let text = String::from_utf16(
                            &data
                                .chunks_exact(2)
                                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                                .collect::<Vec<_>>(),
                        )
                        .unwrap();
                        assert!(text.ends_with('\0'));
                        strings.push(text.trim_end_matches('\0').to_owned());
                    } else if property.id == 0x1c003498 && data.is_ascii() {
                        strings.push(String::from_utf8(data.to_vec()).unwrap());
                    }
                }
            }
        }
    }
    for expected in [
        "Fictitious: café, 東京, مرحبا",
        "Fictitious positioned outline.",
        "Left cell",
        "Right cell",
    ] {
        assert!(
            strings.iter().any(|text| text == expected),
            "{expected}: {strings:?}"
        );
    }
}

#[test]
fn property_nesting_does_not_use_the_call_stack() {
    let mut bytes = 0x80000000_u32.to_le_bytes().to_vec();
    for _ in 0..100_000 {
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&0x44000001_u32.to_le_bytes());
    }
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    let properties = PropertySets::parse(&bytes).unwrap();
    assert_eq!(properties.sets.len(), 100_001);
    drop(properties);
}

#[test]
fn nested_sets_consume_reference_streams_in_property_order() {
    let mut bytes = 0x80000002_u32.to_le_bytes().to_vec();
    bytes.extend_from_slice(&0x101_u32.to_le_bytes());
    bytes.extend_from_slice(&0x202_u32.to_le_bytes());
    bytes.extend_from_slice(&2_u16.to_le_bytes());
    bytes.extend_from_slice(&0x44000001_u32.to_le_bytes());
    bytes.extend_from_slice(&0x20000002_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&0x20000003_u32.to_le_bytes());
    let properties = PropertySets::parse(&bytes).unwrap();
    assert_eq!(properties.sets[0][0].value, Value::Sets(1..2));
    assert_eq!(
        properties.sets[1][0].value,
        Value::References {
            stream: IdStream::Objects,
            compact_ids: &0x101_u32.to_le_bytes()
        }
    );
    assert_eq!(
        properties.sets[0][1].value,
        Value::References {
            stream: IdStream::Objects,
            compact_ids: &0x202_u32.to_le_bytes()
        }
    );
}

#[test]
fn impossible_nested_array_sizes_are_rejected_before_allocation() {
    let mut bytes = 0x80000000_u32.to_le_bytes().to_vec();
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&0x40000001_u32.to_le_bytes());
    bytes.extend_from_slice(&u32::MAX.to_le_bytes());
    bytes.extend_from_slice(&0x44000000_u32.to_le_bytes());
    assert!(PropertySets::parse(&bytes).is_err());
}
