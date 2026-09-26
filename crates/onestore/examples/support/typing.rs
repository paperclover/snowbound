//! Text typed as ops through a `Section`, for the examples that edit a section file.
#![allow(dead_code)]

use onestore::{
    Arena, ExGuid, Section, Transaction,
    op::{Edit, Op, OpError, PageOp},
};
use std::{
    ops::Range,
    time::{SystemTime, UNIX_EPOCH},
};

/// FILETIME now, as an edit's `at`.
pub fn now() -> u64 {
    let unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    (unix.as_secs() + 11_644_473_600) * 10_000_000 + u64::from(unix.subsec_nanos() / 100)
}

/// The op replacing `range` of the text object `text` in `space` with `with`.
pub fn text(space: ExGuid, text: ExGuid, range: Range<u32>, with: &str) -> Edit {
    let op = PageOp::Text {
        text,
        range,
        with: with.to_owned(),
    };
    Edit {
        at: now(),
        ops: vec![Op::Page { space, op }],
    }
}

/// The transaction `edit` seals into on `image`, as `author` made it; none when it stores
/// nothing.
pub fn sealed(image: &[u8], author: &str, edit: &Edit) -> Result<Option<Transaction>, OpError> {
    let arena = Arena::default();
    let mut section = Section::open(&arena, image.to_vec()).map_err(OpError::Failed)?;
    section.apply(author, edit)?;
    section.seal().map_err(OpError::Failed)
}
