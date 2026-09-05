use onestore::{ExGuid, FileDataReference, ObjectData, PropertySets, RevisionIndex, Store, Value};
use std::{
    collections::BTreeSet,
    env,
    io::{self, Write},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut output = io::BufWriter::new(io::stdout().lock());
    for path in env::args_os().skip(1) {
        let bytes = onestore::read_file(path)?;
        let store = Store::parse(&bytes)?;
        let index = RevisionIndex::parse(&store)?;
        index.validate_current()?;
        let mut seen = BTreeSet::new();
        let mut pending = vec![index.root];
        while let Some(osid) = pending.pop() {
            if !seen.insert(osid) {
                continue;
            }
            let rid = index.spaces[&osid].labels[&(ExGuid::default(), 1)];
            let revision = index.resolve(osid, rid)?;
            for oid in revision.reachable()? {
                let object = &revision.objects[&oid];
                pending.extend(object.references()?.object_spaces);
                if object.jcid == 0x6000b {
                    let metadata = revision
                        .roots
                        .get(&2)
                        .map(|oid| revision.objects[oid].jcid)
                        .unwrap_or(0);
                    writeln!(output, "page\t{osid}\t{metadata:x}")?;
                }
                let mut record = None;
                if object.jcid == 0x6000e
                    && let ObjectData::Properties(data) = object.data
                {
                    let properties = PropertySets::parse(data)?;
                    let boiler = properties.sets[0]
                        .iter()
                        .any(|p| matches!(p.id, 0x88001c87 | 0x88001c88 | 0x88001cb5));
                    record = Some(("ascii", &[][..]));
                    let property = properties.sets[0]
                        .iter()
                        .find(|property| property.id == 0x1c001c22)
                        .or_else(|| {
                            properties.sets[0]
                                .iter()
                                .find(|property| property.id == 0x1c003498)
                        });
                    if let Some(property) = property
                        && let Value::Bytes(data) = property.value
                    {
                        record = Some((
                            if property.id == 0x1c001c22 {
                                if boiler { "boiler-unicode" } else { "unicode" }
                            } else {
                                if boiler { "boiler-ascii" } else { "ascii" }
                            },
                            data,
                        ));
                    }
                }
                if let Some(FileDataReference::Internal(guid)) = object.file_reference()? {
                    record = Some(("file", store.file_data(guid)?));
                }
                if let Some((kind, data)) = record {
                    write!(output, "{kind}\t{osid}\t")?;
                    for byte in data {
                        write!(output, "{byte:02x}")?;
                    }
                    writeln!(output)?;
                }
            }
        }
    }
    Ok(())
}
