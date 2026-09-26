use onestore::{
    Arena, CommitError, CommitState, ExGuid, Section, Transaction,
    op::{Edit, Op, PageOp},
};
use std::{
    io,
    ops::Range,
    time::{SystemTime, UNIX_EPOCH},
};

/// The transaction replacing `range` of `text` in `image`, typed now.
pub fn replaced(
    image: &[u8],
    space: ExGuid,
    text: ExGuid,
    range: Range<u32>,
    with: &str,
) -> Result<Transaction, CommitError> {
    let failed = |error: String| CommitError {
        state: CommitState::NotCommitted,
        error: io::Error::new(io::ErrorKind::InvalidData, error),
    };
    let arena = Arena::default();
    let mut section = Section::open(&arena, image.to_vec()).map_err(|e| failed(e.to_string()))?;
    let at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| failed(e.to_string()))?
        .as_nanos() as u64
        / 100
        + 116_444_736_000_000_000;
    let edit = Edit {
        at,
        ops: vec![Op::Page {
            space,
            op: PageOp::Text {
                text,
                range,
                with: with.to_owned(),
            },
        }],
    };
    section
        .apply("Concurrent client", &edit)
        .map_err(|e| failed(format!("{e:?}")))?;
    section
        .seal()
        .map_err(|e| failed(e.to_string()))?
        .ok_or_else(|| failed("The edit changed nothing".into()))
}
