#![no_main]
use libfuzzer_sys::fuzz_target;
use onestore::{ExGuid, ObjectData, PropertySets, RevisionIndex, Store, Value, create_section};
fuzz_target!(|input: &[u8]| {
    let count = input.first().copied().unwrap_or(0) as usize % 32;
    let entries: Vec<_> = (0..count)
        .map(|i| (format!("section-{i}.one"), [i as u8 + 1; 16]))
        .collect();
    let sections: Vec<_> = entries
        .iter()
        .map(|(name, id)| (name.as_str(), *id))
        .collect();
    let toc = onestore::create_table_of_contents("Open Notebook.onetoc2", &sections).unwrap();
    let store = Store::parse(&toc).unwrap();
    assert!(store.checksum_mismatches.is_empty());
    let index = RevisionIndex::parse(&store).unwrap();
    index.validate_current().unwrap();
    let revision = index
        .resolve(
            index.root,
            index.spaces[&index.root].labels[&(ExGuid::default(), 1)],
        )
        .unwrap();
    assert_eq!(
        revision.objects[&revision.roots[&1]]
            .references()
            .unwrap()
            .objects
            .len(),
        count
    );
    let Ok(text) = std::str::from_utf8(input) else {
        return;
    };
    let result = create_section("synthetic.one", text, "Fuzz author");
    if text.contains(['\0', '\n']) {
        assert!(result.is_err());
        return;
    }
    let bytes = result.unwrap();
    let store = Store::parse(&bytes).unwrap();
    assert!(store.checksum_mismatches.is_empty());
    let index = RevisionIndex::parse(&store).unwrap();
    index.validate_current().unwrap();
    let expected: Vec<_> = text
        .encode_utf16()
        .chain([0])
        .flat_map(u16::to_le_bytes)
        .collect();
    let mut found = 0;
    for (osid, space) in &index.spaces {
        let revision = index
            .resolve(*osid, space.labels[&(ExGuid::default(), 1)])
            .unwrap();
        for object in revision
            .objects
            .values()
            .filter(|object| object.jcid == 0x6000e)
        {
            let ObjectData::Properties(data) = object.data else {
                panic!()
            };
            assert!(
                PropertySets::parse(data).unwrap().sets[0]
                    .iter()
                    .any(|p| p.id == 0x1c001c22 && p.value == Value::Bytes(&expected))
            );
            found += 1;
        }
    }
    assert_eq!(found, 1);
});
