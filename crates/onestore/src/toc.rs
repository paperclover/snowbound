//! Edits to a notebook's table of contents: the documented `jcidPersistablePropertyContainerForTOC`
//! root carries the notebook's colour and lists section and section-group entries, each
//! carrying a file identity, an ordering number and a filename. A section's colour lives in
//! its own metadata (`op::SectionOp::Color`); its entry keeps 0xffffffff, as OneNote writes.

use crate::{
    Error, ExGuid, PropertySets, Store, Transaction, Value,
    document::{Document, Kind},
    revisions::RevisionIndex,
    write::{PropertyObject, RevisionEdit, applied, build_on, check, fresh_guid},
};
use std::collections::{BTreeMap, BTreeSet};

fn invalid(message: &'static str) -> Error {
    Error { offset: 0, message }
}

/// One change to a table of contents, addressed by the entry's file identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TocEdit {
    /// A section (`name.one`) or, with `group`, a section-group folder appended last.
    Add {
        filename: String,
        identity: [u8; 16],
        group: bool,
    },
    Rename {
        identity: [u8; 16],
        filename: String,
    },
    /// The notebook's colour as COLORREF.
    Color(u32),
    /// Every entry, in the wanted order; entries left out keep their relative order after these.
    Order(Vec<[u8; 16]>),
    Remove {
        identity: [u8; 16],
    },
}

fn component(name: &str) -> bool {
    !name.is_empty() && !name.contains(['/', '\\', '\0']) && name != "." && name != ".."
}

/// Applies table-of-contents edits (sections and section groups: add, rename, order,
/// remove; the notebook's colour) in order as one revision of a `.onetoc2` file; none when they change
/// nothing.
pub fn edit_table_of_contents(
    source: &[u8],
    edits: &[TocEdit],
) -> Result<Option<Transaction>, Error> {
    let store = Store::parse(source)?;
    if store.header.file_type != crate::FileType::TableOfContents {
        return Err(invalid("Choose a table-of-contents file"));
    }
    let index = RevisionIndex::parse(&store)?;
    index.validate_current()?;
    let document = Document::parse(&index)?;
    let space = document.root;
    let revision = document.active(space)?;
    let root = *revision
        .roots
        .get(&1)
        .ok_or_else(|| invalid("Missing notebook TOC root"))?;
    let Some(Kind::Toc { entries, .. }) = revision.nodes.get(&root).map(|node| &node.kind) else {
        return Err(invalid("Missing notebook TOC root"));
    };
    // Entries in stored order with their identities and filenames.
    let mut listed: Vec<(ExGuid, [u8; 16], String)> = Vec::new();
    for id in entries {
        let Some(Kind::Toc {
            filename,
            identity: Some(identity),
            ..
        }) = revision.nodes.get(id).map(|node| &node.kind)
        else {
            return Err(invalid("Incomplete notebook TOC reference"));
        };
        listed.push((*id, *identity, filename.clone().unwrap_or_default()));
    }
    let mut created: BTreeMap<ExGuid, PropertyObject> = BTreeMap::new();
    let mut renamed = BTreeSet::new();
    let mut color = None;
    for edit in edits {
        let position = |identity: &[u8; 16]| {
            listed
                .iter()
                .position(|(_, known, _)| known == identity)
                .ok_or_else(|| invalid("The table of contents has no entry with that identity"))
        };
        match edit {
            TocEdit::Add {
                filename,
                identity,
                group,
            } => {
                if !component(filename)
                    || (!group && !filename.to_ascii_lowercase().ends_with(".one"))
                    || *identity == [0; 16]
                {
                    return Err(invalid(
                        "A TOC entry needs a section filename or group folder name and an identity",
                    ));
                }
                if listed.iter().any(|(_, known, name)| {
                    known == identity || name.eq_ignore_ascii_case(filename)
                }) {
                    return Err(invalid(
                        "The table of contents already lists that section or group",
                    ));
                }
                let id = ExGuid {
                    guid: fresh_guid()?,
                    n: 10,
                };
                let mut values: Vec<(u32, Vec<u8>)> = vec![
                    (0x1c001d94, identity.to_vec()),
                    (0x14001cb9, 0u32.to_le_bytes().to_vec()),
                    (0x1c001d6b, crate::create::string(filename)),
                ];
                if !group {
                    values.push((0x14001cbe, vec![0xff; 4]));
                }
                let mut object = PropertyObject {
                    jcid: 0x20001,
                    bytes: crate::create::properties(&values)?,
                    global_ids: std::sync::Arc::new(BTreeMap::from([(0, id.guid)])),
                };
                object.reference(id)?;
                created.insert(id, object);
                listed.push((id, *identity, filename.clone()));
            }
            TocEdit::Rename { identity, filename } => {
                if !component(filename) {
                    return Err(invalid(
                        "A TOC entry needs a filename without path separators",
                    ));
                }
                let at = position(identity)?;
                if listed
                    .iter()
                    .enumerate()
                    .any(|(i, (_, _, name))| i != at && name.eq_ignore_ascii_case(filename))
                {
                    return Err(invalid("The table of contents already lists that name"));
                }
                let id = listed[at].0;
                listed[at].2 = filename.clone();
                renamed.insert(id);
            }
            TocEdit::Color(value) => color = Some(*value),
            TocEdit::Order(wanted) => {
                let mut ordered: Vec<(ExGuid, [u8; 16], String)> = Vec::new();
                for identity in wanted {
                    let at = position(identity)?;
                    if ordered.iter().any(|(_, known, _)| known == identity) {
                        return Err(invalid("An entry is ordered twice"));
                    }
                    ordered.push(listed[at].clone());
                }
                let rest: Vec<_> = listed
                    .iter()
                    .filter(|(_, known, _)| !wanted.contains(known))
                    .cloned()
                    .collect();
                ordered.extend(rest);
                listed = ordered;
            }
            TocEdit::Remove { identity } => {
                let at = position(identity)?;
                let (id, _, _) = listed.remove(at);
                created.remove(&id);
            }
        }
    }
    let listed = listed;
    let transaction = build_on(&index, &[], None, |index| {
        let raw = index.resolve_active(space)?;
        let mut changed = BTreeMap::new();
        let mut root_object = PropertyObject::from_object(&raw.objects[&root])?;
        let mut references = Vec::new();
        for (order, (id, _, filename)) in listed.iter().enumerate() {
            let mut object = match created.remove(id) {
                Some(object) => object,
                None => PropertyObject::from_object(&raw.objects[id])?,
            };
            let stored_order = PropertySets::parse(&object.bytes)?.sets[0]
                .iter()
                .find(|p| p.id == 0x14001cb9)
                .and_then(|p| match p.value {
                    Value::Bytes(b) => b.try_into().ok().map(u32::from_le_bytes),
                    _ => None,
                });
            let mut updates: Vec<(u32, Vec<u8>)> = Vec::new();
            if stored_order != Some(order as u32 + 1) {
                updates.push((0x14001cb9, (order as u32 + 1).to_le_bytes().to_vec()));
            }
            if renamed.contains(id) {
                updates.push((0x1c001d6b, crate::create::string(filename)));
            }
            let updates: Vec<(u32, &[u8])> = updates
                .iter()
                .map(|(id, bytes)| (*id, bytes.as_slice()))
                .collect();
            if !updates.is_empty() {
                object.set(&updates)?;
            }
            references.extend(root_object.reference(*id)?);
            if !updates.is_empty() || created.contains_key(id) || !raw.objects.contains_key(id) {
                changed.insert(*id, object);
            }
        }
        root_object.set(&[(0x24001cf6, &references)])?;
        if let Some(color) = color {
            root_object.set(&[(0x14001cbe, &color.to_le_bytes())])?;
        }
        changed.insert(root, root_object);
        Ok(BTreeMap::from([(space, RevisionEdit::Update(changed))]))
    })?;
    check(&applied(source, transaction.as_ref())?, true)?;
    Ok(transaction)
}
