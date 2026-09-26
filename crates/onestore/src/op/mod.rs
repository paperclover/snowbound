//! Object-level edits of a section: what an editor emits, a queue stores and
//! `Section::apply` writes. Identities of new objects are the emitter's, so applying an edit
//! to another image of the section creates the same objects; UTF-16 code units measure text.

use crate::{
    ExGuid, OutlineEdit, PageCreation, PageEdit, TextAttribute,
    document::{Layout, Tag},
    page::{Definition, InkStroke, Page, PageObject, PageParagraph, Paragraph, TableCell,
        TableColumn, TableRow},
};
use serde::{Deserialize, Serialize};
use std::ops::Range;

mod apply;
pub(crate) mod content;
pub(crate) mod levels;
pub(crate) mod lower;
pub(crate) mod model;
pub(crate) mod properties;
pub(crate) mod table;
#[cfg(test)]
mod tests;

pub use lower::{lower, lower_page};

/// Property identifiers with their encoded values.
pub(crate) type Values = Vec<(u32, Vec<u8>)>;

/// One user action, applied whole or not at all.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Edit {
    /// FILETIME when the action happened; the modification time of what it changes.
    pub at: u64,
    pub ops: Vec<Op>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Op {
    Page { space: ExGuid, op: PageOp },
    Section(SectionOp),
}

/// A change to the page an object space holds. Targets are stored identities; paragraph
/// properties name the paragraph, text edits its text object.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum PageOp {
    /// Replaces a range; inserted text takes the format of the run it lands in, the
    /// following run at a boundary except at the end.
    Text {
        text: ExGuid,
        range: Range<u32>,
        with: String,
    },
    /// Sets and clears character formatting over a range; an empty text's `0..0` is its
    /// insertion format. Cleared properties are inherited again.
    Format {
        text: ExGuid,
        range: Range<u32>,
        set: Vec<TextAttribute>,
        clear: Vec<TextProperty>,
    },
    /// Makes the range a hyperlink to `target`, its field code stored hidden before the
    /// label, or with `None` removes the link around it, leaving the label as plain text.
    Link {
        text: ExGuid,
        range: Range<u32>,
        target: Option<String>,
    },
    /// Rewrites an equation's linear text and spans whole.
    Equation { text: ExGuid, math: Paragraph },
    /// Inserts paragraphs before a direct child of `container`, or last. Children follow
    /// their parents in `paragraphs`; levels are absolute. Text keeps its spans' formats;
    /// lists, tags, styles and collapse state are set by their own ops.
    Insert {
        container: ExGuid,
        before: Option<ExGuid>,
        paragraphs: Vec<PageParagraph>,
    },
    /// Enter at `at`: the text left of it stays, the rest moves to a new paragraph `paragraph`
    /// with text object `right` and a copy of each list node under `lists`, in order.
    Split {
        text: ExGuid,
        at: u32,
        paragraph: ExGuid,
        right: ExGuid,
        lists: Vec<ExGuid>,
    },
    /// Appends the right text to the left one and removes the right paragraph; an empty
    /// left text is replaced by the right text object.
    Join { left: ExGuid, right: ExGuid },
    /// Moves a subtree before a direct child of `parent`, or last; `None` names the page,
    /// whose children are outlines, pictures and ink. A paragraph keeps its level where it
    /// lies deeper than its new parent.
    Move {
        object: ExGuid,
        parent: Option<ExGuid>,
        before: Option<ExGuid>,
    },
    /// Removes a subtree; an emptied table cell receives an empty paragraph.
    Delete { object: ExGuid },
    /// Sets a paragraph's absolute outline level, regrouping its container.
    Level { paragraph: ExGuid, level: u32 },
    /// Outline position and width, or a paragraph's saved expansion state.
    Outline { object: ExGuid, edit: OutlineEdit },
    /// Paragraph formatting stored on a paragraph's text; `None` leaves a value as it is.
    Paragraph {
        paragraph: ExGuid,
        alignment: Option<u8>,
        rtl: Option<bool>,
        space_before: Option<f32>,
        space_after: Option<f32>,
        line_spacing: Option<f32>,
        language: Option<u32>,
    },
    /// References a paragraph style, created from its definition where the page lacks it.
    Style {
        paragraph: ExGuid,
        style: ExGuid,
        definition: Definition,
    },
    /// Replaces a paragraph's list nodes; a node another paragraph owns is copied.
    List {
        paragraph: ExGuid,
        lists: Vec<(ExGuid, Definition)>,
    },
    /// Replaces the note tags of a paragraph, its text or a table; tag definitions the page
    /// lacks are created from `definitions`.
    Tags {
        target: ExGuid,
        tags: Vec<Tag>,
        definitions: Vec<(ExGuid, Definition)>,
    },
    /// Adds an outline with its paragraphs, a picture with its payload, or ink, before a
    /// page child or on top.
    Add {
        object: PageObject,
        before: Option<ExGuid>,
    },
    /// A stored picture's position, displayed size and description.
    Picture {
        picture: ExGuid,
        layout: Layout,
        alt: Option<String>,
    },
    /// A stored attachment's shown name, recorded source path and icon size.
    Attachment {
        attachment: ExGuid,
        filename: String,
        source_path: Option<String>,
        size: Option<[f32; 2]>,
    },
    /// Adds and erases whole strokes of stored ink.
    Strokes {
        ink: ExGuid,
        add: Vec<InkStroke>,
        remove: Vec<ExGuid>,
    },
    Table { table: ExGuid, edit: TableEdit },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum TableEdit {
    /// New rows before a row, or last, each with one cell per column.
    Rows {
        before: Option<ExGuid>,
        rows: Vec<TableRow>,
    },
    /// A new column at `at`: one cell per row, in row order.
    Column {
        at: u32,
        width: f32,
        cells: Vec<TableCell>,
    },
    DeleteRow(ExGuid),
    DeleteColumn(u32),
    /// Every column's width and lock.
    Columns(Vec<TableColumn>),
    Borders(bool),
    /// A cell's shading (`None` clears it) and indentation table.
    Cell {
        cell: ExGuid,
        shading: Option<u32>,
        indents: Vec<f32>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum SectionOp {
    Create(PageCreation),
    /// A page created with `creation` holding `page`'s content under the identities it
    /// carries (`Page::copy` gives fresh ones); the created title stays.
    Import { creation: PageCreation, page: Page },
    /// Page moves and indentation, in order.
    Pages(Vec<PageEdit>),
    /// Removes pages permanently; their revisions stay stored.
    Delete(Vec<ExGuid>),
}

/// A character property that `PageOp::Format` can clear so the text inherits it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextProperty {
    Bold,
    Italic,
    Underline,
    Strike,
    Superscript,
    Subscript,
    Hidden,
    Hyperlink,
    HyperlinkLabel,
    Math,
    Font,
    FontSize,
    Color,
    Highlight,
}

impl TextProperty {
    pub(crate) fn id(self) -> u32 {
        match self {
            Self::Bold => 0x08001c04,
            Self::Italic => 0x08001c05,
            Self::Underline => 0x08001c06,
            Self::Strike => 0x08001c07,
            Self::Superscript => 0x08001c08,
            Self::Subscript => 0x08001c09,
            Self::Hidden => 0x08001e16,
            Self::Hyperlink => 0x08001e14,
            Self::HyperlinkLabel => 0x08001e19,
            Self::Math => 0x08003401,
            Self::Font => 0x1c001c0a,
            Self::FontSize => 0x10001c0b,
            Self::Color => 0x14001c0c,
            Self::Highlight => 0x14001c0d,
        }
    }
}

/// Why `Section::apply` refused an edit; the section is as it was before it.
#[derive(Clone, Debug, PartialEq)]
pub enum OpError {
    /// An object an op names is not reachable in its space's active revision.
    TargetUnavailable(ExGuid),
    /// A new object's identity is already reachable.
    DuplicateIdentity(ExGuid),
    /// An anchor is no direct child of its container, or a move would make a cycle.
    StructureChanged(&'static str),
    /// The writers cannot make this change to this content.
    Unsupported(&'static str),
    /// Writing failed after the section began changing; it must be reopened.
    Failed(crate::Error),
}

impl std::fmt::Display for OpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TargetUnavailable(id) => write!(f, "{id} is not on the page"),
            Self::DuplicateIdentity(id) => write!(f, "{id} already exists"),
            Self::StructureChanged(message) | Self::Unsupported(message) => f.write_str(message),
            Self::Failed(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for OpError {}
