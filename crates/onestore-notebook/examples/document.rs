use onestore::{
    FileDataReference, RevisionIndex, Store,
    document::{Document, Kind},
};
use std::{
    borrow::Cow,
    collections::BTreeSet,
    env, fs,
    io::{self, BufWriter, Write},
    path::{Path, PathBuf},
};

fn write_json(
    path: impl AsRef<Path>,
    value: &impl serde::Serialize,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut output = BufWriter::new(fs::File::create(path)?);
    serde_json::to_writer(&mut output, value)?;
    output.flush()?;
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args_os().skip(1);
    let path = args.next().ok_or("Provide a .one or .onetoc2 file.")?;
    let destination = args.next().map(PathBuf::from);
    if args.next().is_some() {
        return Err("Provide a source file and an optional new export directory.".into());
    }
    let source_path = PathBuf::from(path);
    let bytes = onestore::read_file(&source_path)?;
    let store = Store::parse(&bytes)?;
    let index = RevisionIndex::parse(&store)?;
    let document = Document::parse(&index)?;
    if let Some(destination) = destination {
        fs::create_dir(&destination)?;
        fs::create_dir(destination.join("assets"))?;
        let parent = source_path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let mut source = onestore_notebook::Local::open(parent)?;
        let section = source_path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or("Use a UTF-8 section filename")?;
        let mut assets = Vec::new();
        let mut seen = BTreeSet::new();
        for node in document
            .spaces
            .values()
            .flat_map(|s| s.revisions.values())
            .flat_map(|r| r.nodes.values())
        {
            if let Kind::File {
                reference, payload, ..
            } = &node.kind
                && seen.insert(serde_json::to_string(reference)?)
            {
                let data = match reference {
                    FileDataReference::Internal(_) => {
                        Cow::Borrowed(payload.ok_or("Missing embedded file data")?)
                    }
                    FileDataReference::External(filename) => {
                        match onestore_notebook::read_external_asset(
                            &mut source,
                            section,
                            filename,
                            256 * 1024 * 1024,
                        ) {
                            Ok(bytes) => Cow::Owned(bytes),
                            Err(error) => {
                                let kind = match &error {
                                    onestore_notebook::Error::Io { error, .. } => {
                                        format!("{:?}", error.kind())
                                    }
                                    _ => "InvalidData".into(),
                                };
                                assets.push(serde_json::json!({"reference":reference,"path":null,"error":{"kind":kind,"message":error.to_string()}}));
                                continue;
                            }
                        }
                    }
                    FileDataReference::Invalid => {
                        assets.push(serde_json::json!({"reference":reference,"path":null,"error":{"kind":"InvalidData","message":"The document marks this payload as unavailable"}}));
                        continue;
                    }
                };
                let path = format!("assets/{}.bin", assets.len());
                fs::write(destination.join(&path), data)?;
                assets.push(serde_json::json!({"reference":reference,"path":path}));
            }
        }
        write_json(destination.join("assets.json"), &assets)?;
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
        write_json(destination.join("text.json"), &text)?;
        write_json(destination.join("document.json"), &document)?;
    } else {
        serde_json::to_writer(io::stdout().lock(), &document)?;
    }
    Ok(())
}
