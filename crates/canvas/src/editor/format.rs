//! OneNote's Home-ribbon commands: character formatting, lists, alignment and note tags.

use super::*;
use crate::layout::{DEFAULT_FONT, DEFAULT_FONT_SIZE};
use onestore::document::{Format, Kind, Tag};
use onestore::page::ParagraphContent;
use onestore::page::text::new_id;

/// The COLORREF OneNote stores for automatic colour, which overrides a style's colour.
const AUTOMATIC: u32 = 0xff00_0000;
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
    Tag(NoteTag),
    /// Removes every note tag from the selected paragraphs.
    RemoveTags,
    /// Checks the selected paragraphs' check boxes, or clears them when all are checked.
    Check,
    /// Format Painter: gives the selection the character formatting and alignment of text
    /// picked up with [`CanvasEditor::painted_format`], as OneNote's does.
    Paint(Format),
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
    fn of(kind: &Kind) -> Option<Self> {
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

/// OneNote 2010's default tags, the first nine on Ctrl+1 to Ctrl+9, as it stores their
/// definitions (`corpus/structural-probe/tag-gallery.one`); declared in gallery order, their
/// action types.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoteTag {
    ToDo,
    Important,
    Question,
    RememberForLater,
    Definition,
    Highlight,
    Contact,
    Address,
    PhoneNumber,
    WebSiteToVisit,
    Idea,
    Password,
    Critical,
    ProjectA,
    ProjectB,
    MovieToSee,
    BookToRead,
    MusicToListenTo,
    SourceForArticle,
    RememberForBlog,
    DiscussWithPersonA,
    DiscussWithPersonB,
    DiscussWithManager,
    SendInEmail,
    ScheduleMeeting,
    CallBack,
    ToDoPriority1,
    ToDoPriority2,
    ClientRequest,
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
    /// The library style every selected paragraph's list shares.
    pub list: Option<ListStyle>,
    /// Tags every selected paragraph has.
    pub tags: Vec<NoteTag>,
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

impl NoteTag {
    pub const ALL: [Self; 29] = [
        Self::ToDo,
        Self::Important,
        Self::Question,
        Self::RememberForLater,
        Self::Definition,
        Self::Highlight,
        Self::Contact,
        Self::Address,
        Self::PhoneNumber,
        Self::WebSiteToVisit,
        Self::Idea,
        Self::Password,
        Self::Critical,
        Self::ProjectA,
        Self::ProjectB,
        Self::MovieToSee,
        Self::BookToRead,
        Self::MusicToListenTo,
        Self::SourceForArticle,
        Self::RememberForBlog,
        Self::DiscussWithPersonA,
        Self::DiscussWithPersonB,
        Self::DiscussWithManager,
        Self::SendInEmail,
        Self::ScheduleMeeting,
        Self::CallBack,
        Self::ToDoPriority1,
        Self::ToDoPriority2,
        Self::ClientRequest,
    ];

    /// The tag's name, as OneNote stores and lists it.
    pub const fn label(self) -> &'static str {
        match self {
            Self::ToDo => "To Do",
            Self::Important => "Important",
            Self::Question => "Question",
            Self::RememberForLater => "Remember for later",
            Self::Definition => "Definition",
            Self::Highlight => "Highlight",
            Self::Contact => "Contact",
            Self::Address => "Address",
            Self::PhoneNumber => "Phone number",
            Self::WebSiteToVisit => "Web site to visit",
            Self::Idea => "Idea",
            Self::Password => "Password",
            Self::Critical => "Critical",
            Self::ProjectA => "Project A",
            Self::ProjectB => "Project B",
            Self::MovieToSee => "Movie to see",
            Self::BookToRead => "Book to read",
            Self::MusicToListenTo => "Music to listen to",
            Self::SourceForArticle => "Source for article",
            Self::RememberForBlog => "Remember for blog",
            Self::DiscussWithPersonA => "Discuss with <Person A>",
            Self::DiscussWithPersonB => "Discuss with <Person B>",
            Self::DiscussWithManager => "Discuss with manager",
            Self::SendInEmail => "Send in e-mail",
            Self::ScheduleMeeting => "Schedule meeting",
            Self::CallBack => "Call back",
            Self::ToDoPriority1 => "To Do priority 1",
            Self::ToDoPriority2 => "To Do priority 2",
            Self::ClientRequest => "Client request",
        }
    }

    /// The symbol OneNote draws for the tag; 0 draws none.
    pub const fn shape(self) -> u16 {
        match self {
            Self::ToDo => CHECKBOX,
            Self::Important => 13,
            Self::Question => 15,
            Self::RememberForLater | Self::Definition => 0,
            Self::Highlight => 136,
            Self::Contact => 118,
            Self::Address => 23,
            Self::PhoneNumber => 18,
            Self::WebSiteToVisit | Self::SourceForArticle => 125,
            Self::Idea => 21,
            Self::Password => 131,
            Self::Critical => 17,
            Self::ProjectA => 100,
            Self::ProjectB => 101,
            Self::MovieToSee => 122,
            Self::BookToRead => 132,
            Self::MusicToListenTo => 121,
            Self::RememberForBlog => 24,
            Self::DiscussWithPersonA | Self::DiscussWithPersonB => 94,
            Self::DiscussWithManager => 95,
            Self::SendInEmail => 106,
            Self::ScheduleMeeting | Self::CallBack => 12,
            Self::ToDoPriority1 => 28,
            Self::ToDoPriority2 => 71,
            Self::ClientRequest => 8,
        }
    }

    pub(super) fn definition(self) -> Definition {
        let highlight = match self {
            Self::RememberForLater => Some(0x0000_ffff),
            Self::Definition => Some(0x0000_ff00),
            _ => None,
        };
        Definition {
            kind: Kind::TagDefinition {
                label: Some(self.label().into()),
                action_type: Some(self as u16),
                shape: Some(self.shape()),
                // The highlighting tags also set black text.
                color: highlight.map(|_| 0),
                highlight,
            },
            format: Format::default(),
        }
    }
}

/// Whether tag symbol `shape` is a check box, which OneNote stores unchecked when applied.
fn checkable(shape: Option<u16>) -> bool {
    matches!(
        shape.and_then(|shape| crate::outline::TagIcon::of(shape, false)),
        Some(crate::outline::TagIcon::CheckBox { .. })
    )
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

pub(super) fn leaves_mut(nodes: &mut [PageParagraph], change: &mut impl FnMut(&mut PageParagraph)) {
    for node in nodes {
        match &mut node.content {
            ParagraphContent::Text(_) => change(node),
            ParagraphContent::Table(table) => {
                for cell in table.rows.iter_mut().flat_map(|row| &mut row.cells) {
                    leaves_mut(&mut cell.paragraphs, change);
                }
            }
            _ => {}
        }
    }
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

/// Seconds since 1980, as note tags date themselves.
fn time32() -> Option<u32> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
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
            list: common(paragraphs.iter().map(|node| self.list_style(node))),
            tags: NoteTag::ALL
                .into_iter()
                .filter(|tag| {
                    let kind = tag.definition().kind;
                    paragraphs
                        .iter()
                        .all(|node| tags(node).any(|tag| self.tag_kind(tag) == Some(&kind)))
                })
                .collect(),
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
            | Formatting::Tag(_)
            | Formatting::RemoveTags
            | Formatting::Check
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
                leaves_mut(&mut replacement, &mut |node| {
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
                });
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
                leaves_mut(&mut replacement, &mut |node| {
                    if ranges.remove(&node.id).is_some() {
                        if remove {
                            node.lists.clear();
                        } else if let Some(id) = lists.get(&node.id) {
                            node.lists = vec![*id];
                        }
                    }
                });
            }
            Formatting::Tag(tag) => {
                let definition = tag.definition();
                let Kind::TagDefinition {
                    action_type, shape, ..
                } = definition.kind
                else {
                    unreachable!()
                };
                let existing = self
                    .definitions
                    .iter()
                    .find(|(_, other)| other.kind == definition.kind);
                let id = match existing {
                    Some((id, _)) => *id,
                    None => {
                        let id = new_id()?;
                        self.definitions.insert(id, definition.clone());
                        id
                    }
                };
                let has = |node: &PageParagraph| {
                    tags(node).any(|tag| self.tag_kind(tag) == Some(&definition.kind))
                };
                let remove = covered(&replacement, ends).all(|(node, _)| has(node));
                let created = time32();
                leaves_mut(&mut replacement, &mut |node| {
                    if ranges.remove(&node.id).is_none() || !remove && has(node) {
                        return;
                    }
                    let ParagraphContent::Text(text) = &mut node.content else {
                        unreachable!()
                    };
                    // An element holds one tag of each action type.
                    for tags in [&mut node.tags, &mut text.tags] {
                        tags.retain(|tag| match self.tag_kind(tag) {
                            Some(kind) if remove => *kind != definition.kind,
                            Some(Kind::TagDefinition {
                                action_type: other, ..
                            }) => *other != action_type,
                            _ => true,
                        });
                    }
                    if !remove {
                        let checkable = checkable(shape);
                        // Stored newest first.
                        text.tags.insert(
                            0,
                            Tag {
                                definition: Some(id),
                                action_type: None,
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
                });
            }
            Formatting::RemoveTags => leaves_mut(&mut replacement, &mut |node| {
                if ranges.remove(&node.id).is_some() {
                    node.tags.clear();
                    node.text_mut().unwrap().tags.clear();
                }
            }),
            Formatting::Check => self.check(&mut replacement, ranges, ends),
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
                leaves_mut(&mut replacement, &mut |node| {
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
                });
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
            Formatting::Tag(tag) => state.tags.contains(tag),
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
        let paragraph = leaves(document.nodes(), None)
            .position(|(_, _, node)| node.id == id)
            .ok_or(EditError::InvalidRange)?;
        let at = TextPosition {
            paragraph,
            offset: 0,
        };
        let (container, range, ends) = selected(document, [at; 2].into())?;
        let mut replacement = document.container(container)?[range.clone()].to_vec();
        let ranges = covered(&replacement, ends)
            .map(|(node, range)| (node.id, range))
            .collect();
        self.check(&mut replacement, ranges, ends);
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

    /// Checks the check boxes of the `covered` paragraphs, or clears them once all are
    /// checked; OneNote keeps a cleared box's completion time as zero.
    fn check(
        &self,
        replacement: &mut [PageParagraph],
        mut covered: BTreeMap<ExGuid, Range<usize>>,
        ends: Ends,
    ) {
        let checkable = |tag: &Tag| {
            matches!(
                self.tag_kind(tag),
                Some(Kind::TagDefinition { shape, .. }) if checkable(*shape)
            )
        };
        let checked = self::covered(replacement, ends)
            .flat_map(|(node, _)| tags(node))
            .filter(|tag| checkable(tag))
            .all(|tag| tag.status & 1 != 0);
        let completed = if checked { Some(0) } else { time32() };
        leaves_mut(replacement, &mut |node| {
            if covered.remove(&node.id).is_none() {
                return;
            }
            let ParagraphContent::Text(text) = &mut node.content else {
                unreachable!()
            };
            for tag in node.tags.iter_mut().chain(&mut text.tags) {
                if checkable(tag) && (tag.status & 1 != 0) == checked {
                    tag.status ^= 1;
                    tag.completed = completed;
                }
            }
        });
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
            .format(&mut engine, Formatting::Tag(NoteTag::ToDo))
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
            .format(&mut engine, Formatting::Tag(NoteTag::ToDo))
            .unwrap();
        assert_eq!(text_tags(&editor, 0), [("To Do".into(), 0, true)]);
        assert_eq!(text_tags(&editor, 1), [("To Do".into(), 0, true)]);
        assert_eq!(editor.format_state().unwrap().tags, [NoteTag::ToDo]);
        let icon =
            |editor: &CanvasEditor| editor.active_outline().shaped.paragraphs[1].tags[0].icon;
        assert_eq!(
            icon(&editor),
            crate::outline::TagIcon::CheckBox { checked: false }
        );
        editor.format(&mut engine, Formatting::Check).unwrap();
        assert_eq!(text_tags(&editor, 1), [("To Do".into(), 1, true)]);
        assert_eq!(
            icon(&editor),
            crate::outline::TagIcon::CheckBox { checked: true }
        );
        editor.select([at(1, 0); 2].into()).unwrap();
        editor.format(&mut engine, Formatting::Check).unwrap();
        editor.select([at(0, 1), at(1, 2)].into()).unwrap();
        editor.format(&mut engine, Formatting::Check).unwrap();
        assert_eq!(text_tags(&editor, 0), [("To Do".into(), 1, true)]);
        assert_eq!(text_tags(&editor, 1), [("To Do".into(), 1, true)]);

        editor.select([at(1, 0), at(2, 5)].into()).unwrap();
        editor
            .format(&mut engine, Formatting::Tag(NoteTag::Question))
            .unwrap();
        assert_eq!(
            text_tags(&editor, 1),
            [("Question".into(), 1, true), ("To Do".into(), 1, true)]
        );
        assert_eq!(text_tags(&editor, 2), [("Question".into(), 1, true)]);
        assert_eq!(editor.format_state().unwrap().tags, [NoteTag::Question]);
        let questions = editor
            .definitions
            .values()
            .filter(|definition| **definition == NoteTag::Question.definition())
            .count();
        assert_eq!(questions, 1);
        editor
            .format(&mut engine, Formatting::Tag(NoteTag::Question))
            .unwrap();
        assert_eq!(text_tags(&editor, 2), []);
        editor.select([at(0, 0); 2].into()).unwrap();
        editor
            .format(&mut engine, Formatting::Tag(NoteTag::ToDo))
            .unwrap();
        assert_eq!(text_tags(&editor, 0), []);
        for _ in 0..7 {
            editor.undo(&mut engine).unwrap();
        }
        assert!(!editor.undo(&mut engine).unwrap());
        assert_eq!(editor.active_outline().document, original);
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
        assert_eq!(stored.len(), NoteTag::ALL.len());
        for tag in NoteTag::ALL {
            assert!(stored.contains(&&tag.definition()), "{tag:?}");
        }
        let mut engine = TextEngine::default();
        let mut editor = plain(&mut engine, &["tagged"]);
        for tag in NoteTag::ALL {
            editor.format(&mut engine, Formatting::Tag(tag)).unwrap();
        }
        assert_eq!(editor.format_state().unwrap().tags, NoteTag::ALL);
        let paragraph = &editor.active_outline().shaped.paragraphs[0];
        use crate::outline::TagIcon;
        let icons = paragraph
            .tags
            .iter()
            .map(|tag| tag.icon)
            .collect::<Vec<_>>();
        assert_eq!(icons.len(), NoteTag::ALL.len() - 2);
        assert_eq!(
            icons[..7],
            [
                TagIcon::CheckBox { checked: false },
                TagIcon::Star,
                TagIcon::Question,
                TagIcon::Highlight,
                TagIcon::Contact,
                TagIcon::Address,
                TagIcon::Phone,
            ]
        );
        // As OneNote stores them unchecked: To Do, Discuss with, Schedule meeting, Call back,
        // the priorities and Client request.
        let checkable = NoteTag::ALL
            .into_iter()
            .filter(|tag| checkable(Some(tag.shape())))
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
            .format(&mut engine, Formatting::Tag(NoteTag::ToDo))
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
            Formatting::Tag(NoteTag::ToDo),
            Formatting::Tag(NoteTag::Question),
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
                list: Some(ListStyle::NUMBER),
                tags: vec![NoteTag::ToDo, NoteTag::Question],
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
            .format(&mut engine, Formatting::Tag(NoteTag::CallBack))
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
