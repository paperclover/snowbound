use super::*;
use onestore::{PageEdit, PagePosition};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

type PageOrder = Vec<(ExGuid, u32)>;

/// An atomic page batch with its observed order and indentation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageEdits {
    pub edits: Vec<PageEdit>,
    observed: PageOrder,
}

fn observe(source: &[u8]) -> Result<Option<(ExGuid, PageOrder)>> {
    let store = Store::parse(source)?;
    let index = RevisionIndex::parse(&store)?;
    let document = Document::parse(&index)?;
    let mut pages = Vec::new();
    for (sid, _) in document.pages()? {
        let view = document.active(sid)?;
        let Some(metadata) = view.roots.get(&2).and_then(|id| view.nodes.get(id)) else {
            return Ok(None);
        };
        let Kind::Metadata { level, .. } = metadata.kind else {
            return Ok(None);
        };
        pages.push((sid, level.unwrap_or(1)));
    }
    Ok(Some((document.root, pages)))
}

fn positions(pages: &[(ExGuid, u32)]) -> Result<BTreeMap<ExGuid, (usize, u32)>> {
    let mut positions = BTreeMap::new();
    for (at, (sid, level)) in pages.iter().enumerate() {
        if sid.guid == [0; 16]
            || !(1..=3).contains(level)
            || positions.insert(*sid, (at, *level)).is_some()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Page observations need unique identities and valid indentation",
            )
            .into());
        }
    }
    Ok(positions)
}

impl Replica {
    /// Durably queues the complete page batch using the supplied local snapshot.
    pub fn pages(&self, source: &[u8], edits: &[PageEdit]) -> Result<Option<u64>> {
        let (space, batch, prepared) = PageEdits::capture(source, edits)?;
        self.record(source, space, Operation::Pages(batch), &prepared)
    }
}

impl PageEdits {
    fn capture<'a>(
        source: &'a [u8],
        edits: &[PageEdit],
    ) -> Result<(ExGuid, Self, PreparedEdit<'a>)> {
        let prepared = PreparedEdit::pages(source, edits)?;
        let (space, observed) = observe(source)?.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "Prepared page edits have no page metadata",
            )
        })?;
        positions(&observed)?;
        Ok((
            space,
            Self {
                edits: edits.to_vec(),
                observed,
            },
            prepared,
        ))
    }

    pub(crate) fn prepare<'a>(
        &self,
        source: &'a [u8],
        space: ExGuid,
    ) -> Result<std::result::Result<PreparedEdit<'a>, ConflictKind>> {
        let Some((root, current)) = observe(source)? else {
            return Ok(Err(ConflictKind::TargetUnavailable));
        };
        if root != space {
            return Ok(Err(ConflictKind::TargetUnavailable));
        }
        let before = positions(&self.observed)?;
        let current = positions(&current)?;
        for edit in &self.edits {
            let Some((_, original_level)) = before.get(&edit.space()) else {
                return Ok(Err(ConflictKind::TargetUnavailable));
            };
            let Some((_, current_level)) = current.get(&edit.space()) else {
                return Ok(Err(ConflictKind::TargetUnavailable));
            };
            if current_level != original_level && *current_level != edit.level() {
                return Ok(Err(ConflictKind::StructureChanged));
            }
        }
        let prepared = match PreparedEdit::pages(source, &self.edits) {
            Ok(prepared) => prepared,
            Err(_) => return Ok(Err(ConflictKind::StructureChanged)),
        };
        let (_, wanted) = observe(prepared.as_bytes())?.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "Prepared page edits lost page metadata",
            )
        })?;
        let wanted = positions(&wanted)?;
        for edit in &self.edits {
            if edit.position() == PagePosition::Keep {
                continue;
            }
            let sid = edit.space();
            let peers = before
                .keys()
                .filter(|id| **id != sid && current.contains_key(id));
            let mut unchanged = true;
            let mut satisfied = true;
            for id in peers {
                let observed = current[id].0 < current[&sid].0;
                unchanged &= observed == (before[id].0 < before[&sid].0);
                satisfied &= observed == (wanted[id].0 < wanted[&sid].0);
            }
            if !unchanged && !satisfied {
                return Ok(Err(ConflictKind::StructureChanged));
            }
        }
        Ok(Ok(prepared))
    }

    pub(crate) fn review(&self, source: &[u8], space: ExGuid, edits: &[PageEdit]) -> Result<Self> {
        let identities_match = edits.len() == self.edits.len()
            && edits.iter().all(|edit| {
                self.edits
                    .iter()
                    .find(|original| original.space() == edit.space())
                    .is_some_and(|original| {
                        original
                            .reposition(edit.position(), edit.level())
                            .is_ok_and(|revised| revised == *edit)
                    })
            });
        if !identities_match {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Retain the original page and series identities when reviewing a page batch",
            )
            .into());
        }
        let (root, reviewed, _) = Self::capture(source, edits)?;
        if root != space {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Review the original section for page edits",
            )
            .into());
        }
        Ok(reviewed)
    }
}
