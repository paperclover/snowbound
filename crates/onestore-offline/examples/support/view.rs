use onestore::{
    ExGuid, RevisionIndex, Store,
    document::{Document, Kind},
};

pub struct View {
    pub space: ExGuid,
    pub object: ExGuid,
    pub revision: ExGuid,
    pub text: String,
}

pub fn view(bytes: &[u8]) -> Result<View, Box<dyn std::error::Error>> {
    let store = Store::parse(bytes)?;
    if !store.checksum_mismatches.is_empty() {
        return Err("Transaction checksum damage".into());
    }
    let index = RevisionIndex::parse(&store)?;
    index.validate_current()?;
    let document = Document::parse(&index)?;
    let mut found = Vec::new();
    for (sid, page) in document.pages()? {
        let space = &document.spaces[&sid];
        let rid = space.contexts[&ExGuid::default()];
        let revision = &space.revisions[&rid];
        let mut pending = vec![page];
        let mut seen = std::collections::BTreeSet::new();
        while let Some(oid) = pending.pop() {
            if !seen.insert(oid) {
                continue;
            }
            let node = &revision.nodes[&oid];
            pending.extend(
                node.children
                    .iter()
                    .chain(&node.content)
                    .chain(&node.structure)
                    .copied(),
            );
            if let Kind::RichText { text, .. } = &node.kind
                && text.starts_with("Concurrent edits:")
            {
                revision.text_runs(oid)?;
                found.push(View {
                    space: sid,
                    object: oid,
                    revision: rid,
                    text: text.clone(),
                });
            }
        }
    }
    if found.len() != 1 {
        return Err("Expected one concurrent-edit paragraph".into());
    }
    Ok(found.pop().unwrap())
}
