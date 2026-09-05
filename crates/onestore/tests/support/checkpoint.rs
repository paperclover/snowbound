use onestore::{ExGuid, RevisionIndex, Store};

pub fn pending(source: &[u8], sid: ExGuid, oid: ExGuid, property: u32) -> Vec<u8> {
    let mut source = source.to_vec();
    for value in 1_000_000_u32.. {
        let store = Store::parse(&source).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let rid = index.spaces[&sid].labels[&(ExGuid::default(), 1)];
        let depth =
            std::iter::successors(Some(rid), |id| index.spaces[&sid].revisions[id].dependency)
                .count();
        assert!(depth <= 512);
        if depth == 512 {
            return source;
        }
        source =
            onestore::replace_property_bytes(&source, sid, oid, property, &value.to_le_bytes())
                .unwrap();
    }
    unreachable!()
}
