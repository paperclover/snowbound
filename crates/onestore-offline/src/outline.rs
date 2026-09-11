use super::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Observed {
    path: Vec<ExGuid>,
    values: Vec<Value>,
}

/// A layout or saved expansion intent with its original ancestry and property values.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutlineEdit {
    pub object: ExGuid,
    pub change: onestore::OutlineEdit,
    observed: Observed,
}

fn observe(
    source: &[u8],
    space: ExGuid,
    object: ExGuid,
    change: onestore::OutlineEdit,
) -> Result<Option<Observed>> {
    let store = Store::parse(source)?;
    let index = RevisionIndex::parse(&store)?;
    let document = Document::parse(&index)?;
    let Ok(view) = document.active(space) else {
        return Ok(None);
    };
    let pages = document.pages_in(space)?;
    if pages.len() != 1 {
        return Ok(None);
    }
    let Some(mut paths) = active_paths(view, &pages, &[object]) else {
        return Ok(None);
    };
    let path = paths.pop().unwrap();
    let node = &view.nodes[&object];
    let values = match (change, &node.kind) {
        (onestore::OutlineEdit::Position { .. }, Kind::Outline { .. }) => {
            vec![json!(node.layout.x), json!(node.layout.y)]
        }
        (onestore::OutlineEdit::Width { .. }, Kind::Outline { .. }) => {
            vec![
                json!(node.layout.max_width),
                json!(node.layout.width_set_by_user.unwrap_or(false)),
                json!(node.extra[0].iter().find(|field| field.id == 0x14001cdb)),
            ]
        }
        (onestore::OutlineEdit::Collapsed(_), Kind::Paragraph { collapse_state, .. }) => {
            vec![json!(collapse_state.unwrap_or(0))]
        }
        _ => return Ok(None),
    };
    Ok(Some(Observed { path, values }))
}

impl Replica {
    /// Durably records layout or saved expansion changes, preserving independent remote edits.
    pub fn outline(
        &self,
        source: &[u8],
        space: ExGuid,
        object: ExGuid,
        change: onestore::OutlineEdit,
    ) -> Result<Option<u64>> {
        let (edit, prepared) = OutlineEdit::capture(source, space, object, change)?;
        self.record(source, space, Operation::Outline(edit), &prepared)
    }
}

impl OutlineEdit {
    pub(crate) fn capture<'a>(
        source: &'a [u8],
        space: ExGuid,
        object: ExGuid,
        change: onestore::OutlineEdit,
    ) -> Result<(Self, PreparedEdit<'a>)> {
        let prepared = PreparedEdit::outline(source, space, object, change)?;
        let observed = observe(source, space, object, change)?.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "Prepared outline edit has no active target",
            )
        })?;
        Ok((
            Self {
                object,
                change,
                observed,
            },
            prepared,
        ))
    }

    pub(crate) fn prepare<'a>(
        &self,
        snapshot: &'a [u8],
        space: ExGuid,
    ) -> Result<std::result::Result<PreparedEdit<'a>, ConflictKind>> {
        let Some(current) = observe(snapshot, space, self.object, self.change)? else {
            return Ok(Err(ConflictKind::TargetUnavailable));
        };
        if current.path != self.observed.path {
            return Ok(Err(ConflictKind::StructureChanged));
        }
        let prepared = match PreparedEdit::outline(snapshot, space, self.object, self.change) {
            Ok(prepared) => prepared,
            Err(_) => return Ok(Err(ConflictKind::UnsupportedEdit)),
        };
        let wanted =
            observe(prepared.as_bytes(), space, self.object, self.change)?.ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Prepared outline edit lost its target",
                )
            })?;
        if self.observed.values.len() != current.values.len()
            || wanted.values.len() != current.values.len()
        {
            return Ok(Err(ConflictKind::UnsupportedEdit));
        }
        if self
            .observed
            .values
            .iter()
            .zip(&current.values)
            .zip(&wanted.values)
            .any(|((before, current), wanted)| current != before && current != wanted)
        {
            return Ok(Err(ConflictKind::LayoutChanged));
        }
        Ok(Ok(prepared))
    }
}
