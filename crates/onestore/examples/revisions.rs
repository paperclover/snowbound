use onestore::{RevisionIndex, Store};
use std::{env, fs};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    for path in env::args_os().skip(1) {
        let bytes = fs::read(&path)?;
        let store = Store::parse(&bytes)?;
        let index = RevisionIndex::parse(&store)?;
        index
            .validate_current()
            .map_err(|error| format!("{path:?}: {error}"))?;
        for (id, space) in &index.spaces {
            for rid in space.revisions.keys() {
                let resolved = index
                    .resolve(*id, *rid)
                    .map_err(|error| format!("{path:?}: {id} / {rid}: {error}"))?;
                resolved
                    .reachable()
                    .map_err(|error| format!("{path:?}: {id} / {rid}: {error}"))?;
                for object in resolved.objects.values() {
                    if let Some(onestore::FileDataReference::Internal(guid)) =
                        object.file_reference()?
                    {
                        store.file_data(guid)?;
                    }
                    if let onestore::ObjectData::Properties(bytes) = object.data {
                        onestore::PropertySets::parse(bytes)?;
                    }
                }
            }
        }
        let revisions: usize = index
            .spaces
            .values()
            .map(|space| space.revisions.len())
            .sum();
        println!(
            "{:?}: {} object spaces, {revisions} revisions",
            path,
            index.spaces.len()
        );
    }
    Ok(())
}
