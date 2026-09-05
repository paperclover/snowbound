use onestore::{
    FileDataReference, RevisionIndex, Store,
    document::{Document, Kind},
};
use std::{collections::BTreeSet, env, fs, io, path::PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args_os().skip(1);
    let path = args.next().ok_or("Provide a .one or .onetoc2 file.")?;
    let destination = args.next().map(PathBuf::from);
    if args.next().is_some() {
        return Err("Provide a source file and an optional new export directory.".into());
    }
    let bytes = onestore::read_file(path)?;
    let store = Store::parse(&bytes)?;
    let index = RevisionIndex::parse(&store)?;
    let document = Document::parse(&index)?;
    if let Some(destination) = destination {
        fs::create_dir(&destination)?;
        fs::create_dir(destination.join("assets"))?;
        let mut assets = Vec::new();
        let mut seen = BTreeSet::new();
        for node in document
            .spaces
            .values()
            .flat_map(|s| s.revisions.values())
            .flat_map(|r| r.nodes.values())
        {
            if let Kind::File {
                reference: FileDataReference::Internal(guid),
                payload: Some(data),
                ..
            } = &node.kind
                && seen.insert(guid)
            {
                let path = format!("assets/{}.bin", assets.len());
                fs::write(destination.join(&path), data)?;
                assets.push(serde_json::json!({"reference": FileDataReference::Internal(*guid), "path": path}));
            }
        }
        serde_json::to_writer(fs::File::create(destination.join("assets.json"))?, &assets)?;
        let mut text = std::collections::BTreeMap::new();
        for (sid, space) in &document.spaces {
            let mut revisions = std::collections::BTreeMap::new();
            for (rid, revision) in &space.revisions {
                let mut objects = std::collections::BTreeMap::new();
                for (oid, node) in &revision.nodes {
                    if matches!(node.kind, Kind::RichText { .. }) {
                        objects.insert(*oid, revision.text_runs(*oid)?);
                    }
                }
                revisions.insert(*rid, objects);
            }
            text.insert(*sid, revisions);
        }
        serde_json::to_writer(fs::File::create(destination.join("text.json"))?, &text)?;
        serde_json::to_writer(
            fs::File::create(destination.join("document.json"))?,
            &document,
        )?;
    } else {
        serde_json::to_writer(io::stdout().lock(), &document)?;
    }
    Ok(())
}
