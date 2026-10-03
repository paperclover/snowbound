//! OneNote's Home-ribbon commands: character formatting, lists, alignment and note tags.

use super::*;
use crate::layout::{DEFAULT_FONT, DEFAULT_FONT_SIZE};
use onestore::document::{Format, Kind, Tag};
use onestore::page::ParagraphContent;
use onestore::page::text::new_id;
use serde::{Deserialize, Serialize};

/// The COLORREF OneNote stores for automatic colour, which overrides a style's colour.
pub(super) const AUTOMATIC: u32 = 0xff00_0000;
/// To Do's check box.
const CHECKBOX: u16 = 3;

/// A toolbar command on the selection, or with only a caret, on the text typed there next.
#[derive(Clone, Debug, PartialEq)]
pub enum Formatting {
    /// Removed where every selected character has it, applied to all of them otherwise.
    Toggle(Toggle),
    Font(String),
    FontSize(f32),
    /// A COLORREF, or `None` for automatic.
    Color(Option<u32>),
    /// A COLORREF, or `None` for no highlight.
    Highlight(Option<u32>),
    /// Returns text to its paragraph style.
    Clear,
    /// Toggles OneNote's default bullet or number, as Ctrl+. and Ctrl+/ do.
    Bullets,
    Numbering,
    /// Applies a style from OneNote's bullet or numbering library; `None` removes lists.
    List(Option<ListStyle>),
    Indent,
    Outdent,
    Align(Alignment),
    /// A tag of the user's list and its place there, which the page stores as its action
    /// type.
    Tag(NoteTag, u16),
    /// Removes every note tag from the selected paragraphs.
    RemoveTags,
    /// Checks the selected paragraphs' check boxes, or clears them when all are checked.
    Check,
    /// Gives the selected bulleted and numbered paragraphs this tag in place of their list.
    ToDoList(NoteTag, u16),
    /// Gives the selected paragraphs with this tag OneNote's default bullet in its place.
    BulletedList(NoteTag, u16),
    /// Format Painter: gives the selection the character formatting and alignment of text
    /// picked up with [`CanvasEditor::painted_format`], as OneNote's does.
    Paint(Format),
    /// Gives the selected paragraphs a paragraph style, as OneNote 2010's Styles gallery
    /// does: the page's style of that definition, made where it has none, and their text
    /// loses its character formatting but links, fields and language.
    Style(Definition),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Toggle {
    Bold,
    Italic,
    Underline,
    Strikethrough,
    /// Excludes superscript.
    Subscript,
    /// Excludes subscript.
    Superscript,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Alignment {
    Left,
    Center,
    Right,
}

/// A style of OneNote 2010's bullet or numbering library, by its place in the gallery.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListStyle {
    Bullet(usize),
    Number(usize),
}

/// OneNote 2010's bullet library in gallery order, as it stores each: font, glyph and
/// ListMSAAIndex. Ctrl+. applies the third.
pub const BULLET_LIBRARY: [(&str, &str, u16); 34] = [
    ("Wingdings", "l", 3),
    ("Symbol", "\u{b7}", 1),
    ("Calibri", "\u{2022}", 1),
    ("Courier New", "\u{25cb}", 4),
    ("Wingdings 2", "\u{9d}", 6),
    ("Wingdings 2", "\u{9c}", 5),
    ("Wingdings", "\u{b2}", 13),
    ("Tahoma", "\u{25ca}", 14),
    ("Wingdings", "w", 15),
    ("Wingdings 2", "\u{ae}", 34),
    ("Wingdings", "v", 16),
    ("Arial", "\u{25aa}", 7),
    ("Verdana", "\u{25ab}", 8),
    ("Wingdings", "\u{a7}", 9),
    ("Arial", "\u{25a1}", 10),
    ("Wingdings 3", "}", 11),
    ("Arial", "\u{25ba}", 12),
    ("Arial", "\u{2192}", 21),
    ("Symbol", "\u{de}", 22),
    ("Arial", ">", 20),
    ("Wingdings", "\u{d8}", 23),
    ("Arial", "*", 24),
    ("Wingdings", "\u{ad}", 17),
    ("Wingdings", "\u{ae}", 19),
    ("Wingdings", "\u{af}", 18),
    ("Arial", "-", 25),
    ("Arial", "\u{2013}", 26),
    ("Arial", "\u{2014}", 27),
    ("Wingdings", "J", 28),
    ("Wingdings", "K", 29),
    ("Wingdings", "L", 30),
    ("Wingdings", "\u{fc}", 31),
    ("Wingdings", "(", 32),
    ("Wingdings", "*", 33),
];

/// OneNote 2010's numbering library in gallery order, as NumberListFormat stores each: the
/// number's sequence follows U+FFFD. Ctrl+/ applies the first.
pub const NUMBER_LIBRARY: [&str; 19] = [
    "\u{fffd}\u{0}.",
    "\u{fffd}\u{5}.",
    "\u{fffd}\u{4}.",
    "\u{fffd}\u{3}.",
    "\u{fffd}\u{2}.",
    "\u{fffd}\u{1}.",
    "\u{fffd}\u{0})",
    "\u{fffd}\u{4})",
    "\u{fffd}\u{3})",
    "\u{fffd}\u{2})",
    "\u{fffd}\u{1})",
    "(\u{fffd}\u{0})",
    "(\u{fffd}\u{3})",
    "(\u{fffd}\u{4})",
    "(\u{fffd}\u{2})",
    "(\u{fffd}\u{1})",
    "\u{fffd}\u{6}.",
    "\u{fffd}\u{7}.",
    "\u{fffd}\u{0}-",
];

impl ListStyle {
    pub(super) const BULLET: Self = Self::Bullet(2);
    pub(super) const NUMBER: Self = Self::Number(0);

    /// The library style a stored list has, if it is one.
    pub(super) fn of(kind: &Kind) -> Option<Self> {
        let Kind::List {
            font,
            format: Some(format),
            bullet,
            ..
        } = kind
        else {
            return None;
        };
        match bullet {
            Some(bullet) => BULLET_LIBRARY
                .iter()
                .position(|entry| {
                    (Some(entry.0), entry.1, entry.2) == (font.as_deref(), format.as_str(), *bullet)
                })
                .map(Self::Bullet),
            None => NUMBER_LIBRARY
                .iter()
                .position(|known| known == format)
                .map(Self::Number),
        }
    }
}

/// OneNote's Default font (Options > General): what new outlines' text is set in, as the
/// page's `p` quick style, and the face and colour new titles take.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DefaultFont {
    pub face: String,
    pub size: f32,
    /// COLORREF; none is Automatic.
    pub color: Option<u32>,
}

impl Default for DefaultFont {
    /// OneNote 2010's: Calibri 11, Automatic.
    fn default() -> Self {
        Self {
            face: "Calibri".into(),
            size: 11.0,
            color: None,
        }
    }
}

impl DefaultFont {
    /// The `p` quick style OneNote 2010 stores for new text in this font
    /// (`corpus/default-font`): plain, in its colour, no highlight or paragraph spacing.
    pub(crate) fn body_style(&self) -> Definition {
        Definition {
            kind: Kind::Style {
                name: Some("p".into()),
                next: None,
            },
            format: Format {
                bold: Some(false),
                italic: Some(false),
                underline: Some(false),
                strike: Some(false),
                superscript: Some(false),
                subscript: Some(false),
                font: Some(self.face.clone()),
                font_size: Some(self.size),
                color: Some(self.color.unwrap_or(AUTOMATIC)),
                highlight: Some(AUTOMATIC),
                space_before: Some(0.0),
                space_after: Some(0.0),
                line_spacing: Some(0.0),
                ..Format::default()
            },
        }
    }
}

/// A tag of the user's list as the Customize Tags dialog edits it: what its definition
/// stores besides its place in the list, which the page stores as its action type.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NoteTag {
    pub label: String,
    /// MS-ONE's NoteTagShape; 0 has no symbol.
    pub shape: u16,
    /// COLORREFs; none leaves the text's own.
    pub color: Option<u32>,
    pub highlight: Option<u32>,
    /// Snowbound's own art for the tag, named for its content, which a notebook's
    /// `.snowbound` folder maps the tag's name and symbol to; OneNote draws the symbol.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub art: Option<String>,
}

/// OneNote 2010's tag list before it is customized, the first nine on Ctrl+1 to Ctrl+9
/// (`corpus/structural-probe/tag-gallery.one`): names, symbols and highlights.
const DEFAULT_TAGS: [(&str, u16, Option<u32>); 29] = [
    ("To Do", CHECKBOX, None),
    ("Important", 13, None),
    ("Question", 15, None),
    ("Remember for later", 0, Some(0x0000_ffff)),
    ("Definition", 0, Some(0x0000_ff00)),
    ("Highlight", 136, None),
    ("Contact", 118, None),
    ("Address", 23, None),
    ("Phone number", 18, None),
    ("Web site to visit", 125, None),
    ("Idea", 21, None),
    ("Password", 131, None),
    ("Critical", 17, None),
    ("Project A", 100, None),
    ("Project B", 101, None),
    ("Movie to see", 122, None),
    ("Book to read", 132, None),
    ("Music to listen to", 121, None),
    ("Source for article", 125, None),
    ("Remember for blog", 24, None),
    ("Discuss with <Person A>", 94, None),
    ("Discuss with <Person B>", 94, None),
    ("Discuss with manager", 95, None),
    ("Send in e-mail", 106, None),
    ("Schedule meeting", 12, None),
    ("Call back", 12, None),
    ("To Do priority 1", 28, None),
    ("To Do priority 2", 71, None),
    ("Client request", 8, None),
];

impl NoteTag {
    pub fn defaults() -> Vec<Self> {
        DEFAULT_TAGS
            .iter()
            .map(|&(label, shape, highlight)| Self {
                label: label.into(),
                shape,
                // The highlighting tags also set black text.
                color: highlight.map(|_| 0),
                highlight,
                art: None,
            })
            .collect()
    }

    /// The tag as its definition stores it, without Snowbound's art.
    pub fn stored(&self) -> Self {
        Self {
            art: None,
            ..self.clone()
        }
    }

    /// The definition OneNote stores for the tag at place `action_type` in the list.
    pub(crate) fn definition(&self, action_type: u16) -> Definition {
        Definition {
            kind: Kind::TagDefinition {
                label: Some(self.label.clone()),
                action_type: Some(action_type),
                shape: Some(self.shape),
                color: self.color,
                highlight: self.highlight,
            },
            format: Format::default(),
        }
    }

    /// The tag a stored definition describes, and its action type.
    pub fn of(kind: &Kind) -> Option<(Self, u16)> {
        let Kind::TagDefinition {
            label,
            action_type,
            shape,
            color,
            highlight,
        } = kind
        else {
            return None;
        };
        Some((
            Self {
                label: label.clone().unwrap_or_default(),
                shape: shape.unwrap_or(0),
                color: *color,
                highlight: *highlight,
                art: None,
            },
            action_type.unwrap_or(0),
        ))
    }
}

/// What the selection shows on the toolbar.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FormatState {
    /// Attributes every selected character has.
    pub toggles: Vec<Toggle>,
    /// The font and size every selected character is laid out in, where they share one.
    pub font: Option<String>,
    pub font_size: Option<f32>,
    /// The alignment every selected paragraph shares.
    pub alignment: Option<Alignment>,
    /// Whether every selected paragraph is bulleted or numbered.
    pub bullets: bool,
    pub numbering: bool,
    /// Whether any selected paragraph is bulleted or numbered.
    pub listed: bool,
    /// The library style every selected paragraph's list shares.
    pub list: Option<ListStyle>,
    /// Tags every selected paragraph has, with their action types.
    pub tags: Vec<(NoteTag, u16)>,
    /// The stored name of the paragraph style every selected paragraph shares.
    pub style: Option<String>,
    /// The font and size the selected paragraphs' style gives, or else the Default font,
    /// where they share one: what their text is in without formatting of its own.
    pub style_font: Option<String>,
    pub style_size: Option<f32>,
}

impl Toggle {
    const ALL: [Self; 6] = [
        Self::Bold,
        Self::Italic,
        Self::Underline,
        Self::Strikethrough,
        Self::Subscript,
        Self::Superscript,
    ];

    fn get(self, format: &Format) -> Option<bool> {
        match self {
            Self::Bold => format.bold,
            Self::Italic => format.italic,
            Self::Underline => format.underline,
            Self::Strikethrough => format.strike,
            Self::Subscript => format.subscript,
            Self::Superscript => format.superscript,
        }
    }

    fn slot(self, format: &mut Format) -> &mut Option<bool> {
        match self {
            Self::Bold => &mut format.bold,
            Self::Italic => &mut format.italic,
            Self::Underline => &mut format.underline,
            Self::Strikethrough => &mut format.strike,
            Self::Subscript => &mut format.subscript,
            Self::Superscript => &mut format.superscript,
        }
    }
}

/// Where a selection starts and ends, as text leaf identity and byte offset.
type Ends = [(ExGuid, usize); 2];

/// The selection's ends and the container range holding both; a selection ending at a
/// paragraph's start ends with the paragraph before.
pub(super) fn selected(
    document: &TextDocument,
    selection: Selection,
) -> Result<(Option<ExGuid>, Range<usize>, Ends), EditError> {
    let [anchor, focus] = selection.positions;
    let (start, mut end) = (anchor.min(focus), anchor.max(focus));
    if end.offset == 0 && end.paragraph > start.paragraph {
        end.paragraph -= 1;
        let text = document
            .paragraph(end.paragraph)
            .ok_or(EditError::InvalidRange)?;
        end.offset = text.utf16_offset(text.text().len())?;
    }
    let (container, first, head) = document
        .leaf(start.paragraph)
        .ok_or(EditError::InvalidRange)?;
    let (end_container, last, tail) = document
        .leaf(end.paragraph)
        .ok_or(EditError::InvalidRange)?;
    let range = if container == end_container {
        first..last + 1
    } else {
        let root = |container: Option<ExGuid>, index| {
            container.map_or(Ok(index), |cell| document.root(cell))
        };
        root(container, first)?..root(end_container, last)? + 1
    };
    let byte = |node: &PageParagraph, offset| node.text().unwrap().text.byte_offset(offset);
    Ok((
        if container == end_container {
            container
        } else {
            None
        },
        range,
        [
            (head.id, byte(head, start.offset)?),
            (tail.id, byte(tail, end.offset)?),
        ],
    ))
}

/// The text leaves of `nodes` from `start` through `end`, each with the byte range selected.
fn covered(
    nodes: &[PageParagraph],
    [start, end]: Ends,
) -> impl Iterator<Item = (&PageParagraph, Range<usize>)> {
    let mut done = false;
    leaves(nodes, None)
        .map(|(_, _, node)| node)
        .skip_while(move |node| node.id != start.0)
        .take_while(move |node| !std::mem::replace(&mut done, node.id == end.0))
        .map(move |node| {
            let from = if node.id == start.0 { start.1 } else { 0 };
            let to = if node.id == end.0 {
                end.1
            } else {
                node.text().unwrap().text.text().len()
            };
            (node, from..to)
        })
}

pub(super) fn leaves_mut(nodes: &mut [PageParagraph]) -> Vec<&mut PageParagraph> {
    let mut leaves = Vec::new();
    for node in nodes {
        if node.text().is_some() {
            leaves.push(node);
        } else if let ParagraphContent::Table(table) = &mut node.content {
            for cell in table.rows.iter_mut().flat_map(|row| &mut row.cells) {
                leaves.extend(leaves_mut(&mut cell.paragraphs));
            }
        }
    }
    leaves
}

/// `text` with `change` applied to the formats of bytes `range`, or to its only format when empty.
pub(super) fn restyle(
    text: &Paragraph,
    range: Range<usize>,
    change: impl Fn(&mut Format),
) -> Paragraph {
    if text.text().is_empty() {
        let mut format = text.spans()[0].format.clone();
        change(&mut format);
        return Paragraph::new(String::new(), format);
    }
    let mut runs = Vec::new();
    let mut start = 0;
    for span in text.spans() {
        let [from, to] = [range.start, range.end].map(|at| at.clamp(start, span.end));
        for (part, inside) in [
            (start..from, false),
            (from..to, true),
            (to..span.end, false),
        ] {
            if !part.is_empty() {
                let mut format = span.format.clone();
                if inside {
                    change(&mut format);
                }
                runs.push((text.text()[part].to_owned(), format));
            }
        }
        start = span.end;
    }
    Paragraph::from_runs(runs)
}

/// The formats of the visible characters in `range`, or of the paragraphs without text when the
/// selection holds no characters.
fn character_formats<'a>(
    covered: impl Iterator<Item = (&'a PageParagraph, Range<usize>)>,
) -> Vec<&'a Format> {
    let mut characters = Vec::new();
    let mut blank = Vec::new();
    for (node, range) in covered {
        let text = &node.text().unwrap().text;
        if text.text().is_empty() {
            blank.push(&text.spans()[0].format);
        }
        let mut start = 0;
        for span in text.spans() {
            if start < range.end && span.end > range.start && span.format.hidden != Some(true) {
                characters.push(&span.format);
            }
            start = span.end;
        }
    }
    if characters.is_empty() {
        blank
    } else {
        characters
    }
}

fn common<T: PartialEq>(mut values: impl Iterator<Item = Option<T>>) -> Option<T> {
    let first = values.next()??;
    values
        .all(|value| value.as_ref() == Some(&first))
        .then_some(first)
}

/// `format` moved from paragraph style `old` to `new`: what the old style gave, or nothing
/// did, the new one gives.
/// `text` in paragraph style `definition` from style format `old`: it loses its character
/// formatting but links, fields and language.
pub(super) fn styled(text: &Paragraph, old: &Format, definition: &Definition) -> Paragraph {
    restyle(text, 0..text.text().len(), |format| {
        // An equation keeps its own formatting; what it took from the old style it takes
        // from the new.
        if [format.math, format.embedded_object].contains(&Some(true)) {
            *format = followed(format, old, &definition.format);
            return;
        }
        *format = Format {
            hidden: format.hidden,
            hyperlink: format.hyperlink,
            hyperlink_label: format.hyperlink_label,
            math: format.math,
            embedded_object: format.embedded_object,
            language: format.language,
            alignment: format.alignment,
            rtl: format.rtl,
            list_spacing: format.list_spacing,
            math_object: format.math_object.clone(),
            ..definition.format.clone()
        };
    })
}

fn followed(format: &Format, old: &Format, new: &Format) -> Format {
    let mut format = format.clone();
    macro_rules! follow {
        ($($field:ident),*) => {$(
            if format.$field.is_none() || old.$field.is_some() && format.$field == old.$field {
                format.$field = new.$field.clone();
            }
        )*};
    }
    follow!(
        bold,
        italic,
        underline,
        strike,
        superscript,
        subscript,
        font,
        font_size,
        color,
        highlight,
        space_before,
        space_after,
        line_spacing
    );
    format
}

/// Seconds since 1980, as note tags date themselves.
pub(super) fn time32() -> Option<u32> {
    let now = web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    u32::try_from(now.checked_sub(315_532_800)?).ok()
}

fn tags(node: &PageParagraph) -> impl Iterator<Item = &Tag> {
    node.tags.iter().chain(&node.text().unwrap().tags)
}

impl CanvasEditor {
    /// The format text typed at `position` takes: the caret's pending format, else the text's
    /// before it. Text after a link's label or field code is plain, as OneNote types it, and
    /// math typed into an equation is a run between its objects until it is built up.
    pub(super) fn typing_format(&self, position: TextPosition) -> Result<Format, EditError> {
        if let Some((id, at, format)) = &self.pending
            && *id == self.active_outline().id
            && *at == position
        {
            return Ok(format.clone());
        }
        let text = self
            .active_outline()
            .document
            .paragraph(position.paragraph)
            .ok_or(EditError::InvalidRange)?;
        let mut format = text.format_at(position.offset)?.clone();
        if format.hidden == Some(true) && format.hyperlink == Some(true)
            || super::link::links(text, position.paragraph)
                .iter()
                .any(|link| link.code.is_some() && link.label.end == position.offset)
        {
            super::link::unlinked(&mut format);
        }
        if format.math == Some(true) {
            format = onestore::page::Math::format(&format);
        }
        Ok(format)
    }

    fn list(&self, node: &PageParagraph) -> Option<Formatting> {
        match &self.definitions.get(node.lists.last()?)?.kind {
            Kind::List { format, .. } => Some(
                if format
                    .as_ref()
                    .is_some_and(|format| format.contains('\u{fffd}'))
                {
                    Formatting::Numbering
                } else {
                    Formatting::Bullets
                },
            ),
            _ => None,
        }
    }

    fn list_style(&self, node: &PageParagraph) -> Option<ListStyle> {
        ListStyle::of(&self.definitions.get(node.lists.last()?)?.kind)
    }

    fn tag_kind(&self, tag: &Tag) -> Option<&Kind<'static>> {
        Some(&self.definitions.get(tag.definition.as_ref()?)?.kind)
    }

    /// What Format Painter picks up: the first selected character's formatting, or with only
    /// a caret, what would be typed there.
    pub fn painted_format(&self) -> Result<Format, EditorError> {
        let outline = self.active_outline();
        let (container, range, ends) = selected(&outline.document, outline.selection)?;
        let [anchor, focus] = outline.selection.positions;
        let nodes = &outline.document.container(container)?[range];
        match character_formats(covered(nodes, ends)).first() {
            Some(format) if ends[0] != ends[1] => Ok((*format).clone()),
            _ => Ok(self.typing_format(anchor.min(focus))?),
        }
    }

    pub fn format_state(&self) -> Result<FormatState, EditorError> {
        let outline = self.active_outline();
        self.format_state_of(outline, outline.selection)
    }

    fn format_state_of(
        &self,
        outline: &TextOutline,
        selection: Selection,
    ) -> Result<FormatState, EditorError> {
        let (container, range, ends) = selected(&outline.document, selection)?;
        let nodes = &outline.document.container(container)?[range];
        let paragraphs = covered(nodes, ends)
            .map(|(node, _)| node)
            .collect::<Vec<_>>();
        let caret;
        let formats = if ends[0] == ends[1] {
            let [anchor, focus] = selection.positions;
            caret = self.typing_format(anchor.min(focus))?;
            vec![&caret]
        } else {
            character_formats(covered(nodes, ends))
        };
        let list = |kind| {
            paragraphs
                .iter()
                .all(|node| self.list(node).as_ref() == Some(&kind))
        };
        let styled = |node: &&PageParagraph| {
            (node.style.as_ref())
                .and_then(|style| self.definitions.get(style))
                .map(|definition| &definition.format)
        };
        Ok(FormatState {
            toggles: Toggle::ALL
                .into_iter()
                .filter(|toggle| {
                    formats
                        .iter()
                        .all(|format| toggle.get(format) == Some(true))
                })
                .collect(),
            font: common(
                formats
                    .iter()
                    .map(|format| Some(format.font.as_deref().unwrap_or(DEFAULT_FONT).to_owned())),
            ),
            font_size: common(
                formats
                    .iter()
                    .map(|format| Some(format.font_size.unwrap_or(DEFAULT_FONT_SIZE))),
            ),
            alignment: common(paragraphs.iter().map(|node| {
                Some(
                    match node.text().unwrap().text.spans()[0].format.alignment {
                        Some(1) => Alignment::Center,
                        Some(2) => Alignment::Right,
                        _ => Alignment::Left,
                    },
                )
            })),
            bullets: list(Formatting::Bullets),
            numbering: list(Formatting::Numbering),
            listed: paragraphs.iter().any(|node| self.list(node).is_some()),
            list: common(paragraphs.iter().map(|node| self.list_style(node))),
            tags: paragraphs.first().map_or_else(Vec::new, |first| {
                let mut shared: Vec<_> = tags(first)
                    .filter_map(|tag| NoteTag::of(self.tag_kind(tag)?))
                    .filter(|(tag, action_type)| {
                        let kind = tag.definition(*action_type).kind;
                        paragraphs
                            .iter()
                            .all(|node| tags(node).any(|tag| self.tag_kind(tag) == Some(&kind)))
                    })
                    .collect();
                shared.sort_by_key(|(_, action_type)| *action_type);
                shared
            }),
            style: common(paragraphs.iter().map(|node| {
                match &self.definitions.get(&node.style?)?.kind {
                    Kind::Style { name, .. } => name.clone(),
                    _ => None,
                }
            })),
            style_font: common(paragraphs.iter().map(|node| {
                Some(
                    styled(node)
                        .and_then(|format| format.font.clone())
                        .unwrap_or_else(|| self.default_font.face.clone()),
                )
            })),
            style_size: common(paragraphs.iter().map(|node| {
                Some(
                    styled(node)
                        .and_then(|format| format.font_size)
                        .unwrap_or(self.default_font.size),
                )
            })),
        })
    }

    /// Applies a toolbar command as one undo step that keeps the selection.
    pub fn format(
        &mut self,
        engine: &mut TextEngine,
        command: Formatting,
    ) -> Result<(), EditorError> {
        if self.page_selected() {
            return self.format_page(engine, command);
        }
        let outline = self.active_outline();
        let (id, title, selection) = (outline.id, outline.title, outline.selection);
        let (container, mut range, ends) = selected(&outline.document, selection)?;
        let mut replacement = outline.document.container(container)?[range.clone()].to_vec();
        let mut ranges = covered(&replacement, ends)
            .map(|(node, range)| (node.id, range))
            .collect::<BTreeMap<_, _>>();
        let bases = |nodes: &[PageParagraph]| {
            covered(nodes, ends)
                .map(|(node, _)| Ok((node.id, self.style_format(node.style)?)))
                .collect::<Result<BTreeMap<_, _>, EditError>>()
        };
        match &command {
            Formatting::Align(_)
            | Formatting::Bullets
            | Formatting::Numbering
            | Formatting::List(_)
            | Formatting::Tag(..)
            | Formatting::RemoveTags
            | Formatting::Check
            | Formatting::ToDoList(..)
            | Formatting::BulletedList(..)
            | Formatting::Style(_)
                if title =>
            {
                return Ok(());
            }
            Formatting::Indent | Formatting::Outdent => {
                self.indent(engine, command == Formatting::Outdent)?;
                return Ok(());
            }
            Formatting::Align(alignment) => {
                let bases = bases(&replacement)?;
                for node in leaves_mut(&mut replacement) {
                    if ranges.remove(&node.id).is_some() {
                        let value = match alignment {
                            Alignment::Left => bases[&node.id].alignment.map(|_| 0),
                            Alignment::Center => Some(1),
                            Alignment::Right => Some(2),
                        };
                        let text = &mut node.text_mut().unwrap().text;
                        *text = restyle(text, 0..text.text().len(), |format| {
                            format.alignment = value;
                        });
                    }
                }
            }
            Formatting::Bullets | Formatting::Numbering | Formatting::List(_) => {
                let style = match command {
                    Formatting::Bullets => Some(ListStyle::BULLET),
                    Formatting::Numbering => Some(ListStyle::NUMBER),
                    Formatting::List(style) => style,
                    _ => unreachable!(),
                };
                // A toggle leaves paragraphs already of its kind as they are, however styled.
                let has = |node: &PageParagraph| match command {
                    Formatting::List(_) => self.list_style(node) == style,
                    _ => self.list(node).as_ref() == Some(&command),
                };
                let remove = style.is_none()
                    || !matches!(command, Formatting::List(_))
                        && covered(&replacement, ends).all(|(node, _)| has(node));
                // A list applied after a plain sibling nests under it, as Tab would
                // (`evidence/structural-edits/xml/pb-1.xml`); removing it, or restyling a
                // list, leaves it where it is, as OneNote 2010 does.
                let nodes = outline.document.container(container)?;
                if !remove
                    && nodes[range.start].lists.is_empty()
                    && crate::document::previous_sibling(nodes, range.start, &BTreeSet::new())
                        .is_some_and(|sibling| nodes[sibling].lists.is_empty())
                    && let Some(edit) =
                        crate::document::indent(nodes, container, range.clone(), false)
                {
                    range = edit.range;
                    replacement = edit.replacement;
                }
                let wanted = covered(&replacement, ends)
                    .map(|(node, _)| node)
                    .filter(|node| !remove && !has(node))
                    .filter_map(|node| {
                        let format = &node.text().unwrap().text.spans()[0].format;
                        Some((node.id, list_definition(style?, format)))
                    })
                    .collect::<Vec<_>>();
                // A list definition belongs to one paragraph, as OneNote stores it.
                let mut lists = BTreeMap::new();
                for (node, definition) in wanted {
                    let id = new_id()?;
                    self.definitions.insert(id, definition);
                    lists.insert(node, id);
                }
                for node in leaves_mut(&mut replacement) {
                    if ranges.remove(&node.id).is_some() {
                        if remove {
                            node.lists.clear();
                        } else if let Some(id) = lists.get(&node.id) {
                            node.lists = vec![*id];
                        }
                    }
                }
            }
            Formatting::Tag(tag, action_type) => {
                let definition = tag.definition(*action_type);
                let id = self.define_tag(&definition)?;
                let has = |node: &PageParagraph| {
                    tags(node).any(|tag| self.tag_kind(tag) == Some(&definition.kind))
                };
                let remove = covered(&replacement, ends).all(|(node, _)| has(node));
                let created = time32();
                for node in leaves_mut(&mut replacement) {
                    if ranges.remove(&node.id).is_none() || !remove && has(node) {
                        continue;
                    }
                    let ParagraphContent::Text(text) = &mut node.content else {
                        unreachable!()
                    };
                    let added = (!remove).then_some((id, tag.shape, created));
                    self.retag([&mut node.tags, &mut text.tags], &definition.kind, added);
                }
            }
            Formatting::RemoveTags => {
                for node in leaves_mut(&mut replacement) {
                    if ranges.remove(&node.id).is_some() {
                        node.tags.clear();
                        node.text_mut().unwrap().tags.clear();
                    }
                }
            }
            Formatting::Check => self.check(&mut replacement, &ranges),
            Formatting::ToDoList(tag, action_type) => {
                let definition = tag.definition(*action_type);
                let id = self.define_tag(&definition)?;
                let created = time32();
                for node in leaves_mut(&mut replacement) {
                    if ranges.remove(&node.id).is_none() || self.list(node).is_none() {
                        continue;
                    }
                    node.lists.clear();
                    if tags(node).any(|tag| self.tag_kind(tag) == Some(&definition.kind)) {
                        continue;
                    }
                    let ParagraphContent::Text(text) = &mut node.content else {
                        unreachable!()
                    };
                    let added = Some((id, tag.shape, created));
                    self.retag([&mut node.tags, &mut text.tags], &definition.kind, added);
                }
            }
            Formatting::BulletedList(tag, action_type) => {
                let kind = tag.definition(*action_type).kind;
                for node in leaves_mut(&mut replacement) {
                    if ranges.remove(&node.id).is_none()
                        || !tags(node).any(|tag| self.tag_kind(tag) == Some(&kind))
                    {
                        continue;
                    }
                    if self.list(node) != Some(Formatting::Bullets) {
                        let format = &node.text().unwrap().text.spans()[0].format;
                        let list = list_definition(ListStyle::BULLET, format);
                        let id = new_id()?;
                        self.definitions.insert(id, list);
                        node.lists = vec![id];
                    }
                    let ParagraphContent::Text(text) = &mut node.content else {
                        unreachable!()
                    };
                    self.retag([&mut node.tags, &mut text.tags], &kind, None);
                }
            }
            Formatting::Style(definition) => {
                let olds = bases(&replacement)?;
                let style = self.define_style(definition)?;
                for node in leaves_mut(&mut replacement) {
                    if ranges.remove(&node.id).is_some() {
                        node.style = Some(style);
                        let old = &olds[&node.id];
                        let text = &mut node.text_mut().unwrap().text;
                        *text = styled(text, old, definition);
                    }
                }
            }
            Formatting::Toggle(_)
            | Formatting::Font(_)
            | Formatting::FontSize(_)
            | Formatting::Color(_)
            | Formatting::Highlight(_)
            | Formatting::Paint(_)
            | Formatting::Clear => {
                let caret = (ends[0] == ends[1])
                    .then(|| {
                        let [anchor, focus] = selection.positions;
                        let caret = anchor.min(focus);
                        Ok::<_, EditError>((caret, self.typing_format(caret)?))
                    })
                    .transpose()?;
                let on = match command {
                    Formatting::Toggle(toggle) => !match &caret {
                        Some((_, format)) => vec![format],
                        None => character_formats(covered(&replacement, ends)),
                    }
                    .iter()
                    .all(|format| toggle.get(format) == Some(true)),
                    _ => true,
                };
                let change = |base: &Format, format: &mut Format| match &command {
                    Formatting::Toggle(toggle) => {
                        let exclusive = match toggle {
                            Toggle::Subscript => Some(Toggle::Superscript),
                            Toggle::Superscript => Some(Toggle::Subscript),
                            _ => None,
                        };
                        for toggle in exclusive.into_iter().chain([*toggle]) {
                            *toggle.slot(format) = toggle.get(base).map(|_| false);
                        }
                        if on {
                            *toggle.slot(format) = Some(true);
                        }
                    }
                    Formatting::Font(font) => format.font = Some(font.clone()),
                    Formatting::FontSize(size) => format.font_size = Some(*size),
                    Formatting::Color(color) => {
                        format.color = color.or(base.color.map(|_| AUTOMATIC));
                    }
                    Formatting::Highlight(color) => {
                        format.highlight = color.or(base.highlight.map(|_| AUTOMATIC));
                    }
                    Formatting::Paint(painted) => {
                        for toggle in Toggle::ALL {
                            *toggle.slot(format) = toggle.get(painted).or(toggle.get(base));
                        }
                        format.font = painted.font.clone().or_else(|| base.font.clone());
                        format.font_size = painted.font_size.or(base.font_size);
                        format.color = painted.color.or(base.color);
                        format.highlight = painted.highlight.or(base.highlight);
                    }
                    _ => {
                        for toggle in Toggle::ALL {
                            *toggle.slot(format) = toggle.get(base);
                        }
                        format.font.clone_from(&base.font);
                        format.font_size = base.font_size;
                        format.color = base.color;
                        format.highlight = base.highlight;
                    }
                };
                let bases = bases(&replacement)?;
                if let Some((caret, mut format)) = caret {
                    change(&bases[&ends[0].0], &mut format);
                    self.pending = Some((id, caret, format));
                    return Ok(());
                }
                for node in leaves_mut(&mut replacement) {
                    if let Some(range) = ranges.remove(&node.id) {
                        let base = &bases[&node.id];
                        let text = &mut node.text_mut().unwrap().text;
                        *text = restyle(text, range, |format| change(base, format));
                        if let Formatting::Paint(painted) = &command
                            && !title
                        {
                            *text = restyle(text, 0..text.text().len(), |format| {
                                format.alignment = painted.alignment;
                            });
                        }
                    }
                }
            }
        }
        if self.active_outline().document.container(container)?[range.clone()] == replacement[..] {
            return Ok(());
        }
        self.commit(
            engine,
            DocumentEdit {
                columns: BTreeMap::new(),
                container,
                range,
                replacement,
            },
            selection,
        )
    }
}

impl CanvasEditor {
    /// The page's paragraph style holding `definition`, made where it has none.
    pub(super) fn define_style(&mut self, definition: &Definition) -> Result<ExGuid, EditError> {
        if let Some((id, _)) = self
            .definitions
            .iter()
            .find(|(_, kept)| *kept == definition)
        {
            return Ok(*id);
        }
        let id = new_id()?;
        self.definitions.insert(id, definition.clone());
        Ok(id)
    }

    /// The page's definition of the tag `definition` describes, made where it has none.
    pub(super) fn define_tag(&mut self, definition: &Definition) -> Result<ExGuid, EditError> {
        if let Some((id, _)) = self
            .definitions
            .iter()
            .find(|(_, other)| other.kind == definition.kind)
        {
            return Ok(*id);
        }
        let id = new_id()?;
        self.definitions.insert(id, definition.clone());
        Ok(id)
    }

    /// A paragraph style named `name`: the page's in its theme's formatting, else the page's
    /// first of that name, else the theme's made on the page; Normal falls back to the
    /// Default font's. None where nothing names it.
    pub(super) fn style_named(&mut self, name: &str) -> Result<Option<ExGuid>, EditError> {
        let themed = self.styles.get(name).cloned();
        let named = |definition: &Definition| matches!(&definition.kind, Kind::Style { name: Some(own), .. } if own == name);
        let known = self
            .definitions
            .iter()
            .filter(|(_, kept)| named(kept))
            .min_by_key(|(_, kept)| Some(*kept) != themed.as_ref())
            .map(|(id, _)| *id);
        if known.is_some() {
            return Ok(known);
        }
        let definition = themed.or_else(|| (name == "p").then(|| self.default_font.body_style()));
        definition
            .map(|definition| self.define_style(&definition))
            .transpose()
    }
}

impl CanvasEditor {
    /// A toolbar command on OneNote 2010's page selection: each outline takes it as if selected
    /// alone, but a toggle turns on everywhere unless every outline already has it.
    fn format_page(
        &mut self,
        engine: &mut TextEngine,
        command: Formatting,
    ) -> Result<(), EditorError> {
        self.whole = None;
        let has = |state: FormatState| match &command {
            Formatting::Toggle(toggle) => state.toggles.contains(toggle),
            Formatting::Bullets => state.bullets,
            Formatting::Numbering => state.numbering,
            Formatting::Tag(tag, action_type) => state.tags.contains(&(tag.stored(), *action_type)),
            _ => false,
        };
        let mut outlines = Vec::new();
        for outline in self.outlines.iter().filter(|outline| !outline.title) {
            let state = self.format_state_of(outline, outline.whole())?;
            outlines.push((outline.id, has(state)));
        }
        let everywhere = outlines.iter().all(|(_, has)| *has);
        self.grouped(|editor| {
            for (id, has) in outlines {
                if has && !everywhere {
                    continue;
                }
                editor.focus_outline(id)?;
                editor.select_all()?;
                editor.format(engine, command.clone())?;
            }
            Ok(editor.select_page()?)
        })
    }

    /// A click on a check box tag: checks the check boxes of paragraph `id` in outline
    /// `outline`, or clears them once all are checked, as one undo step that keeps the
    /// selection.
    pub fn click_check(
        &mut self,
        engine: &mut TextEngine,
        outline: ExGuid,
        id: ExGuid,
    ) -> Result<(), EditorError> {
        self.focus_outline(outline)?;
        let outline = self.active_outline();
        let document = &outline.document;
        let Some(paragraph) = leaves(document.nodes(), None).position(|(_, _, node)| node.id == id)
        else {
            return self.click_block_check(engine, id);
        };
        let at = TextPosition {
            paragraph,
            offset: 0,
        };
        let (container, range, ends) = selected(document, [at; 2].into())?;
        let mut replacement = document.container(container)?[range.clone()].to_vec();
        let ranges = covered(&replacement, ends)
            .map(|(node, range)| (node.id, range))
            .collect();
        self.check(&mut replacement, &ranges);
        let selection = outline.selection;
        self.commit(
            engine,
            DocumentEdit {
                columns: BTreeMap::new(),
                container,
                range,
                replacement,
            },
            selection,
        )
    }

    /// A click on the check box of a table, picture or file paragraph `id`.
    fn click_block_check(
        &mut self,
        engine: &mut TextEngine,
        id: ExGuid,
    ) -> Result<(), EditorError> {
        let outline = self.active_outline();
        let (container, index, node) = descendants(outline.document.nodes(), None)
            .find(|(_, _, node)| node.id == id)
            .ok_or(EditError::InvalidRange)?;
        let mut node = node.clone();
        let content = match &mut node.content {
            ParagraphContent::Table(table) => table.tags.as_mut_slice(),
            ParagraphContent::Image(image) => image.tags.as_mut_slice(),
            ParagraphContent::Attachment(file) => file.tags.as_mut_slice(),
            _ => &mut [],
        };
        self.toggle_checks(node.tags.iter_mut().chain(content).collect());
        let selection = outline.selection;
        self.commit(
            engine,
            DocumentEdit {
                columns: BTreeMap::new(),
                container,
                range: index..index + 1,
                replacement: vec![node],
            },
            selection,
        )
    }

    /// A tag command on selected picture or file `id`, which OneNote 2010 tags itself rather
    /// than its paragraph (`corpus/object-tags`); false for other commands.
    pub fn format_object(
        &mut self,
        engine: &mut TextEngine,
        id: ExGuid,
        command: &Formatting,
    ) -> Result<bool, EditorError> {
        if !matches!(
            command,
            Formatting::Tag(..) | Formatting::RemoveTags | Formatting::Check
        ) {
            return Ok(false);
        }
        let node = self
            .outline_picture(id)
            .map(|(outline, _, _, node)| (outline, node.clone()));
        let mut tags = match &node {
            Some((_, node)) => match &node.content {
                ParagraphContent::Image(image) => image.tags.clone(),
                ParagraphContent::Attachment(file) => file.tags.clone(),
                _ => return Ok(false),
            },
            None => self
                .object_tags_mut(id)
                .ok_or(EditError::InvalidRange)?
                .clone(),
        };
        match command {
            Formatting::Tag(tag, action_type) => {
                let definition = tag.definition(*action_type);
                let defined = self.define_tag(&definition)?;
                let has = tags
                    .iter()
                    .any(|tag| self.tag_kind(tag) == Some(&definition.kind));
                let added = (!has).then_some((defined, tag.shape, time32()));
                self.retag([&mut tags], &definition.kind, added);
            }
            Formatting::RemoveTags => tags.clear(),
            _ => self.toggle_checks(tags.iter_mut().collect()),
        }
        self.finish_composition();
        let Some((outline, mut node)) = node else {
            self.set_object_tags(id, tags);
            return Ok(true);
        };
        match &mut node.content {
            ParagraphContent::Image(image) => image.tags = tags,
            ParagraphContent::Attachment(file) => file.tags = tags,
            _ => unreachable!(),
        }
        self.focus_outline(outline)?;
        let (container, index, _) = descendants(self.active_outline().document.nodes(), None)
            .find(|(_, _, other)| other.id == node.id)
            .ok_or(EditError::InvalidRange)?;
        let selection = self.active_outline().selection;
        self.commit(
            engine,
            DocumentEdit {
                columns: BTreeMap::new(),
                container,
                range: index..index + 1,
                replacement: vec![node],
            },
            selection,
        )?;
        Ok(true)
    }

    /// A click on the check box of picture or file `id` on the page.
    pub fn click_object_check(&mut self, id: ExGuid) -> Result<(), EditorError> {
        let mut tags = self
            .objects
            .iter()
            .find_map(|object| object.tagged().filter(|tagged| tagged.0 == id))
            .ok_or(EditError::InvalidRange)?
            .1
            .to_vec();
        self.toggle_checks(tags.iter_mut().collect());
        self.finish_composition();
        self.set_object_tags(id, tags);
        Ok(())
    }

    /// Takes tags of `kind` from `lists`, or with `added`, the definition's identity, its
    /// shape and when, gives the last list one in place of any of its action type; an element
    /// holds one tag of each action type, stored newest first.
    pub(super) fn retag<const N: usize>(
        &self,
        mut lists: [&mut Vec<Tag>; N],
        kind: &Kind<'static>,
        added: Option<(ExGuid, u16, Option<u32>)>,
    ) {
        let Kind::TagDefinition { action_type, .. } = kind else {
            return;
        };
        for tags in lists.iter_mut() {
            tags.retain(|tag| match self.tag_kind(tag) {
                Some(other) if added.is_none() => other != kind,
                Some(Kind::TagDefinition {
                    action_type: other, ..
                }) => other != action_type,
                _ => true,
            });
        }
        let (Some((id, shape, created)), Some(tags)) = (added, lists.last_mut()) else {
            return;
        };
        let checkable = crate::outline::checkable(shape);
        tags.insert(
            0,
            Tag {
                definition: Some(id),
                action_type: None,
                shape: None,
                property_status: None,
                status: u16::from(!checkable),
                created,
                completed: if checkable { Some(0) } else { created },
                start: None,
                due: None,
                task_id: None,
                extra_set: 0,
            },
        );
    }

    /// Checks the check boxes among `tags`, or clears them once all are checked; OneNote keeps
    /// a cleared box's completion time as zero.
    pub(super) fn toggle_checks(&self, mut tags: Vec<&mut Tag>) {
        let checked = tags
            .iter()
            .filter(|tag| self.check_box(tag).is_some())
            .all(|tag| tag.status & 1 != 0);
        let completed = if checked { Some(0) } else { time32() };
        for tag in &mut tags {
            if self.check_box(tag).is_some() && (tag.status & 1 != 0) == checked {
                tag.status ^= 1;
                tag.completed = completed;
            }
        }
    }

    /// The definition and shape of `tag` when it is a check box.
    fn check_box(&self, tag: &Tag) -> Option<(&Kind<'static>, u16)> {
        match self.tag_kind(tag)? {
            kind @ Kind::TagDefinition {
                shape: Some(shape), ..
            } if crate::outline::checkable(*shape) => Some((kind, *shape)),
            _ => None,
        }
    }

    /// Enter in a to-do list: `opened` takes an unchecked copy of each check box of `from`.
    pub(super) fn continue_checks(&self, from: &PageParagraph, opened: &mut PageParagraph) {
        let created = time32();
        let ParagraphContent::Text(text) = &mut opened.content else {
            unreachable!()
        };
        for tag in tags(from).collect::<Vec<_>>().into_iter().rev() {
            if let (Some((kind, shape)), Some(id)) = (self.check_box(tag), tag.definition) {
                self.retag(
                    [&mut opened.tags, &mut text.tags],
                    kind,
                    Some((id, shape, created)),
                );
            }
        }
    }

    /// Enter on an empty to-do item ends the list: its check boxes go.
    pub(super) fn end_checks(&self, node: &mut PageParagraph) {
        let ParagraphContent::Text(text) = &mut node.content else {
            unreachable!()
        };
        for tags in [&mut node.tags, &mut text.tags] {
            tags.retain(|tag| self.check_box(tag).is_none());
        }
    }

    /// Checks the check boxes of the `covered` paragraphs, or clears them once all are
    /// checked.
    fn check(&self, replacement: &mut [PageParagraph], covered: &BTreeMap<ExGuid, Range<usize>>) {
        let tags = leaves_mut(replacement)
            .into_iter()
            .filter(|node| covered.contains_key(&node.id))
            .flat_map(|node| {
                let ParagraphContent::Text(text) = &mut node.content else {
                    unreachable!()
                };
                node.tags.iter_mut().chain(&mut text.tags)
            })
            .collect();
        self.toggle_checks(tags);
    }
}

/// The list a Tab (`deeper`) or Shift+Tab gives a paragraph whose list is one OneNote 2010
/// steps through as Tab nests it: from Ctrl+. through nine bullets, and from Ctrl+/ through
/// eight number formats, each as its gallery stores it, starting over after the last
/// (`evidence/toolbar-17/tab-lists.txt`). Outdenting stops at the first.
pub(super) fn nested_list(definition: &Definition, deeper: bool) -> Option<Definition> {
    const BULLETS: [usize; 9] = [2, 3, 13, 14, 9, 7, 15, 26, 8];
    const NUMBERS: [usize; 8] = [0, 2, 4, 6, 7, 9, 16, 17];
    let Kind::List { restart, .. } = &definition.kind else {
        return None;
    };
    let current = ListStyle::of(&definition.kind)?;
    let (chain, style): (&[usize], fn(usize) -> ListStyle) = match current {
        ListStyle::Bullet(_) => (&BULLETS, ListStyle::Bullet),
        ListStyle::Number(_) => (&NUMBERS, ListStyle::Number),
    };
    let at = chain.iter().position(|place| style(*place) == current)?;
    let at = if deeper {
        (at + 1) % chain.len()
    } else {
        at.checked_sub(1)?
    };
    let mut kind = list_definition(style(chain[at]), &Format::default()).kind;
    if let Kind::List { restart: value, .. } = &mut kind {
        *value = *restart;
    }
    Some(Definition {
        kind,
        format: definition.format.clone(),
    })
}

/// The list OneNote 2010 gives a paragraph with `format` in library `style`, as its gallery
/// and its Ctrl+. bullet store it (`corpus/paragraph-edit/reconciliation/keyboard`); a number
/// also takes the text's font and language (`corpus/outline-edit/tree`).
pub(super) fn list_definition(style: ListStyle, format: &Format) -> Definition {
    let font_size = Some(format.font_size.unwrap_or(DEFAULT_FONT_SIZE));
    match style {
        ListStyle::Number(index) => Definition {
            kind: Kind::List {
                font: None,
                format: Some(NUMBER_LIBRARY[index].into()),
                restart: None,
                bullet: None,
            },
            format: Format {
                bold: Some(false),
                italic: Some(false),
                font: format.font.clone(),
                font_size,
                color: Some(AUTOMATIC),
                language: format.language,
                ..Format::default()
            },
        },
        ListStyle::Bullet(index) => {
            let (font, glyph, bullet) = BULLET_LIBRARY[index];
            Definition {
                kind: Kind::List {
                    font: Some(font.into()),
                    format: Some(glyph.into()),
                    restart: None,
                    bullet: Some(bullet),
                },
                format: Format {
                    font_size,
                    color: Some(AUTOMATIC),
                    ..Format::default()
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(paragraph: usize, offset: u32) -> TextPosition {
        TextPosition { paragraph, offset }
    }

    fn calibri() -> Format {
        Format {
            font: Some("Calibri".into()),
            font_size: Some(11.0),
            ..Format::default()
        }
    }

    fn editor(engine: &mut TextEngine, paragraphs: Vec<Paragraph>) -> CanvasEditor {
        CanvasEditor::new(engine, TextDocument::new(paragraphs).unwrap(), 300.0).unwrap()
    }

    fn plain(engine: &mut TextEngine, texts: &[&str]) -> CanvasEditor {
        let paragraphs = texts
            .iter()
            .map(|text| Paragraph::new((*text).into(), calibri()))
            .collect();
        editor(engine, paragraphs)
    }

    /// Each paragraph's runs as text and the value `field` reads from their format.
    fn runs<T>(editor: &CanvasEditor, field: impl Fn(&Format) -> T) -> Vec<Vec<(String, T)>> {
        editor
            .active_outline()
            .document
            .paragraphs()
            .map(|paragraph| {
                let mut start = 0;
                paragraph
                    .spans()
                    .iter()
                    .map(|span| {
                        let text = paragraph.text()[start..span.end].to_owned();
                        start = span.end;
                        (text, field(&span.format))
                    })
                    .collect()
            })
            .collect()
    }

    fn run(text: &str, value: Option<bool>) -> (String, Option<bool>) {
        (text.into(), value)
    }

    #[test]
    fn toggles_apply_unless_every_character_has_them_as_one_undo_step() {
        let mut engine = TextEngine::default();
        let mut editor = editor(
            &mut engine,
            vec![
                Paragraph::from_runs([
                    ("plain ".into(), calibri()),
                    (
                        "bold".into(),
                        Format {
                            bold: Some(true),
                            ..calibri()
                        },
                    ),
                ]),
                Paragraph::new("next".into(), calibri()),
            ],
        );
        let original = editor.active_outline().document.clone();
        let selection = Selection::from([at(1, 2), at(0, 3)]);
        editor.select(selection).unwrap();
        let bold = |editor: &CanvasEditor| runs(editor, |format| format.bold);
        editor
            .format(&mut engine, Formatting::Toggle(Toggle::Bold))
            .unwrap();
        assert_eq!(
            bold(&editor),
            [
                vec![run("pla", None), run("in bold", Some(true))],
                vec![run("ne", Some(true)), run("xt", None)]
            ]
        );
        assert_eq!(editor.selection(), selection);
        assert!(
            editor
                .format_state()
                .unwrap()
                .toggles
                .contains(&Toggle::Bold)
        );
        editor
            .format(&mut engine, Formatting::Toggle(Toggle::Bold))
            .unwrap();
        assert_eq!(
            bold(&editor),
            [vec![run("plain bold", None)], vec![run("next", None)]]
        );
        assert!(editor.undo(&mut engine).unwrap());
        assert!(editor.undo(&mut engine).unwrap());
        assert_eq!(editor.active_outline().document, original);
        assert_eq!(editor.selection(), selection);
        assert!(!editor.undo(&mut engine).unwrap());
        assert!(editor.redo(&mut engine).unwrap());
        assert_eq!(bold(&editor)[1], [run("ne", Some(true)), run("xt", None)]);
    }

    #[test]
    fn a_second_toggle_at_a_caret_turns_the_typing_format_back_off() {
        for toggle in Toggle::ALL {
            for text in ["", "ab"] {
                let mut engine = TextEngine::default();
                let mut editor = plain(&mut engine, &[text]);
                let caret = at(0, text.len() as u32);
                editor.select([caret; 2].into()).unwrap();
                let press = |editor: &mut CanvasEditor, engine: &mut TextEngine| {
                    editor.format(engine, Formatting::Toggle(toggle)).unwrap();
                    editor.format_state().unwrap().toggles
                };
                assert_eq!(press(&mut editor, &mut engine), [toggle], "{toggle:?}");
                assert_eq!(press(&mut editor, &mut engine), [], "{toggle:?}");
                editor.insert(&mut engine, "x").unwrap();
                assert_eq!(
                    runs(&editor, |format| toggle.get(format))[0],
                    [run(&format!("{text}x"), None)]
                );
            }
            let mut engine = TextEngine::default();
            let mut on = calibri();
            *toggle.slot(&mut on) = Some(true);
            let mut editor = editor(&mut engine, vec![Paragraph::new("ab".into(), on)]);
            editor.select([at(0, 1); 2].into()).unwrap();
            editor
                .format(&mut engine, Formatting::Toggle(toggle))
                .unwrap();
            assert_eq!(editor.format_state().unwrap().toggles, [], "{toggle:?}");
            editor.insert(&mut engine, "x").unwrap();
            assert_eq!(
                runs(&editor, |format| toggle.get(format) == Some(true))[0],
                [("a".into(), true), ("x".into(), false), ("b".into(), true)],
                "{toggle:?}"
            );
        }
    }

    #[test]
    fn a_caret_formats_the_text_typed_next_and_scripts_exclude_each_other() {
        let mut engine = TextEngine::default();
        let mut editor = plain(&mut engine, &["ab"]);
        editor.select([at(0, 1); 2].into()).unwrap();
        for toggle in [Toggle::Bold, Toggle::Superscript, Toggle::Subscript] {
            editor
                .format(&mut engine, Formatting::Toggle(toggle))
                .unwrap();
        }
        assert!(!editor.undo(&mut engine).unwrap());
        assert_eq!(
            editor.format_state().unwrap().toggles,
            [Toggle::Bold, Toggle::Subscript]
        );
        editor.insert(&mut engine, "x").unwrap();
        let typed = Format {
            bold: Some(true),
            subscript: Some(true),
            ..calibri()
        };
        assert_eq!(
            runs(&editor, Clone::clone),
            [vec![
                ("a".into(), calibri()),
                ("x".into(), typed.clone()),
                ("b".into(), calibri())
            ]]
        );
        editor
            .format(&mut engine, Formatting::Toggle(Toggle::Italic))
            .unwrap();
        editor.select([at(0, 0); 2].into()).unwrap();
        editor.insert(&mut engine, "y").unwrap();
        assert_eq!(runs(&editor, |format| format.italic)[0][0], run("ya", None));

        editor
            .format(&mut engine, Formatting::FontSize(20.0))
            .unwrap();
        editor.compose(&mut engine, "k".into(), 1..1).unwrap();
        editor.compose(&mut engine, "kl".into(), 2..2).unwrap();
        editor.commit_text(&mut engine, "kl".into()).unwrap();
        assert_eq!(
            runs(&editor, |format| format.font_size)[0][..2],
            [("y".into(), Some(11.0)), ("kl".into(), Some(20.0))]
        );
        editor.undo(&mut engine).unwrap();
        assert_eq!(
            editor
                .active_outline()
                .document
                .paragraphs()
                .next()
                .unwrap()
                .text(),
            "yaxb"
        );
    }

    /// A style from the gallery replaces the paragraph's character formatting but keeps its
    /// language, as OneNote 2010's does; Enter at its end takes its NextStyle, from the
    /// theme where the page has none (lab, 2026-09-30).
    #[test]
    fn a_gallery_style_clears_formatting_and_enter_takes_its_next_style() {
        let mut engine = TextEngine::default();
        let body = Format {
            font: Some("Calibri".into()),
            font_size: Some(11.0),
            language: Some(0x40c),
            ..Format::default()
        };
        let source = TextDocument::new(vec![Paragraph::from_runs([
            ("Plain ".to_owned(), body.clone()),
            (
                "red".to_owned(),
                Format {
                    color: Some(0xff),
                    italic: Some(true),
                    ..body.clone()
                },
            ),
        ])])
        .unwrap();
        let mut editor = CanvasEditor::new(&mut engine, source, 240.0).unwrap();
        let style = |name: &str, size, next: Option<&str>| Definition {
            kind: Kind::Style {
                name: Some(name.into()),
                next: next.map(Into::into),
            },
            format: Format {
                bold: Some(name != "p"),
                italic: Some(false),
                font: Some("Georgia".into()),
                font_size: Some(size),
                color: Some(0x0033_2211),
                space_before: Some(12.0),
                ..Format::default()
            },
        };
        let heading = style("h2", 16.0, Some("p"));
        editor.styles = BTreeMap::from([("p".to_owned(), style("p", 12.0, None))]);
        editor.select([at(0, 3); 2].into()).unwrap();
        editor
            .format(&mut engine, Formatting::Style(heading.clone()))
            .unwrap();
        let node = editor.active_outline().document.nodes()[0].clone();
        assert_eq!(editor.definitions[&node.style.unwrap()], heading);
        let spans = node.text().unwrap().text.spans().to_vec();
        assert_eq!(spans.len(), 1);
        assert_eq!(
            spans[0].format,
            Format {
                language: Some(0x40c),
                ..heading.format.clone()
            }
        );
        assert_eq!(editor.format_state().unwrap().style.as_deref(), Some("h2"));
        let length = node.text().unwrap().text.text().len() as u32;
        editor.select([at(0, length); 2].into()).unwrap();
        editor.enter(&mut engine, false).unwrap();
        let next = editor.active_outline().document.nodes()[1].style;
        assert_eq!(editor.definitions[&next.unwrap()], editor.styles["p"]);
        // Enter after Normal keeps Normal.
        editor.insert(&mut engine, "body").unwrap();
        editor.enter(&mut engine, false).unwrap();
        assert_eq!(editor.active_outline().document.nodes()[2].style, next);
        editor.undo(&mut engine).unwrap();
        editor.undo(&mut engine).unwrap();
        editor.undo(&mut engine).unwrap();
        editor.undo(&mut engine).unwrap();
        assert_eq!(editor.active_outline().document.nodes()[0].style, None);
    }

    #[test]
    fn clearing_and_removing_attributes_return_to_the_paragraph_style() {
        let mut engine = TextEngine::default();
        let style = new_id().unwrap();
        let heading = Format {
            bold: Some(true),
            color: Some(0x0012_3456),
            font: Some("Georgia".into()),
            font_size: Some(18.0),
            ..Format::default()
        };
        let mut nodes = TextDocument::new(vec![Paragraph::new("Heading".into(), heading.clone())])
            .unwrap()
            .nodes()
            .to_vec();
        nodes[0].style = Some(style);
        let outline = Outline {
            id: new_id().unwrap(),
            title: false,
            min_width: None,
            layout: onestore::document::Layout {
                max_width: Some(300.0),
                ..Default::default()
            },
            indents: vec![18.0, 0.0, 27.0, 27.0],
            paragraphs: nodes,
            unsupported: Vec::new(),
        };
        let definitions = BTreeMap::from([(
            style,
            Definition {
                kind: Kind::Style {
                    name: Some("h1".into()),
                    next: None,
                },
                format: heading.clone(),
            },
        )]);
        let mut editor =
            CanvasEditor::from_outlines(&mut engine, vec![outline], definitions).unwrap();
        editor.select_all().unwrap();
        let state = editor.format_state().unwrap();
        assert_eq!(state.toggles, [Toggle::Bold]);
        assert_eq!(
            (state.font.as_deref(), state.font_size, state.alignment),
            (Some("Georgia"), Some(18.0), Some(Alignment::Left))
        );
        for command in [
            Formatting::Toggle(Toggle::Bold),
            Formatting::Color(None),
            Formatting::Highlight(Some(0x0000_ffff)),
            Formatting::Font("Arial".into()),
            Formatting::Align(Alignment::Center),
        ] {
            editor.format(&mut engine, command).unwrap();
        }
        editor.select([at(0, 0), at(0, 4)].into()).unwrap();
        editor
            .format(&mut engine, Formatting::FontSize(9.0))
            .unwrap();
        editor.select_all().unwrap();
        assert_eq!(editor.format_state().unwrap().font_size, None);
        let format = editor
            .active_outline()
            .document
            .paragraph(0)
            .unwrap()
            .spans()[1]
            .format
            .clone();
        assert_eq!(
            format,
            Format {
                bold: Some(false),
                color: Some(AUTOMATIC),
                highlight: Some(0x0000_ffff),
                font: Some("Arial".into()),
                alignment: Some(1),
                ..heading.clone()
            }
        );
        editor.format(&mut engine, Formatting::Clear).unwrap();
        assert_eq!(
            runs(&editor, Clone::clone),
            [vec![(
                "Heading".into(),
                Format {
                    alignment: Some(1),
                    ..heading
                }
            )]]
        );
        editor
            .format(&mut engine, Formatting::Highlight(None))
            .unwrap();
        for _ in 0..7 {
            assert!(editor.undo(&mut engine).unwrap());
        }
        assert!(!editor.undo(&mut engine).unwrap());
    }

    /// Each paragraph's number as laid out, checked against laying the outline out afresh.
    fn numbers(engine: &mut TextEngine, editor: &CanvasEditor) -> Vec<Option<u32>> {
        let outline = editor.active_outline();
        let fresh = OutlineLayout::flow(
            outline.document.nodes().iter(),
            &outline.indents,
            outline.wrap_width(),
            true,
            0,
            None,
            &mut |node, previous, width, indents| {
                ParagraphLayout::shape(engine, node, previous, width, indents, &editor.definitions)
            },
        )
        .unwrap();
        let laid = |layout: &OutlineLayout| {
            layout
                .paragraphs
                .iter()
                .map(|paragraph| {
                    let markers = paragraph.markers.iter().map(|(marker, origin)| {
                        (*origin, marker.lines().next().unwrap().0.metrics().advance)
                    });
                    (paragraph.number.clone(), markers.collect::<Vec<_>>())
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(laid(&outline.shaped), laid(&fresh));
        outline
            .shaped
            .paragraphs
            .iter()
            .map(|paragraph| paragraph.number.as_ref().map(|(count, _)| count.number))
            .collect()
    }

    #[test]
    fn lists_toggle_switch_kind_and_renumber_only_what_changes() {
        let mut engine = TextEngine::default();
        let mut editor = plain(&mut engine, &["one", "two", "three", "four"]);
        let original = editor.active_outline().document.clone();
        let all = Selection::from([at(0, 0), at(3, 4)]);
        editor.select(all).unwrap();
        editor.format(&mut engine, Formatting::Bullets).unwrap();
        let state = editor.format_state().unwrap();
        assert!(state.bullets && !state.numbering);
        assert_eq!(numbers(&mut engine, &editor), [None; 4]);
        let bullet = editor.active_outline().document.nodes()[0].lists[0];
        assert_eq!(
            editor.definitions[&bullet],
            list_definition(ListStyle::BULLET, &calibri())
        );
        editor.format(&mut engine, Formatting::Numbering).unwrap();
        let state = editor.format_state().unwrap();
        assert!(!state.bullets && state.numbering);
        assert_eq!(
            numbers(&mut engine, &editor),
            [Some(1), Some(2), Some(3), Some(4)]
        );
        let untouched = editor.active_outline().shaped.paragraphs[0]
            .text
            .shaped
            .styles()
            .as_ptr();
        editor.select([at(1, 1); 2].into()).unwrap();
        editor.format(&mut engine, Formatting::Numbering).unwrap();
        // An unnumbered sibling leaves the count alone (`evidence/structural-edits/xml/c8-bs-1.xml`).
        assert_eq!(
            numbers(&mut engine, &editor),
            [Some(1), None, Some(2), Some(3)]
        );
        assert_eq!(
            editor.active_outline().shaped.paragraphs[0]
                .text
                .shaped
                .styles()
                .as_ptr(),
            untouched
        );
        editor.undo(&mut engine).unwrap();
        editor.select([at(2, 0); 2].into()).unwrap();
        editor.format(&mut engine, Formatting::Indent).unwrap();
        assert_eq!(
            numbers(&mut engine, &editor),
            [Some(1), Some(2), Some(1), Some(3)]
        );
        assert_eq!(editor.selection(), Selection::from([at(2, 0); 2]));
        editor.format(&mut engine, Formatting::Bullets).unwrap();
        assert_eq!(
            numbers(&mut engine, &editor),
            [Some(1), Some(2), None, Some(3)]
        );
        assert!(editor.format_state().unwrap().bullets);
        editor.format(&mut engine, Formatting::Outdent).unwrap();
        assert_eq!(
            numbers(&mut engine, &editor),
            [Some(1), Some(2), None, Some(3)]
        );
        editor.select(all).unwrap();
        let state = editor.format_state().unwrap();
        assert!(!state.bullets && !state.numbering);
        for _ in 0..6 {
            editor.undo(&mut engine).unwrap();
        }
        assert_eq!(editor.active_outline().document, original);
        assert_eq!(numbers(&mut engine, &editor), [None; 4]);
    }

    #[test]
    fn library_styles_replace_any_list_and_none_removes_them() {
        let mut engine = TextEngine::default();
        let mut editor = plain(&mut engine, &["one", "two", "three"]);
        let original = editor.active_outline().document.clone();
        let all = Selection::from([at(0, 0), at(2, 5)]);
        editor.select(all).unwrap();
        editor.format(&mut engine, Formatting::Bullets).unwrap();
        assert_eq!(editor.format_state().unwrap().list, Some(ListStyle::BULLET));
        let pick = |style| Formatting::List(Some(style));
        editor
            .format(&mut engine, pick(ListStyle::Bullet(0)))
            .unwrap();
        let state = editor.format_state().unwrap();
        assert!(state.bullets && !state.numbering);
        assert_eq!(state.list, Some(ListStyle::Bullet(0)));
        let first = editor.active_outline().document.nodes()[0].lists[0];
        assert_eq!(
            editor.definitions[&first],
            list_definition(ListStyle::Bullet(0), &calibri())
        );
        // Picking the style again leaves the lists as they are.
        let stored = editor.active_outline().document.clone();
        editor
            .format(&mut engine, pick(ListStyle::Bullet(0)))
            .unwrap();
        assert_eq!(editor.active_outline().document, stored);
        // A number format replaces the bullets; "First." lays out as any number does.
        editor
            .format(&mut engine, pick(ListStyle::Number(17)))
            .unwrap();
        let state = editor.format_state().unwrap();
        assert!(state.numbering && !state.bullets);
        assert_eq!(state.list, Some(ListStyle::Number(17)));
        assert_eq!(numbers(&mut engine, &editor), [Some(1), Some(2), Some(3)]);
        // Paragraphs of different styles share none.
        editor.select([at(1, 0); 2].into()).unwrap();
        editor
            .format(&mut engine, pick(ListStyle::Number(0)))
            .unwrap();
        editor.select(all).unwrap();
        let state = editor.format_state().unwrap();
        assert!(state.numbering && state.list.is_none());
        editor.format(&mut engine, Formatting::List(None)).unwrap();
        let state = editor.format_state().unwrap();
        assert!(!state.bullets && !state.numbering && state.list.is_none());
        assert_eq!(numbers(&mut engine, &editor), [None; 3]);
        for _ in 0..6 {
            editor.undo(&mut engine).unwrap();
        }
        assert_eq!(editor.active_outline().document, original);
    }

    #[test]
    fn restyling_a_list_after_a_plain_sibling_leaves_it_where_it_is() {
        let mut engine = TextEngine::default();
        let mut editor = plain(&mut engine, &["head", "item"]);
        let levels = |editor: &CanvasEditor| {
            editor
                .active_outline()
                .document
                .nodes()
                .iter()
                .map(|node| node.level)
                .collect::<Vec<_>>()
        };
        editor.select([at(1, 0); 2].into()).unwrap();
        editor.format(&mut engine, Formatting::Numbering).unwrap();
        assert_eq!(levels(&editor), [1, 2]);
        editor.format(&mut engine, Formatting::Outdent).unwrap();
        assert_eq!(levels(&editor), [1, 1]);
        editor.format(&mut engine, Formatting::Bullets).unwrap();
        editor
            .format(&mut engine, Formatting::List(Some(ListStyle::Bullet(0))))
            .unwrap();
        assert_eq!(levels(&editor), [1, 1]);
        assert_eq!(
            editor.format_state().unwrap().list,
            Some(ListStyle::Bullet(0))
        );
    }

    #[test]
    fn tab_steps_through_onenotes_nested_bullets_and_numbers() {
        let chain = |start: ListStyle, steps: usize| {
            let mut definition = list_definition(start, &calibri());
            let mut styles = vec![ListStyle::of(&definition.kind).unwrap()];
            for _ in 0..steps {
                definition = nested_list(&definition, true).unwrap();
                styles.push(ListStyle::of(&definition.kind).unwrap());
            }
            (styles, definition)
        };
        let (bullets, deepest) = chain(ListStyle::BULLET, 9);
        assert_eq!(
            bullets,
            [2, 3, 13, 14, 9, 7, 15, 26, 8, 2].map(ListStyle::Bullet)
        );
        // Wingdings § is ListMSAAIndex 9 here, as the gallery stores it.
        assert_eq!(BULLET_LIBRARY[13], ("Wingdings", "\u{a7}", 9));
        assert_eq!(
            deepest.format,
            list_definition(ListStyle::BULLET, &calibri()).format
        );
        let (numbers, _) = chain(ListStyle::NUMBER, 8);
        assert_eq!(
            numbers,
            [0, 2, 4, 6, 7, 9, 16, 17, 0].map(ListStyle::Number)
        );
        let second = nested_list(&list_definition(ListStyle::NUMBER, &calibri()), true).unwrap();
        assert_eq!(
            ListStyle::of(&nested_list(&second, false).unwrap().kind),
            Some(ListStyle::NUMBER)
        );
        assert!(nested_list(&list_definition(ListStyle::BULLET, &calibri()), false).is_none());
        // A style off both paths keeps its list.
        assert!(nested_list(&list_definition(ListStyle::Bullet(33), &calibri()), true).is_none());
    }

    #[test]
    fn every_library_style_reads_back_as_itself() {
        let styles = (0..BULLET_LIBRARY.len())
            .map(ListStyle::Bullet)
            .chain((0..NUMBER_LIBRARY.len()).map(ListStyle::Number));
        for style in styles {
            let definition = list_definition(style, &calibri());
            assert_eq!(ListStyle::of(&definition.kind), Some(style));
        }
    }

    #[test]
    fn numbering_follows_stored_sequences_restarts_and_nesting() {
        let mut engine = TextEngine::default();
        let mut editor = plain(&mut engine, &["a", "b", "c", "d", "e"]);
        let mut nodes = editor.active_outline().document.nodes().to_vec();
        for (node, (sequence, restart, level)) in nodes.iter_mut().zip([
            ('\u{1}', None, 1),
            ('\u{1}', None, 1),
            ('\u{4}', None, 2),
            ('\u{1}', Some(9), 1),
            ('\u{1}', None, 1),
        ]) {
            let id = new_id().unwrap();
            let mut definition = list_definition(ListStyle::NUMBER, &calibri());
            let Kind::List {
                format,
                restart: value,
                ..
            } = &mut definition.kind
            else {
                unreachable!()
            };
            *format = Some(format!("(\u{fffd}{sequence})"));
            *value = restart;
            editor.definitions.insert(id, definition);
            node.lists = vec![id];
            node.level = level;
        }
        let outline = Outline {
            paragraphs: nodes,
            ..editor.active_outline().snapshot()
        };
        let editor =
            CanvasEditor::from_outlines(&mut engine, vec![outline], editor.definitions).unwrap();
        assert_eq!(
            numbers(&mut engine, &editor),
            [Some(1), Some(2), Some(1), Some(9), Some(10)]
        );
    }

    /// OneNote 2010's numbers for siblings and children in the formats and levels given, as
    /// its COM interface reports them (`evidence/toolbar-17/restart.txt`); `None` is a plain
    /// paragraph, `Some("•")` a bullet.
    #[test]
    fn numbers_restart_where_the_format_changes() {
        let numbered = |paragraphs: &[(Option<&str>, u32)]| {
            let mut engine = TextEngine::default();
            let texts = vec!["x"; paragraphs.len()];
            let mut editor = plain(&mut engine, &texts);
            let mut nodes = editor.active_outline().document.nodes().to_vec();
            for (node, (list, level)) in nodes.iter_mut().zip(paragraphs) {
                node.level = *level;
                let style = match list {
                    None => continue,
                    Some("•") => ListStyle::BULLET,
                    Some(_) => ListStyle::NUMBER,
                };
                let mut definition = list_definition(style, &calibri());
                if let (Some(value), Kind::List { format, .. }) = (list, &mut definition.kind)
                    && style == ListStyle::NUMBER
                {
                    *format = Some((*value).into());
                }
                let id = new_id().unwrap();
                editor.definitions.insert(id, definition);
                node.lists = vec![id];
            }
            let outline = Outline {
                paragraphs: nodes,
                ..editor.active_outline().snapshot()
            };
            let editor =
                CanvasEditor::from_outlines(&mut engine, vec![outline], editor.definitions)
                    .unwrap();
            numbers(&mut engine, &editor)
        };
        let [dot, paren, letter] = ["\u{fffd}\u{0}.", "\u{fffd}\u{0})", "\u{fffd}\u{4}."];
        // Another format starts again, and so does the first format after it.
        assert_eq!(
            numbered(&[
                (Some(dot), 1),
                (Some(dot), 1),
                (Some(paren), 1),
                (Some(dot), 1),
                (Some(paren), 1),
            ]),
            [Some(1), Some(2), Some(1), Some(1), Some(1)]
        );
        // Plain and bulleted siblings leave the count alone.
        assert_eq!(
            numbered(&[(Some(dot), 1), (None, 1), (Some("•"), 1), (Some(dot), 1)]),
            [Some(1), None, None, Some(2)]
        );
        // So does another sequence; children count apart from their parents, afresh under each.
        assert_eq!(
            numbered(&[(Some(dot), 1), (Some(letter), 1), (Some(dot), 1)]),
            [Some(1), Some(1), Some(1)]
        );
        assert_eq!(
            numbered(&[
                (Some(dot), 1),
                (Some(dot), 2),
                (Some(dot), 2),
                (Some(dot), 1),
                (Some(dot), 2),
                (None, 1),
                (Some(dot), 1),
            ]),
            [Some(1), Some(1), Some(2), Some(2), Some(1), None, Some(3)]
        );
    }

    fn text_tags(editor: &CanvasEditor, paragraph: usize) -> Vec<(String, u16, bool)> {
        let (_, _, node) = editor.active_outline().document.leaf(paragraph).unwrap();
        tags(node)
            .map(|tag| {
                let Some(Kind::TagDefinition { label, .. }) = editor.tag_kind(tag) else {
                    panic!("{tag:?}")
                };
                let settled = tag.completed
                    == if tag.status & 1 == 0 {
                        Some(0)
                    } else {
                        tag.created
                    };
                (
                    label.clone().unwrap(),
                    tag.status,
                    settled || tag.status & 1 == 1,
                )
            })
            .collect()
    }

    /// OneNote 2010 checks a clicked To Do box (status 1, completion time now) and clears it
    /// (status 0, completion time kept as zero) without moving the caret.
    #[test]
    fn clicking_a_check_box_checks_and_clears_it_as_one_step() {
        let mut engine = TextEngine::default();
        let mut editor = plain(&mut engine, &["task", "other"]);
        editor.select([at(0, 0), at(1, 5)].into()).unwrap();
        editor
            .format(
                &mut engine,
                Formatting::Tag(NoteTag::defaults()[0].clone(), 0),
            )
            .unwrap();
        editor.select([at(1, 2); 2].into()).unwrap();
        let outline = editor.active_outline().id;
        let task = editor.active_outline().document.nodes()[0].id;
        let selection = editor.selection();
        editor.click_check(&mut engine, outline, task).unwrap();
        assert_eq!(editor.selection(), selection);
        assert_eq!(text_tags(&editor, 0), [("To Do".into(), 1, true)]);
        assert_eq!(text_tags(&editor, 1), [("To Do".into(), 0, true)]);
        let tag = |editor: &CanvasEditor| {
            editor.active_outline().document.nodes()[0]
                .text()
                .unwrap()
                .tags[0]
                .clone()
        };
        assert!(tag(&editor).completed.is_some_and(|time| time > 0));
        editor.click_check(&mut engine, outline, task).unwrap();
        assert_eq!((tag(&editor).status, tag(&editor).completed), (0, Some(0)));
        editor.undo(&mut engine).unwrap();
        assert_eq!(tag(&editor).status, 1);
    }

    #[test]
    fn tags_toggle_check_and_replace_tags_of_their_action_type() {
        let mut engine = TextEngine::default();
        let mut editor = plain(&mut engine, &["task", "other", "music"]);
        let music = new_id().unwrap();
        editor.definitions.insert(
            music,
            Definition {
                kind: Kind::TagDefinition {
                    label: Some("Music".into()),
                    action_type: Some(2),
                    shape: Some(121),
                    color: None,
                    highlight: None,
                },
                format: Format::default(),
            },
        );
        let mut nodes = editor.active_outline().document.nodes().to_vec();
        nodes[2].tags.push(Tag {
            definition: Some(music),
            action_type: None,
            shape: None,
            property_status: None,
            status: 1,
            created: Some(1),
            completed: Some(1),
            start: None,
            due: None,
            task_id: None,
            extra_set: 0,
        });
        let outline = Outline {
            paragraphs: nodes,
            ..editor.active_outline().snapshot()
        };
        let mut editor =
            CanvasEditor::from_outlines(&mut engine, vec![outline], editor.definitions).unwrap();
        let original = editor.active_outline().document.clone();
        editor.select([at(0, 1), at(1, 2)].into()).unwrap();
        editor
            .format(
                &mut engine,
                Formatting::Tag(NoteTag::defaults()[0].clone(), 0),
            )
            .unwrap();
        assert_eq!(text_tags(&editor, 0), [("To Do".into(), 0, true)]);
        assert_eq!(text_tags(&editor, 1), [("To Do".into(), 0, true)]);
        assert_eq!(
            editor.format_state().unwrap().tags,
            [(NoteTag::defaults()[0].clone(), 0)]
        );
        let icon =
            |editor: &CanvasEditor| editor.active_outline().shaped.paragraphs[1].tags[0].icon;
        assert_eq!(
            icon(&editor),
            crate::outline::TagIcon::Symbol {
                shape: 3,
                checked: false
            }
        );
        editor.format(&mut engine, Formatting::Check).unwrap();
        assert_eq!(text_tags(&editor, 1), [("To Do".into(), 1, true)]);
        assert_eq!(
            icon(&editor),
            crate::outline::TagIcon::Symbol {
                shape: 3,
                checked: true
            }
        );
        editor.select([at(1, 0); 2].into()).unwrap();
        editor.format(&mut engine, Formatting::Check).unwrap();
        editor.select([at(0, 1), at(1, 2)].into()).unwrap();
        editor.format(&mut engine, Formatting::Check).unwrap();
        assert_eq!(text_tags(&editor, 0), [("To Do".into(), 1, true)]);
        assert_eq!(text_tags(&editor, 1), [("To Do".into(), 1, true)]);

        editor.select([at(1, 0), at(2, 5)].into()).unwrap();
        editor
            .format(
                &mut engine,
                Formatting::Tag(NoteTag::defaults()[2].clone(), 2),
            )
            .unwrap();
        assert_eq!(
            text_tags(&editor, 1),
            [("Question".into(), 1, true), ("To Do".into(), 1, true)]
        );
        assert_eq!(text_tags(&editor, 2), [("Question".into(), 1, true)]);
        assert_eq!(
            editor.format_state().unwrap().tags,
            [(NoteTag::defaults()[2].clone(), 2)]
        );
        let questions = editor
            .definitions
            .values()
            .filter(|definition| **definition == NoteTag::defaults()[2].definition(2))
            .count();
        assert_eq!(questions, 1);
        editor
            .format(
                &mut engine,
                Formatting::Tag(NoteTag::defaults()[2].clone(), 2),
            )
            .unwrap();
        assert_eq!(text_tags(&editor, 2), []);
        editor.select([at(0, 0); 2].into()).unwrap();
        editor
            .format(
                &mut engine,
                Formatting::Tag(NoteTag::defaults()[0].clone(), 0),
            )
            .unwrap();
        assert_eq!(text_tags(&editor, 0), []);
        for _ in 0..7 {
            editor.undo(&mut engine).unwrap();
        }
        assert!(!editor.undo(&mut engine).unwrap());
        assert_eq!(editor.active_outline().document, original);
    }

    #[test]
    fn lists_become_to_do_lists_and_back_as_one_undo_step_each() {
        let mut engine = TextEngine::default();
        let mut editor = plain(&mut engine, &["one", "two", "three"]);
        editor.select([at(0, 0), at(1, 3)].into()).unwrap();
        editor.format(&mut engine, Formatting::Bullets).unwrap();
        editor.select([at(1, 0); 2].into()).unwrap();
        editor.format(&mut engine, Formatting::Numbering).unwrap();
        let listed = editor.active_outline().document.clone();
        let all = Selection::from([at(0, 0), at(2, 5)]);
        editor.select(all).unwrap();
        assert!(editor.format_state().unwrap().listed);
        let to_do = NoteTag::defaults()[0].clone();
        editor
            .format(&mut engine, Formatting::ToDoList(to_do.clone(), 0))
            .unwrap();
        let lists = |editor: &CanvasEditor| {
            let document = &editor.active_outline().document;
            document
                .nodes()
                .iter()
                .map(|node| editor.list(node))
                .collect::<Vec<_>>()
        };
        assert_eq!(lists(&editor), [None, None, None]);
        let task = || vec![("To Do".to_owned(), 0, true)];
        assert_eq!(
            (0..3).map(|at| text_tags(&editor, at)).collect::<Vec<_>>(),
            [task(), task(), vec![]]
        );
        let state = editor.format_state().unwrap();
        assert!(!state.listed && state.tags.is_empty());
        editor.select([at(0, 0), at(1, 3)].into()).unwrap();
        assert_eq!(editor.format_state().unwrap().tags, [(to_do.clone(), 0)]);
        editor
            .format(&mut engine, Formatting::BulletedList(to_do, 0))
            .unwrap();
        assert_eq!(
            lists(&editor),
            [Some(Formatting::Bullets), Some(Formatting::Bullets), None]
        );
        assert_eq!(text_tags(&editor, 0), []);
        assert_eq!(text_tags(&editor, 1), []);
        let state = editor.format_state().unwrap();
        assert_eq!((state.list, state.tags), (Some(ListStyle::BULLET), vec![]));
        editor.undo(&mut engine).unwrap();
        assert_eq!(text_tags(&editor, 1), task());
        editor.undo(&mut engine).unwrap();
        assert_eq!(editor.active_outline().document, listed);
    }

    #[test]
    fn the_default_tags_are_onenotes_stored_definitions_and_all_draw() {
        use onestore::{RevisionIndex, Store, document::Document};
        let bytes = include_bytes!("../../../../corpus/structural-probe/tag-gallery.one");
        let store = Store::parse(bytes).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let document = Document::parse(&index).unwrap();
        let (space, _) = document.pages().unwrap()[0];
        let stored = Page::from_space(&document, space).unwrap().definitions;
        let stored = stored
            .values()
            .filter(|definition| matches!(definition.kind, Kind::TagDefinition { .. }))
            .collect::<Vec<_>>();
        let defaults = NoteTag::defaults();
        assert_eq!(stored.len(), defaults.len());
        for (place, tag) in defaults.iter().enumerate() {
            assert!(stored.contains(&&tag.definition(place as u16)), "{tag:?}");
        }
        let mut engine = TextEngine::default();
        let mut editor = plain(&mut engine, &["tagged"]);
        for (place, tag) in defaults.iter().enumerate() {
            editor
                .format(&mut engine, Formatting::Tag(tag.clone(), place as u16))
                .unwrap();
        }
        assert_eq!(editor.format_state().unwrap().tags.len(), defaults.len());
        let paragraph = &editor.active_outline().shaped.paragraphs[0];
        let shapes = paragraph
            .tags
            .iter()
            .map(|tag| match tag.icon {
                crate::outline::TagIcon::Symbol { shape, .. } => shape,
                crate::outline::TagIcon::Task { .. } => unreachable!(),
            })
            .collect::<Vec<_>>();
        assert_eq!(shapes.len(), defaults.len() - 2);
        assert_eq!(shapes[..7], [3, 13, 15, 136, 118, 23, 18]);
        // As OneNote stores them unchecked: To Do, Discuss with, Schedule meeting, Call back,
        // the priorities and Client request.
        let checkable = defaults
            .iter()
            .filter(|tag| crate::outline::checkable(tag.shape))
            .count();
        assert_eq!(checkable, 9);
        // Remember for later and Definition draw no symbol; the newer one's green marks the text.
        assert!(
            paragraph
                .text
                .backgrounds()
                .all(|(_, color)| color == 0x0000_ff00)
        );
        assert!(paragraph.text.backgrounds().next().is_some());
    }

    /// A customized tag stores its name, symbol and colours with its place in the list as
    /// its action type; applied from another place it takes a definition of its own, as
    /// OneNote 2010's does (`corpus/custom-tags`).
    #[test]
    fn custom_tags_store_their_look_and_place() {
        let mut engine = TextEngine::default();
        let mut editor = plain(&mut engine, &["one", "two"]);
        let tag = NoteTag {
            label: "Snow check".into(),
            shape: 61,
            color: Some(0x0000_0080),
            highlight: Some(0x00ff_cc00),
            art: None,
        };
        editor
            .format(&mut engine, Formatting::Tag(tag.clone(), 0))
            .unwrap();
        assert_eq!(editor.format_state().unwrap().tags, [(tag.clone(), 0)]);
        assert_eq!(text_tags(&editor, 0), [("Snow check".into(), 1, true)]);
        editor.select([at(1, 0); 2].into()).unwrap();
        editor
            .format(&mut engine, Formatting::Tag(tag.clone(), 1))
            .unwrap();
        assert_eq!(editor.format_state().unwrap().tags, [(tag.clone(), 1)]);
        let mut stored: Vec<_> = editor
            .definitions
            .values()
            .filter_map(|definition| NoteTag::of(&definition.kind))
            .collect();
        stored.sort_by_key(|(_, action_type)| *action_type);
        assert_eq!(stored, [(tag.clone(), 0), (tag, 1)]);
        let paragraph = &editor.active_outline().shaped.paragraphs[0];
        assert!(
            paragraph
                .text
                .backgrounds()
                .all(|(_, color)| color == 0x00ff_cc00)
        );
        assert!(paragraph.text.backgrounds().next().is_some());
    }

    #[test]
    fn commands_reach_every_cell_a_selection_crosses() {
        let mut engine = TextEngine::default();
        let mut editor = plain(&mut engine, &["abcdef"]);
        editor.select([at(0, 3); 2].into()).unwrap();
        editor.tab(&mut engine, false).unwrap();
        let table = editor.active_outline().document.clone();
        assert_eq!(table.nodes().len(), 1);
        let selection = Selection::from([at(0, 1), at(1, 2)]);
        editor.select(selection).unwrap();
        editor
            .format(&mut engine, Formatting::Toggle(Toggle::Italic))
            .unwrap();
        assert_eq!(
            runs(&editor, |format| format.italic),
            [
                vec![run("a", None), run("bc", Some(true))],
                vec![run("de", Some(true)), run("f", None)]
            ]
        );
        assert!(
            editor
                .format_state()
                .unwrap()
                .toggles
                .contains(&Toggle::Italic)
        );
        editor
            .format(&mut engine, Formatting::Align(Alignment::Right))
            .unwrap();
        assert_eq!(
            editor.format_state().unwrap().alignment,
            Some(Alignment::Right)
        );
        editor.select([at(1, 0); 2].into()).unwrap();
        editor.format(&mut engine, Formatting::Numbering).unwrap();
        assert_eq!(numbers(&mut engine, &editor), [None, Some(1)]);
        editor
            .format(
                &mut engine,
                Formatting::Tag(NoteTag::defaults()[0].clone(), 0),
            )
            .unwrap();
        assert_eq!(editor.active_outline().shaped.paragraphs[1].tags.len(), 1);
        assert_eq!(editor.selection(), Selection::from([at(1, 0); 2]));
        for _ in 0..4 {
            editor.undo(&mut engine).unwrap();
        }
        assert_eq!(editor.active_outline().document, table);
        assert_eq!(editor.selection(), selection);
    }

    #[test]
    fn formatting_lists_and_tags_survive_the_page_writer() {
        use onestore::{RevisionIndex, Store, document::Document};
        const SECTION: &[u8] =
            include_bytes!("../../../../corpus/paragraph-edit/before/notebook/synthetic.one");
        let page = |bytes: &[u8]| {
            let store = Store::parse(bytes).unwrap();
            let index = RevisionIndex::parse(&store).unwrap();
            let document = Document::parse(&index).unwrap();
            document
                .pages()
                .unwrap()
                .into_iter()
                .map(|(space, _)| (space, Page::from_space(&document, space).unwrap()))
                .find(|(_, page)| page.title == "Split middle")
                .unwrap()
        };
        // Tags keep their place in the element's property arena only once read.
        let settled = |mut page: Page| {
            for object in &mut page.objects {
                if let PageObject::Outline(outline) = object {
                    for text in outline.paragraphs.iter_mut().filter_map(|p| p.text_mut()) {
                        for tag in &mut text.tags {
                            tag.extra_set = 0;
                        }
                    }
                }
            }
            page
        };
        let (space, source) = page(SECTION);
        let mut engine = TextEngine::default();
        let mut editor = CanvasEditor::from_page(source, &mut engine).unwrap();
        let body = editor
            .outlines()
            .iter()
            .find(|outline| !outline.title)
            .unwrap()
            .id;
        editor.focus_outline(body).unwrap();
        let selection = Selection::from([at(0, 2), at(0, 6)]);
        editor.select(selection).unwrap();
        for command in [
            Formatting::Toggle(Toggle::Bold),
            Formatting::Toggle(Toggle::Superscript),
            Formatting::FontSize(14.0),
            Formatting::Color(Some(0x0000_00ff)),
            Formatting::Highlight(Some(0x0000_ffff)),
            Formatting::Align(Alignment::Center),
            Formatting::Numbering,
            Formatting::Tag(NoteTag::defaults()[0].clone(), 0),
            Formatting::Tag(NoteTag::defaults()[2].clone(), 2),
            Formatting::Check,
        ] {
            editor.format(&mut engine, command).unwrap();
        }
        let state = editor.format_state().unwrap();
        let edited = settled(editor.page().unwrap());
        let written = super::ops::saved(SECTION, space, &mut editor);
        let (_, reread) = page(&written);
        let reread = settled(reread);
        assert_eq!(reread.objects, edited.objects);
        let mut editor = CanvasEditor::from_page(reread, &mut engine).unwrap();
        editor.focus_outline(body).unwrap();
        editor.select(selection).unwrap();
        assert_eq!(editor.format_state().unwrap(), state);
        assert_eq!(
            state,
            FormatState {
                toggles: vec![Toggle::Bold, Toggle::Superscript],
                font: state.font.clone(),
                font_size: Some(14.0),
                alignment: Some(Alignment::Center),
                bullets: false,
                numbering: true,
                listed: true,
                list: Some(ListStyle::NUMBER),
                tags: vec![
                    (NoteTag::defaults()[0].clone(), 0),
                    (NoteTag::defaults()[2].clone(), 2)
                ],
                style: state.style.clone(),
                style_font: state.style_font.clone(),
                style_size: state.style_size,
            }
        );
        // Format Painter, a gallery tag and an inserted table write as well.
        let painted = editor.painted_format().unwrap();
        editor.select([at(0, 0), at(0, 2)].into()).unwrap();
        editor
            .format(&mut engine, Formatting::Paint(painted))
            .unwrap();
        let state = editor.format_state().unwrap();
        assert_eq!(
            (state.toggles, state.font_size, state.alignment),
            (
                vec![Toggle::Bold, Toggle::Superscript],
                Some(14.0),
                Some(Alignment::Center)
            )
        );
        editor
            .format(
                &mut engine,
                Formatting::Tag(NoteTag::defaults()[25].clone(), 25),
            )
            .unwrap();
        editor.select([at(0, 1); 2].into()).unwrap();
        editor.insert_table(&mut engine, 2, 3).unwrap();
        assert_eq!(editor.selection(), Selection::from([at(1, 0); 2]));
        let edited = settled(editor.page().unwrap());
        let written = super::ops::saved(&written, space, &mut editor);
        let (_, reread) = page(&written);
        assert_eq!(settled(reread).objects, edited.objects);
    }
}
