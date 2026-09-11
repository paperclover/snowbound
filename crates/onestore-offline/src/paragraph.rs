use super::*;
use onestore::{ParagraphJoin, ParagraphSplit};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Observed {
    text: String,
    structure: Value,
}

/// A stable split intent and the paragraph state observed before local publication.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SplitEdit {
    pub intent: ParagraphSplit,
    observed: Observed,
}

/// A join intent and both paragraphs' observed text, placement and tag ownership.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JoinEdit {
    pub intent: ParagraphJoin,
    observed: [Observed; 2],
}

fn observe(source: &[u8], space: ExGuid, texts: &[ExGuid]) -> Result<Option<Vec<Observed>>> {
    let store = Store::parse(source)?;
    let index = RevisionIndex::parse(&store)?;
    let document = Document::parse(&index)?;
    let Ok(view) = document.active(space) else {
        return Ok(None);
    };
    let pages = document.pages_in(space)?;
    let Some(paths) = active_paths(view, &pages, texts) else {
        return Ok(None);
    };
    let mut observed = Vec::new();
    for (text, path) in texts.iter().zip(paths) {
        let node = &view.nodes[text];
        let Kind::RichText { text: content, .. } = &node.kind else {
            return Ok(None);
        };
        let Some(paragraph) = path.first().and_then(|id| view.nodes.get(id)) else {
            return Ok(None);
        };
        let Kind::Paragraph { lists, .. } = &paragraph.kind else {
            return Ok(None);
        };
        if paragraph.content != [*text] {
            return Ok(None);
        }
        let lists: Vec<_> = lists.iter().map(|id| (*id, &view.nodes[id].kind)).collect();
        observed.push(Observed {
            text: content.clone(),
            structure: json!({
                "path": path,
                "children": paragraph.children,
                "child_level": paragraph.child_level,
                "paragraph": paragraph.kind,
                "lists": lists,
                "tags": node.tags,
            }),
        });
    }
    Ok(Some(observed))
}

impl Replica {
    /// Durably queues a split with stable new identities and observed structural preconditions.
    pub fn split(
        &self,
        source: &[u8],
        space: ExGuid,
        intent: &ParagraphSplit,
    ) -> Result<Option<u64>> {
        let (operation, prepared) = SplitEdit::capture(source, space, intent)?;
        self.record(source, space, Operation::Split(operation), &prepared)
    }

    /// Durably queues a join; changed placement, children or tags require conflict review.
    pub fn join(
        &self,
        source: &[u8],
        space: ExGuid,
        intent: &ParagraphJoin,
    ) -> Result<Option<u64>> {
        let (operation, prepared) = JoinEdit::capture(source, space, intent)?;
        self.record(source, space, Operation::Join(operation), &prepared)
    }
}

impl SplitEdit {
    pub(crate) fn capture<'a>(
        source: &'a [u8],
        space: ExGuid,
        intent: &ParagraphSplit,
    ) -> Result<(Self, PreparedEdit<'a>)> {
        let prepared = PreparedEdit::split(source, space, intent)?;
        let Some(mut observed) = observe(source, space, &[intent.position().0])? else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Select text in an active paragraph",
            )
            .into());
        };
        Ok((
            Self {
                intent: intent.clone(),
                observed: observed.remove(0),
            },
            prepared,
        ))
    }

    pub(crate) fn prepare<'a>(
        &self,
        source: &'a [u8],
        space: ExGuid,
    ) -> Result<std::result::Result<PreparedEdit<'a>, ConflictKind>> {
        let (text, offset) = self.intent.position();
        let Some(observed) = observe(source, space, &[text])? else {
            return Ok(Err(ConflictKind::TargetUnavailable));
        };
        if observed[0].structure != self.observed.structure {
            return Ok(Err(ConflictKind::StructureChanged));
        }
        let Some(range) = rebase::rebase(&self.observed.text, &observed[0].text, offset..offset)
        else {
            return Ok(Err(ConflictKind::TextChanged));
        };
        Ok(
            PreparedEdit::split(source, space, &self.intent.reposition(range.start))
                .map_err(|_| ConflictKind::UnsupportedEdit),
        )
    }
}

impl JoinEdit {
    pub(crate) fn review(&self, source: &[u8], space: ExGuid) -> Result<Self> {
        let (reviewed, _) = Self::capture(source, space, &self.intent)?;
        if reviewed.observed[0].text.is_empty() != self.observed[0].text.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "The join would change its surviving text identity",
            )
            .into());
        }
        Ok(reviewed)
    }

    pub(crate) fn capture<'a>(
        source: &'a [u8],
        space: ExGuid,
        intent: &ParagraphJoin,
    ) -> Result<(Self, PreparedEdit<'a>)> {
        let prepared = PreparedEdit::join(source, space, intent)?;
        let Some(observed) = observe(source, space, &intent.texts())? else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Select text in two active paragraphs",
            )
            .into());
        };
        Ok((
            Self {
                intent: intent.clone(),
                observed: observed.try_into().unwrap(),
            },
            prepared,
        ))
    }

    pub(crate) fn prepare<'a>(
        &self,
        source: &'a [u8],
        space: ExGuid,
    ) -> Result<std::result::Result<PreparedEdit<'a>, ConflictKind>> {
        let Some(observed) = observe(source, space, &self.intent.texts())? else {
            return Ok(Err(ConflictKind::TargetUnavailable));
        };
        for (i, (old, new)) in self.observed.iter().zip(&observed).enumerate() {
            if old.structure != new.structure {
                return Ok(Err(ConflictKind::StructureChanged));
            }
            let at = if i == 0 {
                u32::try_from(old.text.encode_utf16().count()).map_err(io::Error::other)?
            } else {
                0
            };
            if rebase::rebase(&old.text, &new.text, at..at).is_none() {
                return Ok(Err(ConflictKind::TextChanged));
            }
        }
        Ok(PreparedEdit::join(source, space, &self.intent)
            .map_err(|_| ConflictKind::UnsupportedEdit))
    }
}
