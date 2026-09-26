//! Edits as ops through a `Section`, sealed: the image an edit leaves, for tests that read
//! it back.
#![allow(dead_code)]

use onestore::{
    Arena, ExGuid, Section, Transaction,
    op::{Edit, Op, OpError, PageOp},
    page::Page,
};

/// FILETIME now, as an edit's `at`.
pub fn now() -> u64 {
    let unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap();
    (unix.as_secs() + 11_644_473_600) * 10_000_000 + u64::from(unix.subsec_nanos() / 100)
}

/// The transaction `ops` seal into on `source`, as one edit by `author`; none when they
/// store nothing.
pub fn transaction(
    source: &[u8],
    author: &str,
    ops: Vec<Op>,
) -> Result<Option<Transaction>, OpError> {
    let arena = Arena::default();
    let mut section = Section::open(&arena, source.to_vec()).map_err(OpError::Failed)?;
    section.apply(author, &Edit { at: now(), ops })?;
    section.seal().map_err(OpError::Failed)
}

/// `source` after `ops`, applied as one edit and sealed.
pub fn edited(source: &[u8], ops: Vec<Op>) -> Result<Vec<u8>, OpError> {
    let mut image = source.to_vec();
    if let Some(transaction) = transaction(source, "Author", ops)? {
        transaction.apply(&mut image).map_err(OpError::Failed)?;
    }
    Ok(image)
}

/// `source` after `ops` on the page in `space`.
pub fn page_edited(source: &[u8], space: ExGuid, ops: Vec<PageOp>) -> Result<Vec<u8>, OpError> {
    edited(
        source,
        ops.into_iter().map(|op| Op::Page { space, op }).collect(),
    )
}

/// `source` with the page in `space` taken to `after` by the ops `op::lower_page` finds,
/// as a test states an edit by the page it leaves.
pub fn saved(source: &[u8], space: ExGuid, after: &Page) -> Result<Vec<u8>, String> {
    let arena = Arena::default();
    let section = Section::open(&arena, source.to_vec()).map_err(|error| error.to_string())?;
    let before = section.page(space).map_err(|error| error.to_string())?;
    let ops = onestore::op::lower_page(&before, after).map_err(|error| error.to_string())?;
    page_edited(source, space, ops).map_err(|error| error.to_string())
}

/// A plain paragraph holding `text` in OneNote's default text style, with fresh identities.
pub fn paragraph(text: &str) -> onestore::page::PageParagraph {
    use onestore::page::{PageParagraph, Paragraph, ParagraphContent, TextObject, text::new_id};
    let format = onestore::document::Format {
        font: Some("Calibri".into()),
        font_size: Some(11.0),
        language: Some(0x409),
        ..Default::default()
    };
    PageParagraph {
        id: new_id().unwrap(),
        parent: None,
        level: 1,
        style: None,
        format: Default::default(),
        content: ParagraphContent::Text(TextObject {
            id: new_id().unwrap(),
            date_field: None,
            text: Paragraph::new(text.into(), format),
            tags: Vec::new(),
        }),
        lists: Vec::new(),
        tags: Vec::new(),
        media: Default::default(),
        collapsed: false,
    }
}

/// A new outline at `(x, y)` in points holding `paragraphs`.
pub fn outline(
    x: f32,
    y: f32,
    paragraphs: Vec<onestore::page::PageParagraph>,
) -> onestore::page::PageObject {
    use onestore::page::{Outline, PageObject, text::new_id};
    PageObject::Outline(Outline {
        id: new_id().unwrap(),
        title: false,
        min_width: None,
        layout: onestore::document::Layout {
            x: Some(x),
            y: Some(y),
            ..Default::default()
        },
        indents: Vec::new(),
        paragraphs,
        unsupported: Vec::new(),
    })
}

/// An edit applied through a `Section`: the image it leaves and the transaction publishing
/// it, none when it stores nothing.
pub struct Applied {
    pub image: Vec<u8>,
    pub transaction: Option<Transaction>,
}

impl Applied {
    pub fn as_bytes(&self) -> &[u8] {
        &self.image
    }

    /// Publishes the transaction under the filesystem adapter's exclusion.
    #[cfg(any(unix, windows))]
    pub fn commit_file(
        &self,
        path: impl AsRef<std::path::Path>,
    ) -> Result<(), onestore::CommitError> {
        self.transaction
            .as_ref()
            .expect("the edit stores a revision")
            .commit_file(path)
    }

    /// Publishes the transaction under caller-held exclusion.
    pub fn commit(&self, io: &mut impl onestore::CommitIo) -> Result<(), onestore::CommitError> {
        self.transaction
            .as_ref()
            .expect("the edit stores a revision")
            .commit(io)
    }
}

/// `ops` applied to `source` as one edit by `author`.
pub fn apply(source: &[u8], author: &str, ops: Vec<Op>) -> Result<Applied, OpError> {
    let transaction = transaction(source, author, ops)?;
    let mut image = source.to_vec();
    if let Some(transaction) = &transaction {
        transaction.apply(&mut image).map_err(OpError::Failed)?;
    }
    Ok(Applied { image, transaction })
}

/// `saved` as an `Applied`, to publish it.
pub fn save(source: &[u8], space: ExGuid, after: &Page) -> Result<Applied, String> {
    let arena = Arena::default();
    let section = Section::open(&arena, source.to_vec()).map_err(|error| error.to_string())?;
    let before = section.page(space).map_err(|error| error.to_string())?;
    let ops = onestore::op::lower_page(&before, after).map_err(|error| error.to_string())?;
    let ops = ops.into_iter().map(|op| Op::Page { space, op }).collect();
    apply(source, "Author", ops).map_err(|error| error.to_string())
}

/// `op` on the page in `space`, applied to `source`.
pub fn page_op(source: &[u8], space: ExGuid, op: PageOp) -> Result<Applied, OpError> {
    apply(source, "Author", vec![Op::Page { space, op }])
}

/// `op` on the section, applied to `source`.
pub fn section_op(source: &[u8], op: onestore::op::SectionOp) -> Result<Applied, OpError> {
    apply(source, "Author", vec![Op::Section(op)])
}

/// Adds an outline at `(x, y)` holding one paragraph of `text`: the op and the identities
/// of the outline and its text.
pub fn new_outline(x: f32, y: f32, text: &str) -> (PageOp, ExGuid, ExGuid) {
    let paragraph = paragraph(text);
    let text = paragraph.text().unwrap().id;
    let object = outline(x, y, vec![paragraph]);
    let id = object.id();
    (
        PageOp::Add {
            object,
            before: None,
        },
        id,
        text,
    )
}
