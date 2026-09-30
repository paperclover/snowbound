//! The commands the menu bar, the toolbar and the keyboard share: each one's title, its
//! chords on each platform, when it applies and what it does.

use crate::recording::{Media, Transport};
use crate::{Command as Work, FONTS, HIGHLIGHTS, SIZES, State, platform, search};
use canvas::editor::{Alignment, FormatState, Formatting, ListStyle, Pen, Toggle};
use canvas::interaction::{Request, ink::Tool};
use draw::edit::{Key, Modifiers, NamedKey, Platform};
use onestore::page::ink::ShapeKind;
use std::{error::Error, sync::Arc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Id {
    Settings,
    NewNotebook,
    OpenNotebook,
    OpenFromServer,
    CloseNotebook,
    NewSection,
    NewSectionGroup,
    NewPage,
    NewSubpage,
    PageVersions,
    CopyPageLink,
    ShowNotebook,
    ExportPdf,
    ExportSectionPdf,
    Print,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    FormatPainter,
    SelectAll,
    Find,
    Search,
    SearchResults,
    /// The palette listing notebooks, sections and pages to go to.
    GoTo,
    /// The palette listing commands, as typing `>` in it does.
    CommandPalette,
    Back,
    Forward,
    ZoomIn,
    ZoomOut,
    ActualSize,
    Sidebar,
    PageList,
    PagesMatchTheme,
    FullPageView,
    /// OneNote's Hide Spelling Errors, which leaves misspelled words unmarked.
    HideSpelling,
    /// The Spelling pane, on the next marked word.
    Spelling,
    PageColor,
    Table,
    Picture,
    ScreenClipping,
    Attachment,
    Link,
    InsertSpace,
    Equation,
    Symbol,
    Date,
    Time,
    DateTime,
    RecordAudio,
    RecordVideo,
    Transport(crate::recording::Transport),
    /// The Draw tab's tools.
    SelectType,
    Pen,
    Eraser,
    Lasso,
    Shape(ShapeKind),
    SnapToGrid,
    Toggle(Toggle),
    Highlight,
    FontColor,
    Bullets,
    Numbering,
    Align(Alignment),
    Indent,
    Outdent,
    ClearFormatting,
    /// The tag at this place in the user's tag list.
    Tag(usize),
    CustomizeTags,
    RemoveTags,
    FindTags,
    Help,
    CheckForUpdates,
}

/// Whether this platform can run command `id` at all; the toolbar and palette leave out
/// one it can't.
pub fn offered(id: Id) -> bool {
    id != Id::ScreenClipping || cfg!(target_os = "macos")
}

/// What a menu or toolbar offers: a command, or one of a list's entries.
#[derive(Clone, Debug, PartialEq)]
pub enum Choice {
    Command(Id),
    Font(String),
    Size(f32),
    /// A COLORREF, or `None` for no highlight.
    Highlight(Option<u32>),
    /// A COLORREF, or `None` for automatic.
    Color(Option<u32>),
    List(Option<ListStyle>),
    /// An empty table of rows by columns.
    Table([usize; 2]),
    /// A page colour, COLORREF, or `None` for no colour.
    PageColor(Option<u32>),
    /// An index into `RULE_LINES`, or `None` for none.
    RuleLines(Option<usize>),
    /// A template whose art becomes the page's background, or `None` for none.
    Art(Option<&'static str>),
    /// A place in the pen gallery, which Pen then draws with.
    Pen(usize),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub enabled: bool,
    /// Whether a toggle command is on; `None` for a command that is not a toggle.
    pub checked: Option<bool>,
}

/// A key with modifiers; `command` is the shortcut modifier, Control off macOS.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Chord {
    pub key: Press,
    pub shift: bool,
    pub option: bool,
    pub control: bool,
    pub command: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Press {
    /// Unshifted and lowercase, as on a US keyboard.
    Char(char),
    Named(NamedKey),
}

pub struct Command {
    pub id: Id,
    pub title: &'static str,
    pub mac: &'static [Chord],
    pub pc: &'static [Chord],
}

const fn key(key: char) -> Chord {
    press(Press::Char(key))
}

pub(crate) const fn named(key: NamedKey) -> Chord {
    press(Press::Named(key))
}

const fn press(key: Press) -> Chord {
    Chord {
        key,
        shift: false,
        option: false,
        control: false,
        command: false,
    }
}

/// `key` with the shortcut modifier.
pub(crate) const fn cmd(key: char) -> Chord {
    self::key(key).command()
}

impl Chord {
    pub(crate) const fn command(mut self) -> Self {
        self.command = true;
        self
    }

    const fn shift(mut self) -> Self {
        self.shift = true;
        self
    }

    pub(crate) const fn option(mut self) -> Self {
        self.option = true;
        self
    }

    pub(crate) const fn control(mut self) -> Self {
        self.control = true;
        self
    }
}

macro_rules! row {
    ($id:expr, $title:expr, $mac:expr, $pc:expr $(,)?) => {
        Command {
            id: $id,
            title: $title,
            mac: $mac,
            pc: $pc,
        }
    };
}

const NONE: &[Chord] = &[];

/// OneNote 2010's chords off macOS; there, AppKit's and OneNote for Mac's.
pub const COMMANDS: &[Command] = &[
    row!(Id::Settings, "Settings…", &[cmd(',')], &[cmd(',')]),
    row!(Id::NewNotebook, "New Notebook…", NONE, NONE),
    row!(Id::OpenNotebook, "Open Notebook…", &[cmd('o')], &[cmd('o')]),
    row!(Id::OpenFromServer, "Open Notebook from Server…", NONE, NONE),
    row!(Id::CloseNotebook, "Close This Notebook", NONE, NONE),
    row!(Id::NewSection, "New Section", &[cmd('t')], &[cmd('t')]),
    row!(Id::NewSectionGroup, "New Section Group", NONE, NONE),
    row!(Id::NewPage, "New Page", &[cmd('n')], &[cmd('n')]),
    row!(
        Id::NewSubpage,
        "New Subpage",
        &[cmd('n').shift().option()],
        &[cmd('n').shift().option()],
    ),
    row!(Id::PageVersions, "Page Versions", NONE, NONE),
    row!(Id::CopyPageLink, "Copy Link to Page", NONE, NONE),
    row!(
        Id::ShowNotebook,
        if cfg!(target_os = "macos") {
            "Show in Finder"
        } else {
            "Open Notebook Folder"
        },
        NONE,
        NONE,
    ),
    row!(Id::ExportPdf, "Export as PDF…", NONE, NONE),
    row!(Id::ExportSectionPdf, "Export Section as PDF…", NONE, NONE),
    // Go to takes OneNote's Ctrl+P, and Pause its Ctrl+Alt+P.
    row!(
        Id::Print,
        "Print…",
        &[cmd('p').option().shift()],
        &[cmd('p').option().shift()]
    ),
    row!(Id::Undo, "Undo", &[cmd('z')], &[cmd('z')]),
    row!(
        Id::Redo,
        "Redo",
        &[cmd('z').shift()],
        if cfg!(windows) {
            &[cmd('y'), cmd('z').shift()]
        } else {
            &[cmd('z').shift()]
        },
    ),
    row!(Id::Cut, "Cut", &[cmd('x')], &[cmd('x')]),
    row!(Id::Copy, "Copy", &[cmd('c')], &[cmd('c')]),
    row!(Id::Paste, "Paste", &[cmd('v')], &[cmd('v')]),
    row!(Id::FormatPainter, "Format Painter", NONE, NONE),
    row!(Id::SelectAll, "Select All", &[cmd('a')], &[cmd('a')]),
    row!(Id::Find, "Find on This Page", &[cmd('f')], &[cmd('f')]),
    row!(Id::Search, "Search Notebooks", &[cmd('e')], &[cmd('e')]),
    // OneNote for Mac searches all notebooks with Option-Command-F.
    row!(
        Id::SearchResults,
        "Search Results Pane",
        &[cmd('f').option()],
        &[key('o').option()],
    ),
    row!(Id::GoTo, "Go to Page or Section…", &[cmd('p')], &[cmd('p')]),
    row!(
        Id::CommandPalette,
        "Command Palette…",
        &[cmd('p').shift()],
        &[cmd('p').shift()]
    ),
    // Option-Command-arrows move the caret; Xcode goes back with Control-Command.
    row!(
        Id::Back,
        "Back",
        &[named(NamedKey::ArrowLeft).command().control()],
        &[named(NamedKey::ArrowLeft).option()],
    ),
    row!(
        Id::Forward,
        "Forward",
        &[named(NamedKey::ArrowRight).command().control()],
        &[named(NamedKey::ArrowRight).option()],
    ),
    // OneNote 2010 leaves Control with = and - to subscript and strikethrough.
    row!(
        Id::ZoomIn,
        "Zoom In",
        &[cmd('='), cmd('=').shift()],
        &[cmd('=').option(), cmd('=').option().shift()],
    ),
    row!(Id::ZoomOut, "Zoom Out", &[cmd('-')], &[cmd('-').option()]),
    row!(
        Id::ActualSize,
        "Actual Size",
        &[cmd('0')],
        &[cmd('0').option()]
    ),
    row!(Id::Sidebar, "Notebook List", &[cmd('s').control()], NONE),
    row!(Id::PageList, "Page List", NONE, NONE),
    row!(Id::PagesMatchTheme, "Pages Match UI Theme", NONE, NONE),
    row!(Id::HideSpelling, "Hide Spelling Errors", NONE, NONE),
    row!(
        Id::Spelling,
        "Spelling…",
        &[named(NamedKey::F7)],
        &[named(NamedKey::F7)]
    ),
    // AppKit's Enter Full Screen takes Control-Command-F.
    row!(
        Id::FullPageView,
        "Full Page View",
        NONE,
        &[named(NamedKey::F11)]
    ),
    row!(Id::PageColor, "Page Color", NONE, NONE),
    row!(Id::Table, "Table", NONE, NONE),
    row!(Id::Picture, "Picture…", NONE, NONE),
    row!(Id::ScreenClipping, "Screen Clipping", NONE, NONE),
    row!(Id::Attachment, "Attach File…", NONE, NONE),
    row!(Id::Link, "Link…", &[cmd('k')], &[cmd('k')]),
    row!(Id::InsertSpace, "Insert Space", NONE, NONE),
    row!(
        Id::Equation,
        "Equation",
        &[key('=').control()],
        &[key('=').option()],
    ),
    row!(Id::Symbol, "Symbol", NONE, NONE),
    row!(Id::Date, "Date", NONE, &[key('d').option().shift()]),
    row!(Id::Time, "Time", NONE, &[key('t').option().shift()]),
    row!(
        Id::DateTime,
        "Date & Time",
        NONE,
        &[key('f').option().shift()]
    ),
    row!(Id::RecordAudio, "Record Audio", NONE, NONE),
    row!(Id::RecordVideo, "Record Video", NONE, NONE),
    // OneNote 2010's playback chords, Control-Alt with P, S, Y and U.
    row!(
        Id::Transport(Transport::Pause),
        "Pause",
        &[cmd('p').option()],
        &[cmd('p').option()]
    ),
    row!(
        Id::Transport(Transport::Stop),
        "Stop",
        &[cmd('s').option()],
        &[cmd('s').option()]
    ),
    row!(
        Id::Transport(Transport::Skip(-600)),
        "Rewind 10 Minutes",
        NONE,
        NONE
    ),
    row!(
        Id::Transport(Transport::Skip(-10)),
        "Rewind 10 Seconds",
        &[cmd('y').option()],
        &[cmd('y').option()]
    ),
    row!(
        Id::Transport(Transport::Skip(10)),
        "Fast Forward 10 Seconds",
        &[cmd('u').option()],
        &[cmd('u').option()]
    ),
    row!(
        Id::Transport(Transport::Skip(600)),
        "Fast Forward 10 Minutes",
        NONE,
        NONE
    ),
    row!(Id::Transport(Transport::SeekTo), "Seek To…", NONE, NONE),
    row!(
        Id::Transport(Transport::SeePlayback),
        "See Playback",
        NONE,
        NONE
    ),
    row!(Id::SelectType, "Select & Type", NONE, NONE),
    row!(Id::Pen, "Pen", NONE, NONE),
    row!(Id::Eraser, "Eraser", NONE, NONE),
    row!(Id::Lasso, "Lasso Select", NONE, NONE),
    row!(Id::Shape(ShapeKind::Line), "Line", NONE, NONE),
    row!(Id::Shape(ShapeKind::Arrow), "Arrow", NONE, NONE),
    row!(Id::Shape(ShapeKind::Rectangle), "Rectangle", NONE, NONE),
    row!(Id::Shape(ShapeKind::Ellipse), "Oval", NONE, NONE),
    row!(Id::SnapToGrid, "Snap To Grid", NONE, NONE),
    row!(Id::Toggle(Toggle::Bold), "Bold", &[cmd('b')], &[cmd('b')]),
    row!(
        Id::Toggle(Toggle::Italic),
        "Italic",
        &[cmd('i')],
        &[cmd('i')]
    ),
    row!(
        Id::Toggle(Toggle::Underline),
        "Underline",
        &[cmd('u')],
        &[cmd('u')]
    ),
    row!(
        Id::Toggle(Toggle::Strikethrough),
        "Strikethrough",
        &[cmd('x').shift()],
        &[cmd('-')],
    ),
    row!(
        Id::Toggle(Toggle::Subscript),
        "Subscript",
        &[cmd('=').control().option()],
        &[cmd('=')],
    ),
    row!(
        Id::Toggle(Toggle::Superscript),
        "Superscript",
        &[cmd('=').option().shift()],
        &[cmd('=').shift()],
    ),
    // AppKit's Hide Others takes Option-Command-H.
    row!(
        Id::Highlight,
        "Highlight",
        &[cmd('h').control()],
        &[cmd('h').option()]
    ),
    row!(Id::FontColor, "Font Color", NONE, NONE),
    row!(Id::Bullets, "Bullets", &[cmd('.')], &[cmd('.')]),
    row!(Id::Numbering, "Numbering", &[cmd('/')], &[cmd('/')]),
    row!(
        Id::Align(Alignment::Left),
        "Align Left",
        &[cmd('l')],
        &[cmd('l')]
    ),
    row!(Id::Align(Alignment::Center), "Center", NONE, NONE),
    row!(
        Id::Align(Alignment::Right),
        "Align Right",
        &[cmd('r')],
        &[cmd('r')]
    ),
    row!(
        Id::Indent,
        "Increase Indent",
        &[cmd(']')],
        &[named(NamedKey::ArrowRight).option().shift()],
    ),
    row!(
        Id::Outdent,
        "Decrease Indent",
        &[cmd('[')],
        &[named(NamedKey::ArrowLeft).option().shift()],
    ),
    row!(
        Id::ClearFormatting,
        "Clear Formatting",
        &[cmd('n').shift()],
        &[cmd('n').shift()]
    ),
    row!(Id::CustomizeTags, "Customize Tags…", NONE, NONE),
    row!(
        Id::RemoveTags,
        "Remove Tag",
        &[cmd('0').control()],
        &[cmd('0')]
    ),
    row!(Id::FindTags, "Find Tags", NONE, NONE),
    row!(Id::Help, "Snowbound Help", NONE, NONE),
    row!(Id::CheckForUpdates, "Check for Updates…", NONE, NONE),
];

/// The most tags a list holds: MS-ONE's action types 0 to 99 number them.
pub const TAGS: usize = 100;

/// Where Snowbound Help leads.
const HELP: &str = "https://shale.paperclover.net/snowbound";

pub fn command(id: Id) -> &'static Command {
    COMMANDS
        .iter()
        .find(|command| command.id == id)
        .expect("Every command has a row")
}

impl Command {
    pub fn chords(&self, platform: Platform) -> &'static [Chord] {
        match platform {
            Platform::MacOs => self.mac,
            Platform::Gtk | Platform::Windows => self.pc,
        }
    }
}

/// Every choice the menu bar offers, in the order its `index` counts them.
pub fn choices() -> impl Iterator<Item = Choice> {
    COMMANDS
        .iter()
        .map(|command| Choice::Command(command.id))
        .chain(FONTS.map(|name| Choice::Font(name.to_owned())))
        .chain(SIZES.into_iter().map(Choice::Size))
        .chain((0..TAGS).map(|place| Choice::Command(Id::Tag(place))))
        .chain(std::iter::once(Choice::Highlight(None)))
        .chain(
            HIGHLIGHTS
                .iter()
                .map(|(color, _)| Choice::Highlight(Some(*color))),
        )
}

/// How a menu names `choice`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn title(choice: Choice) -> String {
    match choice {
        Choice::Command(Id::Tag(_)) => unreachable!("The tag list names its tags"),
        Choice::Command(id) => command(id).title.to_owned(),
        Choice::Font(name) => name,
        Choice::Size(size) => format!("{size}"),
        Choice::Highlight(None) => "No Color".to_owned(),
        Choice::Highlight(Some(color)) => HIGHLIGHTS
            .iter()
            .find(|(listed, _)| *listed == color)
            .map_or_else(String::new, |(_, name)| (*name).to_owned()),
        Choice::Color(_)
        | Choice::List(_)
        | Choice::Table(_)
        | Choice::PageColor(_)
        | Choice::RuleLines(_)
        | Choice::Art(_)
        | Choice::Pen(_) => {
            unreachable!("Only the toolbar offers these")
        }
    }
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn index(choice: &Choice) -> usize {
    choices()
        .position(|listed| listed == *choice)
        .expect("Menus offer listed choices")
}

/// The command `key` with `modifiers` runs here.
pub fn find(key: &Key, modifiers: Modifiers) -> Option<Id> {
    ran(
        pressed(key, modifiers, Platform::CURRENT)?,
        Platform::CURRENT,
    )
}

/// The command `chord` runs on `platform`.
fn ran(chord: Chord, platform: Platform) -> Option<Id> {
    COMMANDS
        .iter()
        .find(|command| command.chords(platform).contains(&chord))
        .map(|command| command.id)
        .or_else(|| {
            (0..9)
                .find(|&place| tag_chord(place) == Some(chord))
                .map(Id::Tag)
        })
}

/// The chord applying the tag at `place` in the list: the first nine take Ctrl+1 to Ctrl+9,
/// as OneNote's do.
pub fn tag_chord(place: usize) -> Option<Chord> {
    let digit = u8::try_from(place).ok().filter(|place| *place < 9)?;
    Some(cmd(char::from(b'1' + digit)))
}

fn pressed(key: &Key, modifiers: Modifiers, platform: Platform) -> Option<Chord> {
    let (key, shifted) = match key {
        Key::Named(named) => (Press::Named(*named), false),
        Key::Character(text) => {
            let mut chars = text.chars();
            let character = chars.next().filter(|_| chars.next().is_none())?;
            match SHIFTED.iter().find(|(shifted, _)| *shifted == character) {
                Some((_, base)) => (Press::Char(*base), true),
                None => (Press::Char(character.to_ascii_lowercase()), false),
            }
        }
    };
    Some(Chord {
        key,
        shift: modifiers.shift || shifted,
        option: modifiers.option,
        // Off macOS Control is the shortcut modifier.
        control: modifiers.control && platform == Platform::MacOs,
        command: modifiers.command,
    })
}

/// The US keyboard's shifted symbols and the keys they share.
const SHIFTED: [(char, char); 21] = [
    ('+', '='),
    ('_', '-'),
    ('{', '['),
    ('}', ']'),
    ('<', ','),
    ('>', '.'),
    ('?', '/'),
    (':', ';'),
    ('"', '\''),
    ('|', '\\'),
    ('~', '`'),
    (')', '0'),
    ('!', '1'),
    ('@', '2'),
    ('#', '3'),
    ('$', '4'),
    ('%', '5'),
    ('^', '6'),
    ('&', '7'),
    ('*', '8'),
    ('(', '9'),
];

impl Chord {
    /// The key AppKit matches and shows: a shifted symbol stands for Shift with its key.
    pub fn equivalent(self) -> (String, bool) {
        match self.key {
            Press::Char(base) if self.shift && !base.is_ascii_alphabetic() => {
                let shifted = SHIFTED.iter().find(|(_, key)| *key == base);
                shifted.map_or((base.to_string(), true), |(symbol, _)| {
                    (symbol.to_string(), false)
                })
            }
            Press::Char(base) => (base.to_string(), self.shift),
            Press::Named(NamedKey::Space) => (" ".to_owned(), self.shift),
            Press::Named(NamedKey::ArrowLeft) => ('\u{f702}'.to_string(), self.shift),
            Press::Named(NamedKey::ArrowRight) => ('\u{f703}'.to_string(), self.shift),
            // AppKit's NSF7FunctionKey.
            Press::Named(NamedKey::F7) => ('\u{f70a}'.to_string(), self.shift),
            Press::Named(named) => unreachable!("No command takes {named:?}"),
        }
    }

    /// As a menu shows it on `platform`: ⌃⌥⇧⌘K on macOS, Ctrl+Alt+Shift+K elsewhere.
    pub fn label(self, platform: Platform) -> String {
        if platform == Platform::MacOs {
            let (key, shift) = self.equivalent();
            let key = match self.key {
                Press::Named(NamedKey::Space) => "Space".to_owned(),
                Press::Named(NamedKey::ArrowLeft) => "←".to_owned(),
                Press::Named(NamedKey::ArrowRight) => "→".to_owned(),
                Press::Named(NamedKey::F7) => "F7".to_owned(),
                _ => key.to_uppercase(),
            };
            [
                (self.control, "⌃"),
                (self.option, "⌥"),
                (shift, "⇧"),
                (self.command, "⌘"),
            ]
            .iter()
            .filter(|(held, _)| *held)
            .map(|(_, symbol)| *symbol)
            .chain([key.as_str()])
            .collect()
        } else {
            let key = match self.key {
                Press::Char(key) => key.to_uppercase().to_string(),
                Press::Named(NamedKey::ArrowLeft) => "Left".to_owned(),
                Press::Named(NamedKey::ArrowRight) => "Right".to_owned(),
                Press::Named(named) => format!("{named:?}"),
            };
            [
                (self.command, "Ctrl+"),
                (self.option, "Alt+"),
                (self.shift, "Shift+"),
            ]
            .iter()
            .filter(|(held, _)| *held)
            .map(|(_, modifier)| *modifier)
            .chain([key.as_str()])
            .collect()
        }
    }
}

/// The chord a menu or hint shows for `id` here, empty where it has none.
pub fn shortcut(id: Id) -> String {
    let chord = match id {
        Id::Tag(place) => tag_chord(place),
        id => command(id).chords(Platform::CURRENT).first().copied(),
    };
    chord.map_or_else(String::new, |chord| chord.label(Platform::CURRENT))
}

impl State {
    /// The open section's pen gallery.
    pub(crate) fn pens(&self) -> [Pen; 15] {
        canvas::interaction::ink::pens(crate::section_color(
            self.session
                .as_ref()
                .and_then(|session| session.tabs[session.tab].color),
        ))
    }

    pub(crate) fn format_state(&self) -> FormatState {
        self.view.editor.format_state().unwrap_or_default()
    }

    /// Queues `choice` to run after this frame's input, as a key or a click on it does.
    pub(crate) fn choose(&mut self, choice: Choice) {
        self.commands.push(Work::Choose(choice));
        self.window.request_redraw();
    }

    /// Every choice's status, in `choices` order, for the menu bar.
    pub(crate) fn statuses(&self) -> Vec<Status> {
        let format = self.format_state();
        choices()
            .map(|choice| self.status(&choice, &format))
            .collect()
    }

    pub(crate) fn status(&self, choice: &Choice, format: &FormatState) -> Status {
        let session = self.session.as_ref();
        let welcome = session.is_none() && !self.temporary && self.sectionless.is_none();
        let modal = self.options.is_some()
            || self.link.is_some()
            || self.tag_list.is_some()
            || self.server.is_some();
        let page = (session.is_some() || self.temporary) && !modal;
        let writable = page && !session.is_some_and(|session| session.read_only());
        // Picked drawings leave the text selection behind them unseen, so it takes no edits.
        let typing = self.view.accepts_text() && self.view.ink_selection().is_empty();
        let text = writable && typing;
        // Edit commands act on a focused field instead of the page.
        let field = self.ui.focused_field().is_some();
        let [anchor, focus] = self.view.editor.selection().positions;
        let selected = page && !field && typing && anchor != focus;
        let enabled = |enabled| Status {
            enabled,
            checked: None,
        };
        let checked = |checked| Status {
            enabled: text,
            checked: Some(checked),
        };
        let id = match choice {
            Choice::Command(id) => *id,
            Choice::Font(name) => return checked(format.font.as_ref() == Some(name)),
            Choice::Size(size) => return checked(format.font_size == Some(*size)),
            Choice::Highlight(_) | Choice::Color(_) | Choice::List(_) => return enabled(text),
            Choice::Table(_) => Id::Table,
            Choice::PageColor(_) | Choice::RuleLines(_) | Choice::Art(_) => Id::PageColor,
            Choice::Pen(_) => Id::Pen,
        };
        let tool = self.view.tool();
        match id {
            Id::Settings
            | Id::NewNotebook
            | Id::OpenNotebook
            | Id::OpenFromServer
            | Id::GoTo
            | Id::CommandPalette
            | Id::CustomizeTags
            | Id::Help
            | Id::CheckForUpdates => enabled(!modal),
            Id::CloseNotebook => enabled(!modal && self.notebook().is_some()),
            Id::ShowNotebook => enabled(
                !modal
                    && self
                        .notebook()
                        .is_some_and(|library| library.folder().is_some()),
            ),
            Id::NewSection | Id::NewSectionGroup => enabled(
                !modal
                    && self
                        .notebook()
                        .is_some_and(|library| library.catalog().is_some()),
            ),
            Id::NewPage
            | Id::NewSubpage
            | Id::CopyPageLink
            | Id::Find
            | Id::Search
            | Id::ExportPdf
            | Id::ExportSectionPdf
            | Id::Print => enabled(!modal && session.is_some()),
            Id::PageVersions => session.filter(|_| !modal).map_or(
                Status {
                    enabled: false,
                    checked: Some(false),
                },
                |session| Status {
                    enabled: !session.page_versions(session.space).is_empty(),
                    checked: Some(session.shown_history == Some(session.space)),
                },
            ),
            Id::FullPageView => Status {
                enabled: !welcome && !modal,
                checked: Some(self.full_page),
            },
            Id::Undo => enabled(writable && !field && self.view.editor.can_undo()),
            Id::Redo => enabled(writable && !field && self.view.editor.can_redo()),
            Id::Cut => enabled(writable && selected),
            Id::Copy => enabled(selected),
            Id::Paste => enabled(text && !field),
            Id::SelectAll => enabled(field || page),
            Id::Back => enabled(!modal && self.trail.open()[0]),
            Id::Forward => enabled(!modal && self.trail.open()[1]),
            Id::ZoomIn | Id::ZoomOut | Id::ActualSize => enabled(page),
            Id::Sidebar => Status {
                enabled: !welcome && !modal,
                checked: Some(self.sidebar),
            },
            Id::PageList => Status {
                enabled: !modal && session.is_some(),
                checked: Some(self.pages_open),
            },
            Id::SearchResults => Status {
                enabled: !modal && session.is_some(),
                checked: Some(matches!(
                    self.search.pane,
                    Some(crate::pane::Pane::Search { .. })
                )),
            },
            Id::FindTags => Status {
                enabled: !modal && session.is_some(),
                checked: Some(matches!(
                    self.search.pane,
                    Some(crate::pane::Pane::Tags { .. })
                )),
            },
            Id::PagesMatchTheme => Status {
                enabled: !modal,
                checked: Some(!self.light_pages),
            },
            Id::SnapToGrid => Status {
                enabled: !modal,
                checked: Some(self.view.snap_to_grid),
            },
            Id::HideSpelling => Status {
                enabled: !modal && self.spelling.is_some(),
                checked: Some(self.hide_spelling),
            },
            Id::Spelling => enabled(text && self.view.spelling.is_some()),
            Id::FormatPainter => Status {
                enabled: text,
                checked: Some(self.painter.is_some()),
            },
            Id::PageColor => enabled(writable && session.is_some()),
            Id::ScreenClipping => enabled(text && offered(id)),
            Id::RecordAudio | Id::RecordVideo => {
                let recording = match self.media {
                    Media::Recording { video, .. } => Some(video),
                    _ => None,
                };
                Status {
                    enabled: recording.is_some()
                        || text && !modal && !matches!(self.media, Media::Saving { .. }),
                    checked: Some(recording == Some(id == Id::RecordVideo)),
                }
            }
            Id::Transport(transport) => self.transport_status(transport),
            Id::InsertSpace => Status {
                enabled: writable,
                checked: Some(self.view.inserting_space()),
            },
            Id::SelectType => Status {
                enabled: page,
                checked: Some(tool == Tool::Select),
            },
            Id::Pen | Id::Eraser | Id::Lasso | Id::Shape(_) => Status {
                enabled: writable,
                checked: Some(match (id, tool) {
                    (Id::Pen, Tool::Pen(_))
                    | (Id::Eraser, Tool::Eraser)
                    | (Id::Lasso, Tool::Lasso) => true,
                    (Id::Shape(kind), Tool::Shape(shape, _)) => kind == shape,
                    _ => false,
                }),
            },
            Id::Table
            | Id::Picture
            | Id::Attachment
            | Id::Symbol
            | Id::Link
            | Id::Equation
            | Id::Date
            | Id::Time
            | Id::DateTime
            | Id::Highlight
            | Id::FontColor
            | Id::Indent
            | Id::Outdent
            | Id::ClearFormatting
            | Id::RemoveTags => enabled(text),
            Id::Toggle(toggle) => checked(format.toggles.contains(&toggle)),
            Id::Bullets => checked(format.bullets),
            Id::Numbering => checked(format.numbering),
            Id::Align(alignment) => checked(format.alignment == Some(alignment)),
            Id::Tag(place) => match self.tags.get(place) {
                Some(tag) => checked(format.tags.contains(&(tag.stored(), place as u16))),
                None => Status::default(),
            },
        }
    }

    /// Applies `formatting`, remembering its font, colour or list style as the toolbar's
    /// last pick.
    pub(crate) fn format(&mut self, formatting: Formatting) -> Result<(), Box<dyn Error>> {
        let kept = self.toolbar.clone();
        let pens = &mut self.toolbar;
        match &formatting {
            Formatting::Font(name) => crate::remember(&mut self.recent_fonts, name.clone()),
            Formatting::Highlight(color) => pens.highlight = *color,
            Formatting::Color(color) => pens.font_color = *color,
            Formatting::List(Some(ListStyle::Bullet(place))) => {
                crate::remember(&mut pens.bullets, *place);
            }
            Formatting::List(Some(ListStyle::Number(place))) => {
                crate::remember(&mut pens.numbering, *place);
            }
            _ => {}
        }
        if matches!(formatting, Formatting::Font(_)) || self.toolbar != kept {
            self.save_settings();
        }
        let response = self.view.format(formatting)?;
        self.respond(response);
        Ok(())
    }

    /// The notebook the open section, or the notebook showing none, belongs to.
    fn notebook(&self) -> Option<&Arc<crate::Library>> {
        self.session
            .as_ref()
            .map(|session| &session.library)
            .or(self.sectionless.as_ref())
    }

    /// Runs `choice` where it applies now.
    pub(crate) fn run(&mut self, choice: Choice) -> Result<(), Box<dyn Error>> {
        if !self.status(&choice, &self.format_state()).enabled {
            return Ok(());
        }
        let format = Self::format;
        let id = match choice {
            Choice::Command(id) => id,
            Choice::Font(name) => return format(self, Formatting::Font(name)),
            Choice::Size(size) => return format(self, Formatting::FontSize(size)),
            Choice::Highlight(color) => return format(self, Formatting::Highlight(color)),
            Choice::Color(color) => return format(self, Formatting::Color(color)),
            Choice::List(style) => return format(self, Formatting::List(style)),
            Choice::Table([rows, columns]) => {
                let response = self.view.insert_table(rows, columns)?;
                self.respond(response);
                return Ok(());
            }
            Choice::PageColor(color) => {
                return self.paper_page(color, self.view.editor.rule_lines(), None);
            }
            Choice::Art(name) => {
                return self.with_art(name.and_then(canvas::template::find), |state, art| {
                    let editor = &state.view.editor;
                    state.paper_page(editor.page_color(), editor.rule_lines(), Some(art))
                });
            }
            Choice::RuleLines(lines) => {
                let lines = lines.map(|index| canvas::template::RULE_LINES[index].1);
                return self.paper_page(self.view.editor.page_color(), lines, None);
            }
            Choice::Pen(place) => {
                self.toolbar.pen = place;
                self.save_settings();
                Id::Pen
            }
        };
        let field = self.ui.focused_field();
        let response = match id {
            Id::Settings => {
                self.open_options();
                return Ok(());
            }
            Id::NewNotebook => Work::NewNotebook,
            Id::OpenNotebook => Work::OpenNotebook,
            Id::OpenFromServer => Work::OpenFromServer(None),
            Id::CloseNotebook => {
                Work::CloseNotebook(Arc::clone(self.notebook().ok_or("No notebook is open")?))
            }
            Id::NewSection | Id::NewSectionGroup => {
                let library = Arc::clone(self.notebook().ok_or("No notebook is open")?);
                let folder = self.session.as_ref().map_or_else(String::new, |session| {
                    crate::menus::folder(&session.tabs[session.tab].path)
                });
                Work::Structure(
                    library,
                    if id == Id::NewSection {
                        crate::manage::Structure::NewSection { folder }
                    } else {
                        crate::manage::Structure::NewGroup { folder }
                    },
                )
            }
            Id::NewPage => Work::NewPage { under: None },
            Id::NewSubpage => Work::NewPage {
                under: self.session.as_ref().map(|session| session.space),
            },
            Id::PageVersions => {
                let session = self.session.as_ref().ok_or("No notebook is open")?;
                Work::History {
                    page: session.space,
                    show: session.shown_history != Some(session.space),
                }
            }
            Id::CopyPageLink => {
                let space = self.session.as_ref().ok_or("No notebook is open")?.space;
                let link = self.page_link(space, None)?;
                self.clipboard.set_text(link)?;
                return Ok(());
            }
            Id::ShowNotebook => {
                platform::reveal(&self.notebook().ok_or("No notebook is open")?.location);
                return Ok(());
            }
            Id::ExportPdf | Id::ExportSectionPdf | Id::Print => {
                let scope = if id == Id::ExportSectionPdf {
                    crate::print::Scope::Section
                } else {
                    crate::print::Scope::Page
                };
                return self.print(scope, id != Id::Print);
            }
            Id::Undo | Id::Redo => {
                let response = self.view.undo(id == Id::Redo)?;
                self.respond(response);
                return Ok(());
            }
            Id::Cut | Id::Copy => {
                let response = self.view.copy(id == Id::Cut)?;
                self.respond(response);
                return Ok(());
            }
            Id::Paste => Work::Page(Request::Paste),
            Id::FormatPainter => {
                self.painter = match self.painter {
                    Some(_) => None,
                    None => Some(self.view.editor.painted_format()?),
                };
                return Ok(());
            }
            Id::SelectAll => {
                match field {
                    Some(field) => self.ui.focus_all(field),
                    None => {
                        let response = self.view.widen_selection()?;
                        self.respond(response);
                    }
                }
                return Ok(());
            }
            Id::Find => {
                return self.start_find(field == Some(search::field()));
            }
            Id::Search => {
                self.start_search();
                return Ok(());
            }
            Id::Spelling => {
                self.open_spelling_pane();
                return Ok(());
            }
            Id::SearchResults | Id::FindTags => {
                self.toggle_pane(id == Id::FindTags);
                return Ok(());
            }
            Id::GoTo | Id::CommandPalette => {
                let query = if id == Id::GoTo {
                    ""
                } else {
                    crate::palette::COMMANDS
                };
                ui::popup::open_with(&mut self.ui, crate::palette::id(), query);
                return Ok(());
            }
            Id::Back | Id::Forward => {
                self.travel(id == Id::Forward);
                return Ok(());
            }
            Id::ZoomIn | Id::ZoomOut | Id::ActualSize => {
                let zoom = match id {
                    Id::ZoomIn => self.view.zoom() * 1.1,
                    Id::ZoomOut => self.view.zoom() / 1.1,
                    _ => 1.0,
                };
                let response = self.view.set_zoom(zoom)?;
                self.respond(response);
                return Ok(());
            }
            Id::Sidebar => {
                self.sidebar = !self.sidebar;
                return Ok(());
            }
            Id::PageList => {
                self.pages_open = !self.pages_open;
                return Ok(());
            }
            Id::FullPageView => {
                self.full_page = !self.full_page;
                return Ok(());
            }
            // The menu bar and keys open the toolbar's gallery.
            Id::Table | Id::PageColor => {
                let name = if id == Id::Table {
                    "table"
                } else {
                    "page color"
                };
                self.ui.open_popup(crate::toolbar_popup(name));
                return Ok(());
            }
            Id::Picture => {
                let Some(path) = platform::pick_file("Insert Picture", &crate::PICTURE_TYPES)
                else {
                    return Ok(());
                };
                return self.insert_picture(std::fs::read(path)?);
            }
            Id::Attachment => {
                let Some(path) = platform::pick_file("Attach File", &[]) else {
                    return Ok(());
                };
                return self.attach(&path, None);
            }
            Id::ScreenClipping => {
                #[cfg(target_os = "macos")]
                platform::clip_screen(self.proxy.clone());
                return Ok(());
            }
            Id::Symbol => {
                platform::character_palette();
                return Ok(());
            }
            Id::HideSpelling => {
                self.hide_spelling = !self.hide_spelling;
                self.show_spelling();
                self.save_settings();
                return Ok(());
            }
            Id::SnapToGrid => {
                self.view.snap_to_grid = !self.view.snap_to_grid;
                self.save_settings();
                return Ok(());
            }
            Id::PagesMatchTheme => {
                self.light_pages = !self.light_pages;
                self.follow_color_scheme();
                self.save_settings();
                return Ok(());
            }
            Id::RecordAudio | Id::RecordVideo => return self.record(id == Id::RecordVideo),
            Id::Transport(transport) => return self.run_transport(transport),
            Id::InsertSpace => {
                let response = self.view.insert_space();
                self.respond(response);
                return Ok(());
            }
            Id::SelectType | Id::Pen | Id::Eraser | Id::Lasso | Id::Shape(_) => {
                let pens = self.pens();
                let tool = match id {
                    Id::Pen => Tool::Pen(pens[self.toolbar.pen.min(pens.len() - 1)]),
                    Id::Eraser => Tool::Eraser,
                    Id::Lasso => Tool::Lasso,
                    Id::Shape(kind) => Tool::Shape(kind, pens[0].shape()),
                    _ => Tool::Select,
                };
                let response = self.view.set_tool(tool);
                self.respond(response);
                return Ok(());
            }
            Id::Link => {
                self.open_link_dialog();
                return Ok(());
            }
            Id::Equation => {
                let response = self.view.insert_equation()?;
                self.respond(response);
                return Ok(());
            }
            // OneNote inserts the system's short date or time and a space.
            Id::Date | Id::Time | Id::DateTime => {
                let now = crate::filetime();
                let date = platform::short_date(now);
                let [_, time] = platform::date_text(now);
                let text = match id {
                    Id::Date => date,
                    Id::Time => time,
                    _ => format!("{date} {time}"),
                };
                let response = self.view.insert_text(format!("{text} "))?;
                self.respond(response);
                return Ok(());
            }
            Id::Help => {
                platform::reveal(HELP);
                return Ok(());
            }
            Id::CheckForUpdates => {
                self.updates.check_now();
                return Ok(());
            }
            Id::Toggle(toggle) => return format(self, Formatting::Toggle(toggle)),
            // The highlighter and font colour apply their last pick.
            Id::Highlight => return format(self, Formatting::Highlight(self.toolbar.highlight)),
            Id::FontColor => return format(self, Formatting::Color(self.toolbar.font_color)),
            Id::Bullets => return format(self, Formatting::Bullets),
            Id::Numbering => return format(self, Formatting::Numbering),
            Id::Align(alignment) => return format(self, Formatting::Align(alignment)),
            Id::Indent => return format(self, Formatting::Indent),
            Id::Outdent => return format(self, Formatting::Outdent),
            Id::ClearFormatting => return format(self, Formatting::Clear),
            Id::Tag(place) => {
                let tag = self.tags[place].clone();
                format(self, Formatting::Tag(tag, place as u16))?;
                self.keep_tag_art(place);
                return Ok(());
            }
            Id::CustomizeTags => {
                self.open_customize_tags();
                return Ok(());
            }
            Id::RemoveTags => return format(self, Formatting::RemoveTags),
        };
        self.commands.push(response);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLATFORMS: [Platform; 3] = [Platform::MacOs, Platform::Gtk, Platform::Windows];

    #[test]
    fn section_pen_inks_in_the_light_accent() {
        let section = draw::hsl(200.0, 0.6, 0.6);
        let [red, green, blue] = draw::srgb_bytes(ui::Theme::light().section(section).accent);
        let expected = u32::from_le_bytes([red, green, blue, 0]);
        assert_eq!(
            canvas::interaction::ink::pens(section)[0].color,
            Some(expected)
        );
        assert_eq!(expected, 0x00_b8_89_2e);
    }

    #[test]
    fn each_chord_runs_one_command_on_each_platform() {
        for platform in PLATFORMS {
            let chords: Vec<_> = COMMANDS
                .iter()
                .flat_map(|command| {
                    command
                        .chords(platform)
                        .iter()
                        .map(move |chord| (*chord, command.title))
                })
                .chain((0..9).filter_map(|place| Some((tag_chord(place)?, "a tag"))))
                .collect();
            for (at, (chord, title)) in chords.iter().enumerate() {
                if let Some((_, other)) =
                    chords[at + 1..].iter().find(|(listed, _)| listed == chord)
                {
                    panic!(
                        "{platform:?}: {} runs both {title} and {other}",
                        chord.label(platform)
                    );
                }
            }
        }
    }

    #[test]
    fn each_command_and_choice_is_listed_once() {
        for (at, command) in COMMANDS.iter().enumerate() {
            assert!(
                COMMANDS[at + 1..]
                    .iter()
                    .all(|other| other.id != command.id),
                "{:?}",
                command.id
            );
        }
        let choices: Vec<_> = choices().collect();
        for (at, choice) in choices.iter().enumerate() {
            assert_eq!(index(choice), at, "{choice:?}");
        }
    }

    #[test]
    fn keys_find_their_command_as_each_platform_types_them() {
        let find = |platform, key: &str, shift, option, control, command| {
            let modifiers = Modifiers {
                shift,
                option,
                control,
                command,
            };
            ran(
                pressed(&Key::Character(key.into()), modifiers, platform)?,
                platform,
            )
        };
        let mac = Platform::MacOs;
        assert_eq!(
            find(mac, "b", false, false, false, true),
            Some(Id::Toggle(Toggle::Bold))
        );
        assert_eq!(
            find(mac, "N", true, false, false, true),
            Some(Id::ClearFormatting)
        );
        assert_eq!(find(mac, "+", true, false, false, true), Some(Id::ZoomIn));
        assert_eq!(find(mac, "=", false, false, false, true), Some(Id::ZoomIn));
        assert_eq!(
            find(mac, "+", true, true, false, true),
            Some(Id::Toggle(Toggle::Superscript))
        );
        assert_eq!(
            find(mac, "=", false, false, true, false),
            Some(Id::Equation)
        );
        assert_eq!(
            find(mac, "0", false, false, true, true),
            Some(Id::RemoveTags)
        );
        assert_eq!(find(mac, "7", false, false, false, true), Some(Id::Tag(6)));
        assert_eq!(find(mac, "q", false, false, false, true), None);
        // The highlighter's chord runs the command that applies its last pick.
        assert_eq!(
            find(mac, "h", false, false, true, true),
            Some(Id::Highlight)
        );
        // Control comes with the shortcut modifier off macOS.
        let gtk = Platform::Gtk;
        assert_eq!(
            find(gtk, "b", false, false, true, true),
            Some(Id::Toggle(Toggle::Bold))
        );
        assert_eq!(
            find(gtk, "+", true, false, true, true),
            Some(Id::Toggle(Toggle::Superscript))
        );
        assert_eq!(
            find(gtk, "=", false, false, true, true),
            Some(Id::Toggle(Toggle::Subscript))
        );
        assert_eq!(find(gtk, "+", true, true, true, true), Some(Id::ZoomIn));
        assert_eq!(find(gtk, "D", true, true, false, false), Some(Id::Date));
        assert_eq!(find(gtk, "h", false, true, true, true), Some(Id::Highlight));
        assert_eq!(
            find(gtk, "y", false, false, true, true),
            cfg!(windows).then_some(Id::Redo)
        );
        let indent = pressed(
            &Key::Named(NamedKey::ArrowRight),
            Modifiers {
                shift: true,
                option: true,
                ..Modifiers::default()
            },
            gtk,
        );
        assert!(
            command(Id::Indent)
                .pc
                .iter()
                .any(|chord| Some(*chord) == indent)
        );
    }

    #[test]
    fn chords_read_as_each_platform_labels_them() {
        let label = |id, platform| command(id).chords(platform)[0].label(platform);
        assert_eq!(
            label(Id::Toggle(Toggle::Superscript), Platform::MacOs),
            "⌥⌘+"
        );
        assert_eq!(
            label(Id::Toggle(Toggle::Subscript), Platform::MacOs),
            "⌃⌥⌘="
        );
        assert_eq!(label(Id::ClearFormatting, Platform::MacOs), "⇧⌘N");
        assert_eq!(
            label(Id::Toggle(Toggle::Superscript), Platform::Gtk),
            "Ctrl+Shift+="
        );
        assert_eq!(label(Id::Indent, Platform::Gtk), "Alt+Shift+Right");
        assert_eq!(label(Id::Date, Platform::Windows), "Alt+Shift+D");
    }

    /// Each toolbar button chooses a command or a list's entry, so it runs, enables and
    /// checks as the menu bar and keyboard do.
    #[test]
    fn toolbar_buttons_run_commands() {
        let source = include_str!("main.rs");
        let start = source
            .find("    fn tools(")
            .expect("The toolbar's groups are built in main.rs");
        let body = &source[start..];
        let body = &body[..body[1..]
            .find("\n    fn ")
            .map_or(body.len(), |end| end + 1)];
        assert!(body.contains("self.choose("));
        for effect in [
            "self.respond(",
            "self.changed",
            "self.view.format(",
            "self.view.set_zoom(",
            "self.view.insert",
            "self.open_link_dialog(",
        ] {
            assert!(
                !body.contains(effect),
                "A toolbar button acts through `{effect}`; have it choose a command instead"
            );
        }
    }
}
