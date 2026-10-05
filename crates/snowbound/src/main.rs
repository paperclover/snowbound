#![cfg_attr(windows, windows_subsystem = "windows")]
// The browser drives `State` from `web.rs`; launch, replays and screenshots are the desktop's.
#![cfg_attr(target_arch = "wasm32", allow(dead_code))]
#[cfg(target_os = "macos")]
mod aqua;
mod art;
mod attachment;
mod background;
#[cfg(target_os = "linux")]
#[path = "clipboard_linux.rs"]
mod clipboard;
mod commands;
#[cfg(all(test, feature = "wgpu", not(windows)))]
mod conflict_render;
mod crash;
#[cfg(target_os = "linux")]
#[path = "desktop_linux.rs"]
mod desktop;
#[cfg(target_os = "linux")]
#[path = "dialog_linux.rs"]
mod dialog;
mod guide;
mod history;
#[cfg_attr(not(target_os = "macos"), path = "icloud_linux.rs")]
#[cfg_attr(target_os = "macos", path = "icloud_macos.rs")]
mod icloud;
#[cfg(not(target_arch = "wasm32"))]
#[cfg_attr(target_os = "macos", allow(dead_code))]
mod instance;
mod keys;
mod library;
mod link;
#[cfg(feature = "live")]
mod live;
#[cfg(target_os = "linux")]
#[path = "loader_linux.rs"]
mod loader;
mod manage;
#[cfg_attr(target_os = "linux", path = "media_linux.rs")]
#[cfg_attr(target_os = "macos", path = "media_macos.rs")]
#[cfg_attr(windows, path = "media_windows.rs")]
#[cfg_attr(target_arch = "wasm32", path = "media_web.rs")]
mod media;
mod meeting;
#[cfg(target_os = "macos")]
mod menubar;
mod menus;
mod navigation;
mod options;
mod palette;
mod pane;
mod paste;
#[cfg_attr(target_os = "linux", path = "linux.rs")]
#[cfg_attr(target_os = "macos", path = "macos.rs")]
#[cfg_attr(windows, path = "windows.rs")]
#[cfg_attr(target_arch = "wasm32", path = "web.rs")]
mod platform;
mod prefetch;
mod print;
#[cfg_attr(target_os = "linux", path = "print_linux.rs")]
#[cfg_attr(target_os = "macos", path = "print_macos.rs")]
#[cfg_attr(windows, path = "print_windows.rs")]
#[cfg_attr(target_arch = "wasm32", path = "print_web.rs")]
mod printer;
mod properties;
mod protection;
mod recording;
mod recycle;
mod rename;
mod save_as;
mod screenshot;
mod search;
mod server;
mod settings;
#[cfg(feature = "live")]
mod share;
mod sidebar;
#[cfg_attr(target_os = "linux", path = "spell_linux.rs")]
#[cfg_attr(target_os = "macos", path = "spell_macos.rs")]
#[cfg_attr(windows, path = "spell_windows.rs")]
#[cfg_attr(target_arch = "wasm32", path = "spell_web.rs")]
mod spell;
#[cfg_attr(not(feature = "wgpu"), path = "surface_gl.rs")]
#[cfg_attr(all(target_os = "macos", feature = "wgpu"), path = "surface_macos.rs")]
#[cfg_attr(windows, path = "surface_windows.rs")]
#[cfg_attr(target_arch = "wasm32", path = "surface_web.rs")]
mod surface;
mod symbol;
mod sync;
mod tags;
mod templates;
mod themes;
mod undo;
mod unpack;
mod unread;
mod update;
#[cfg_attr(target_arch = "wasm32", path = "watch_web.rs")]
mod watch;

#[cfg(not(target_arch = "wasm32"))]
use accesskit_winit::Adapter as AccessAdapter;
use canvas::editor::Clip;
#[cfg(not(target_arch = "wasm32"))]
use canvas::editor::{DEFAULT_OUTLINE_WIDTH, TextOutline};
use canvas::gpu::colorref;
use canvas::gpu::page::PageScene;
use canvas::interaction::{Cursor, PageView, Place, Request, Response, TextColors, accessibility};
use canvas::{date::DateField, document::TextDocument, editor::CanvasEditor, layout::TextEngine};
use draw::Renderer;
use library::Library;
use onestore::ExGuid;
use onestore::document::Format;
use onestore::page::Page;
use onestore::page::ink::ShapeKind;
use onestore::page::text::Paragraph;
#[cfg(target_arch = "wasm32")]
use platform::{AccessAdapter, ActiveEventLoop, EventLoopProxy, Window};
use std::{
    collections::{HashMap, HashSet},
    error::Error,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
};
use ui::{Axis, Flags, Id, Spec, Theme, Ui, children, fill, fit, px};
use web_time::Instant;
#[cfg(not(target_arch = "wasm32"))]
use winit::{
    application::ApplicationHandler,
    event::{ElementState, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoopProxy},
    keyboard::ModifiersState,
    window::{Window, WindowId},
};
use winit::{
    dpi::{LogicalPosition, LogicalSize},
    event::{Ime, MouseButton},
    keyboard::{Key, NamedKey},
    window::CursorIcon,
};

/// Height of the title bar the toolbar's buttons centre in: a unified compact toolbar's on
/// macOS, whose traffic lights they line up with.
const TITLE: f32 = 38.0;
/// Height of the toolbar's row, which is the title bar where the platform lets it. Its foot
/// is short of the title bar's, so the tabs below stand as near its buttons as a tab bar
/// under a compact toolbar does.
const TOOLBAR: f32 = TITLE - 4.0;
/// Space between toolbar groups.
const GAP: f32 = 6.0;
/// A tool face's room beside its buttons.
const FACE_PAD: f32 = 5.0;
/// How far a tool face stands above and below its buttons.
const FACE_RISE: f32 = 3.0;
/// Space between groups on faces of their own.
const FACE_GAP: f32 = 2.0;
const TAB_ROW: f32 = 28.0;
/// Width of the section colour around the page, and the rows' margin at the window's sides.
const FRAME: f32 = 6.0;
const PAGE_LIST: f32 = 240.0;
/// The least window: the page list and a page beside it. The window widens past it to the
/// toolbar's narrowest row, the window's controls beside it.
const MIN_SIZE: [f32; 2] = [600.0, 400.0];
/// Extensions of the pictures Insert, Picture offers: those the page both stores and draws.
const PICTURE_TYPES: [&str; 4] = ["png", "jpg", "jpeg", "gif"];
/// Fonts the font box offers first, OneNote's default leading.
const FONTS: [&str; 4] = ["Calibri", "Arial", "Times New Roman", "Courier New"];
/// Picks the font box and the list galleries offer as recent, at most, as OneNote's
/// galleries do.
const RECENT: usize = 5;
/// Height of a line in a numbering gallery's previews.
const NUMBER_LINE: f32 = 16.0;
/// Font sizes the size box offers, OneNote's list in points.
const SIZES: [f32; 17] = [
    8.0, 9.0, 10.0, 10.5, 11.0, 12.0, 14.0, 16.0, 18.0, 20.0, 22.0, 24.0, 26.0, 28.0, 36.0, 48.0,
    72.0,
];
/// OneNote's highlight colours, COLORREF.
const HIGHLIGHTS: [(u32, &str); 15] = [
    (0x00ffff, "Yellow"),
    (0x00ff00, "Bright Green"),
    (0xffff00, "Turquoise"),
    (0xff00ff, "Pink"),
    (0xff0000, "Blue"),
    (0x0000ff, "Red"),
    (0x800000, "Dark Blue"),
    (0x808000, "Teal"),
    (0x008000, "Green"),
    (0x800080, "Violet"),
    (0x000080, "Dark Red"),
    (0x008080, "Dark Yellow"),
    (0x808080, "Gray 50%"),
    (0xc0c0c0, "Gray 25%"),
    (0x000000, "Black"),
];
/// Office's theme and standard font colours, COLORREF, with the names their tooltips give.
const FONT_COLORS: [(u32, &str); 20] = [
    (0xffffff, "White, Background 1"),
    (0x000000, "Black, Text 1"),
    (0xe1ecee, "Tan, Background 2"),
    (0x7d491f, "Dark Blue, Text 2"),
    (0xbd814f, "Blue, Accent 1"),
    (0x4d50c0, "Red, Accent 2"),
    (0x59bb9b, "Olive Green, Accent 3"),
    (0xa26480, "Purple, Accent 4"),
    (0xc6ac4b, "Aqua, Accent 5"),
    (0x4696f7, "Orange, Accent 6"),
    (0x0000c0, "Dark Red"),
    (0x0000ff, "Red"),
    (0x00c0ff, "Orange"),
    (0x00ffff, "Yellow"),
    (0x50d092, "Light Green"),
    (0x50b000, "Green"),
    (0xf0b000, "Light Blue"),
    (0xc07000, "Blue"),
    (0x602000, "Dark Blue"),
    (0xa03070, "Purple"),
];
/// Height of a page tab's row, whose tab leaves `ROW_GAP` below it so tabs stand apart
/// while the gaps still take the pointer.
const ROW: f32 = 29.0;
const ROW_GAP: f32 = 3.0;
/// Space between the page and a page tab that isn't open.
const PILL_MARGIN: f32 = 3.0;
/// Why the platform's date dialog could not change the page's date.
#[cfg_attr(windows, allow(dead_code))]
const DATE_UNCHOSEN: &str = "Choose another date or time.";
const DATE_OUT_OF_RANGE: &str = "This date is outside the notebook's supported range.";

type Continuation = Box<dyn FnOnce(&mut State) -> Result<(), Box<dyn Error>> + Send>;

/// What follows a dialog's answer, run on the event loop. Dialogs never wait on the event
/// loop's thread, so the window keeps drawing and other apps can paste what it copied while
/// one is open; one cancelled drops its reply unanswered.
struct Reply<T>(Box<dyn FnOnce(T) + Send>);

impl<T: Send + 'static> Reply<T> {
    fn new(
        proxy: &EventLoopProxy<UserEvent>,
        then: impl FnOnce(&mut State, T) -> Result<(), Box<dyn Error>> + Send + 'static,
    ) -> Self {
        let proxy = proxy.clone();
        Self(Box::new(move |answer| {
            let _ = proxy.send_event(UserEvent::Then(Box::new(move |state| then(state, answer))));
        }))
    }

    fn send(self, answer: T) {
        (self.0)(answer);
    }

    /// The reply that sends `convert`'s answer here.
    #[cfg(target_os = "linux")]
    fn map<U>(self, convert: impl FnOnce(U) -> T + Send + 'static) -> Reply<U> {
        Reply(Box::new(move |answer| self.send(convert(answer))))
    }

    /// Sends what `ask` answers, waiting on its dialog on a thread of its own.
    #[cfg(windows)]
    fn after(self, ask: impl FnOnce() -> Option<T> + Send + 'static) {
        std::thread::spawn(move || {
            if let Some(answer) = ask() {
                self.send(answer);
            }
        });
    }
}

/// Asks whether to go ahead with `action`, offering `cancel`, the default, too.
fn confirm(message: &str, detail: &str, cancel: &str, action: &str, reply: Reply<()>) {
    let pressed = Reply(Box::new(move |pressed| {
        if pressed == 0 {
            reply.send(());
        }
    }));
    platform::choose(message, detail, &[action, cancel], pressed);
}

enum UserEvent {
    Quit,
    /// The Wayland compositor asked something of the clipboard.
    #[cfg(target_os = "linux")]
    Clipboard,
    /// Quits without asking, as after the user agreed to discard a temporary page.
    #[cfg(not(target_arch = "wasm32"))]
    Exit,
    /// Text AppKit inserts outside key events, such as the character palette's.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    InsertText(String),
    /// A picture to insert at the caret, such as a screen clipping.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Picture(Vec<u8>),
    #[cfg(not(target_arch = "wasm32"))]
    Accessibility(accesskit_winit::Event),
    /// The section's synchronization thread reported an event.
    Sync,
    /// An update check moved on.
    Update,
    /// Scripted input from `SNOWBOUND_REPLAY`.
    Replay(Replay),
    /// A page background finished rasterizing on its worker thread.
    Redraw,
    /// A menu bar item was chosen.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Choose(commands::Choice),
    /// The desktop's colours changed, where the window system doesn't say so itself.
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    Appearance,
    /// What follows work a thread of its own finished, run on the event loop's.
    Then(Continuation),
    /// The iCloud account signed out, signed in or switched, or iCloud Drive was turned off.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    ICloudAccount,
    /// The app's iCloud Drive folder was looked up, or a notebook came or went at its top.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    ICloudFolder,
    /// The system or a later launch asked to open notebook folders, tables of contents or
    /// sections, as the Finder does with a double-clicked file; none brings the window forward.
    Open(Vec<PathBuf>),
    /// The activation token a later launch's launcher gave it, which lets the window come
    /// forward despite the desktop's focus-stealing prevention.
    #[cfg(target_os = "linux")]
    Activate(String),
    /// Draws with this backend from now on.
    Renderer(settings::Backend),
}

/// Asks the event loop for a frame from any thread.
struct Redraw(EventLoopProxy<UserEvent>);

enum Clipboard {
    System(platform::Clipboard),
    /// What was last copied, text alone or a clip.
    Memory(String, Option<Clip>),
}

impl Clipboard {
    fn set_text(&mut self, text: String) -> Result<(), Box<dyn Error>> {
        match self {
            Self::System(clipboard) => clipboard.set_text(text)?,
            Self::Memory(held, clip) => (*held, *clip) = (text, None),
        }
        Ok(())
    }

    fn set(&mut self, clip: Clip) -> Result<(), Box<dyn Error>> {
        match self {
            Self::System(clipboard) => clipboard.set(&paste::Copied::new(&clip)?)?,
            Self::Memory(held, kept) => (*held, *kept) = (clip.text(), Some(clip)),
        }
        Ok(())
    }

    fn pasted(&mut self) -> Option<paste::Pasted> {
        match self {
            Self::System(clipboard) => {
                let files = clipboard.get_files();
                let text = |clipboard: &mut platform::Clipboard| {
                    clipboard.get_text().ok().filter(|text| !text.is_empty())
                };
                if !files.is_empty() {
                    Some(paste::Pasted::Files(files))
                } else if let Some(clip) = clipboard.get_clip().as_deref().and_then(Clip::decode) {
                    Some(paste::Pasted::Clip(clip))
                } else if let Some(html) = clipboard.get_html() {
                    Some(paste::Pasted::Page(html, text(clipboard)))
                } else if let Some(text) = text(clipboard) {
                    Some(paste::Pasted::Text(text))
                } else {
                    clipboard.get_picture().map(paste::Pasted::Picture)
                }
            }
            Self::Memory(held, clip) => Some(match clip {
                Some(clip) => paste::Pasted::Clip(clip.clone()),
                None => paste::Pasted::Text(held.clone()),
            }),
        }
    }
}

impl std::task::Wake for Redraw {
    fn wake(self: Arc<Self>) {
        let _ = self.0.send_event(UserEvent::Redraw);
    }
}

#[derive(Debug)]
enum Replay {
    Input(ui::Event),
    /// A trackpad pinch scaling the page by the factor.
    Pinch(f32),
    /// Paints the next frame into a PNG as well as the window.
    Snapshot(PathBuf),
    /// Waits for nothing to be on its way, then writes the window's accessibility tree as
    /// text where given, and answers.
    Settle(Option<PathBuf>, std::sync::mpsc::Sender<()>),
    /// A frame during a wait, as a visible window's display would ask for.
    Tick,
    /// What the window sent itself when it found nothing on its way, at that instant: once it
    /// arrives, so has every event sent before.
    Mark(Instant),
    Appearance(winit::window::Theme),
    /// Resizes the window's content, in points.
    Resize([f32; 2]),
    /// Draws with this backend from then on, as Options' Renderer does.
    Renderer(settings::Backend),
    Quit,
}

#[cfg(not(target_arch = "wasm32"))]
impl From<accesskit_winit::Event> for UserEvent {
    fn from(event: accesskit_winit::Event) -> Self {
        Self::Accessibility(event)
    }
}

/// Tooling drives the app, with `--screenshot` or `SNOWBOUND_REPLAY`: it reads only the
/// settings and iCloud folder it is given, never the account's own.
#[cfg(not(target_arch = "wasm32"))]
static AUTOMATED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(not(target_arch = "wasm32"))]
fn automated() -> bool {
    AUTOMATED.load(Ordering::Relaxed)
}

/// With `SNOWBOUND_PROFILE` set, prints how long a phase of the frame took since `start`.
fn lap(phase: &str, start: Instant) {
    static PROFILE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *PROFILE.get_or_init(|| std::env::var_os("SNOWBOUND_PROFILE").is_some()) {
        eprintln!("profile\t{phase}\t{}", start.elapsed().as_micros());
    }
}

fn trace_input(event: &impl std::fmt::Debug) {
    if std::env::var_os("SNOWBOUND_TRACE_INPUT").is_some() {
        eprintln!("Input {:?}: {event:?}", web_time::SystemTime::now());
    }
}

fn cursor_icon(cursor: Cursor) -> CursorIcon {
    match cursor {
        Cursor::Default => CursorIcon::Default,
        Cursor::Text => CursorIcon::Text,
        Cursor::Pointer => CursorIcon::Pointer,
        Cursor::Move => platform::move_cursor(),
        Cursor::EwResize => CursorIcon::EwResize,
        Cursor::NsResize => CursorIcon::NsResize,
        Cursor::NwseResize => CursorIcon::NwseResize,
        Cursor::NeswResize => CursorIcon::NeswResize,
        Cursor::RowResize => CursorIcon::RowResize,
        Cursor::ColResize => CursorIcon::ColResize,
        Cursor::Crosshair => CursorIcon::Crosshair,
    }
}

#[cfg(not(target_arch = "wasm32"))]
struct App {
    proxy: EventLoopProxy<UserEvent>,
    input: Option<Input>,
    substitutes: Vec<PathBuf>,
    /// Where `--screenshot` writes its PNGs, drawn from a window never shown.
    screenshot: Option<PathBuf>,
    /// What the window starts from besides its input, until it opens.
    launch: Option<settings::Launch>,
    /// Files and folders to open, as File, Open does, once the window opens.
    opening: Vec<PathBuf>,
    state: Option<State>,
    /// The staged update to swap in once the app has quit.
    restart: Option<PathBuf>,
    startup_error: Option<Box<dyn Error>>,
}

enum Input {
    Notes {
        document: TextDocument,
        width: f32,
        reference: Option<Page>,
    },
    Page(Page),
    Section {
        file: PathBuf,
        title: String,
    },
    /// The open notebooks by location, showing `current`; none shows the first run's
    /// welcome.
    Notebooks {
        locations: Vec<String>,
        current: Option<String>,
    },
}

impl Input {
    /// Lists the notebook folder `root` and shows it first, where the window shows notebooks.
    fn show(&mut self, root: &Path) -> std::io::Result<()> {
        if let Input::Notebooks { locations, current } = self {
            let location = notebook::fs::absolute(root)?.to_string_lossy().into_owned();
            if !locations.contains(&location) {
                locations.push(location.clone());
            }
            *current = Some(location);
        }
        Ok(())
    }
}

/// Asks the event loop to poll sections when their synchronization reports.
fn notify(proxy: EventLoopProxy<UserEvent>) -> impl Fn() + Send + 'static {
    move || {
        let _ = proxy.send_event(UserEvent::Sync);
    }
}

/// Asks the event loop to list the app's iCloud Drive folder again.
fn icloud_listed(proxy: EventLoopProxy<UserEvent>) -> impl Fn() + Send + Sync + 'static {
    move || {
        let _ = proxy.send_event(UserEvent::ICloudFolder);
    }
}

/// The open section and the page the editor shows.
struct Session {
    section: notebook::session::Section,
    /// The notebook the section belongs to.
    library: Arc<Library>,
    /// The sections of the section's folder, and which is this one.
    tabs: Vec<library::Tab>,
    tab: usize,
    /// Spaces, titles and outline levels in section order.
    pages: Vec<(ExGuid, String, u32)>,
    space: ExGuid,
    /// The section's sync status as last read.
    sync: notebook::session::SyncStatus,
    /// Each page's conflict pages, the versions a merge could not take.
    conflicts: Vec<(ExGuid, Vec<onestore::ConflictPage>)>,
    /// The page whose conflict pages the list shows beneath it.
    shown: Option<ExGuid>,
    /// Which of the open conflict page's conflicting changes is selected, in page order.
    change: Option<usize>,
    /// Each page's versions, newest first.
    history: Vec<(ExGuid, Vec<onestore::PageVersion>)>,
    /// The page whose versions the list shows beneath it (Show Page Versions).
    shown_history: Option<ExGuid>,
    /// The version of page `space` open, by its context.
    version: Option<ExGuid>,
}

/// The information bar above a page with conflict pages, or above one of them.
#[derive(Clone, Copy)]
enum Bar {
    Page {
        page: ExGuid,
        shown: bool,
    },
    Version {
        page: ExGuid,
        version: ExGuid,
    },
    /// Above a page version, of a section `grouped` in a section group.
    History {
        page: ExGuid,
        version: ExGuid,
        grouped: bool,
    },
}

impl Session {
    fn title(&self) -> &str {
        match self.conflict(self.space) {
            Some((_, version)) => &version.title,
            None => self
                .pages
                .iter()
                .find(|(space, ..)| *space == self.space)
                .map_or("", |(_, title, _)| title),
        }
    }

    fn versions(&self, page: ExGuid) -> &[onestore::ConflictPage] {
        self.conflicts
            .iter()
            .find(|(listed, _)| *listed == page)
            .map_or(&[], |(_, versions)| versions)
    }

    /// The page conflict page `space` belongs to, with it.
    fn conflict(&self, space: ExGuid) -> Option<(ExGuid, &onestore::ConflictPage)> {
        self.conflicts.iter().find_map(|(page, versions)| {
            Some((
                *page,
                versions.iter().find(|version| version.space == space)?,
            ))
        })
    }

    fn bar(&self) -> Option<Bar> {
        if let Some(version) = self.version {
            return Some(Bar::History {
                page: self.space,
                version,
                grouped: self.grouped(),
            });
        }
        match self.conflict(self.space) {
            Some((page, _)) => Some(Bar::Version {
                page,
                version: self.space,
            }),
            None => (!self.versions(self.space).is_empty()).then_some(Bar::Page {
                page: self.space,
                shown: self.shown == Some(self.space),
            }),
        }
    }

    fn refresh_conflicts(&mut self) -> Result<(), Box<dyn Error>> {
        self.conflicts = self.section.conflicts()?;
        if self
            .shown
            .is_some_and(|page| self.versions(page).is_empty())
        {
            self.shown = None;
        }
        Ok(())
    }

    /// The page in `space` as the editor shows it: a conflict page read-only, its
    /// conflicting changes highlighted.
    fn reader(
        &self,
        space: ExGuid,
    ) -> impl FnOnce() -> Result<Page, Box<dyn Error>> + Send + 'static {
        let replica = Arc::clone(self.section.replica());
        let objects = self
            .conflict(space)
            .map(|(_, version)| version.objects.clone());
        move || {
            let page = replica.page(space)?;
            Ok(match objects {
                Some(objects) => canvas::conflict::highlighted(page, &objects),
                None => page,
            })
        }
    }
}

/// What a loader thread read: a section and its page to show, another page of the open
/// section, or a notebook read again after a change, with the catalog path the open
/// section has in it.
enum Loaded {
    Section(Box<Session>, Page),
    /// A section just created, whose name opens for renaming as OneNote's New Section does.
    Created(Box<Session>, Page),
    Page(ExGuid, Page),
    /// A page's version, by the page's space and the version's context.
    Version(ExGuid, ExGuid, Page),
    /// A notebook read again after a change, at the section that stays open, if any.
    Library(Arc<Library>, Option<String>),
}

/// What an opening page replaces: the section, or the page of the open one.
enum Shown {
    Section(Box<Session>),
    Created(Box<Session>),
    Page(ExGuid),
    Version(ExGuid, ExGuid),
}

/// A loader thread's request number and what it read, its page laid out.
type Read = (u64, Result<Laid, String>);

enum Laid {
    Page(Box<Opening>),
    /// A notebook read again after a change, at the section that stays open, if any.
    Library(Arc<Library>, Option<String>),
}

/// The newest page read, laid out and waiting for the pictures it shows first, so it
/// never appears without them.
struct Opening {
    loaded: Shown,
    scene: (PageScene, [f32; 2]),
    editor: CanvasEditor,
    since: Instant,
    requested: Instant,
}

/// How long an opening page waits for its pictures before showing without them.
const HOLD: std::time::Duration = std::time::Duration::from_millis(200);

/// Work the interface asked for, done after the frame is built.
enum Command {
    /// Opens a notebook's section by catalog path, at the page it showed last.
    OpenSection(Arc<Library>, String),
    /// Asks for a picture for the tag New Tag or Modify Tag edits.
    TagPicture,
    /// Asks for a notebook folder and opens it.
    OpenNotebook,
    /// Asks for a server's address and opens a notebook on it; with a notebook's location,
    /// signs in again to open it.
    OpenFromServer(Option<String>),
    /// Asks for the code of a notebook another computer shares, and opens it.
    #[cfg(feature = "live")]
    OpenShared,
    /// Asks where to keep a new notebook and creates it.
    NewNotebook,
    /// Opens the Snowbound Guide from the user's documents, copying it there first.
    OpenGuide,
    /// Adds Snowbound to the app menu.
    #[cfg(target_os = "linux")]
    Install,
    /// Closes a notebook, keeping its files.
    CloseNotebook(Arc<Library>),
    /// Changes a notebook's sections and groups.
    Structure(Arc<Library>, manage::Structure),
    /// Adds a page at the end of the open section, or a subpage of a page, titled `title`
    /// or untitled, and edits its title.
    NewPage {
        under: Option<ExGuid>,
        title: String,
    },
    /// Deletes pages of the open section to the notebook's recycle bin.
    DeletePages(Vec<ExGuid>),
    /// The page's context menu item of this text, at the caret or on the file selected.
    Text(&'static str),
    /// Restores or deletes for good pages of the recycle bin's open section.
    Recycle(recycle::Request),
    /// Moves or indents pages of the open section.
    Pages(Vec<onestore::PageEdit>),
    /// Moves a page of the open section to the end of another section of its folder.
    MovePage {
        space: ExGuid,
        path: String,
    },
    /// Gives the open page a template's background.
    Template(&'static str),
    OpenPage(ExGuid),
    /// Shows a page's conflict pages in the list and opens the newest, or hides them.
    Versions {
        page: ExGuid,
        show: bool,
    },
    DeleteVersion {
        page: ExGuid,
        version: ExGuid,
    },
    /// Copies a conflict page, as a page of its own, to the end of section tab `tab`.
    CopyVersion {
        version: ExGuid,
        tab: usize,
    },
    /// Selects the next, or previous, conflicting change on the open conflict page.
    SelectChange {
        forward: bool,
    },
    /// Shows or hides a page's versions in the list.
    History {
        page: ExGuid,
        show: bool,
    },
    OpenVersion {
        page: ExGuid,
        version: ExGuid,
    },
    RestoreVersion {
        page: ExGuid,
        version: ExGuid,
    },
    DeletePageVersion {
        page: ExGuid,
        version: ExGuid,
    },
    DeleteAllVersions(history::Scope),
    Page(Request),
    /// Runs a command, or one of a list's entries, after the frame's input.
    Choose(commands::Choice),
}

struct State {
    window: Arc<Window>,
    /// The user name edits are stored under, as OneNote names the Office user.
    author: String,
    proxy: EventLoopProxy<UserEvent>,
    /// Results of reads done off the frame thread, by request number.
    loads: (mpsc::Sender<Read>, mpsc::Receiver<Read>),
    /// Lays out pages the loader threads read, off the frame.
    layouts: Arc<Mutex<TextEngine>>,
    /// The newest read requested; older ones are dropped when they finish.
    loading: u64,
    opening: Option<Opening>,
    /// The section tab, if another, that the newest read opens, and when it was asked for.
    switching: Option<(Option<usize>, Instant)>,
    prefetch: prefetch::Prefetch,
    /// Where each page was left this run, by `Library::key` and page space, as OneNote
    /// returns to it until the notebook closes.
    places: HashMap<(String, ExGuid), Place>,
    /// The page each section showed last this run, by `Library::key`.
    last_pages: HashMap<String, ExGuid>,
    /// Pages visited, for Back and Forward.
    trail: navigation::Trail,
    /// What Undo and Redo take back across pages and sections.
    undo: undo::Timeline,
    swipe: navigation::Swipe,
    /// Asks for a frame when a worker thread finishes something the page shows.
    redraw: std::task::Waker,
    /// Dropped before `surface`, as an OpenGL renderer needs its context.
    renderer: Renderer,
    surface: surface::Surface,
    /// Whether the system's material shows behind the window, which a surface started
    /// again keeps.
    backdrop: bool,
    /// Options' Renderer, as the settings keep it.
    renderer_choice: settings::Backend,
    /// The backend frames are drawn with, and its adapter's name.
    drawing: (settings::Backend, String),
    /// Backends that failed to start this run, and why.
    unavailable: Vec<(settings::Backend, String)>,
    ui: Ui,
    view: PageView,
    /// The open notebooks in the sidebar's order.
    notebooks: Vec<Arc<Library>>,
    session: Option<Session>,
    /// A page not kept in any notebook, from a text file or `--page`.
    temporary: bool,
    /// Where the settings are saved; `None` leaves them as they were read.
    settings: Option<PathBuf>,
    /// The directory holding the sections' replicas.
    cache: PathBuf,
    /// Whether the notebook sidebar is expanded.
    sidebar: bool,
    /// Notebooks and section groups whose rows the sidebar folds, by `Library::key`.
    folded: HashSet<String>,
    /// The open context menu: what it was opened on, and where.
    menu: Option<(menus::Target, [f32; 2])>,
    /// The notebook the sync status popup shows, opened from its context menu at a point;
    /// `None` shows the open section's, below the toolbar's button.
    sync_notebook: Option<(Arc<Library>, [f32; 2])>,
    /// What the palette's actions menu is on while it is open.
    actions: Option<menus::Target>,
    /// An action on a page of another section, done once that section opens on the page.
    after_open: Option<(ExGuid, menus::Action)>,
    /// The Options dialog's choices while it is open.
    options: Option<options::Options>,
    /// The Themes dialog while it is open.
    themes: Option<themes::Dialog>,
    /// Print Preview and Settings: the choices last made, and the dialog's while open.
    printing: print::Printing,
    updates: update::Updates,
    /// The Link dialog's fields while it is open.
    link: Option<link::LinkDialog>,
    properties: Option<properties::Properties>,
    symbols: Option<symbol::Symbols>,
    /// Save As while it is open.
    save_as: Option<save_as::Dialog>,
    /// Unpack Notebook while it is open.
    unpacking: Option<unpack::Dialog>,
    /// Snowbound's own dialogs, first the one shown, where the desktop has none.
    #[cfg(target_os = "linux")]
    asking: std::collections::VecDeque<dialog::Dialog>,
    /// What has been read in each notebook, on this computer.
    reads: unread::Reads,
    #[cfg(feature = "live")]
    peers: live::Peers,
    /// Options' Live Share.
    live_options: settings::Live,
    /// Open Notebook from Server while it is open.
    server: Option<server::Connect>,
    /// Notebooks in iCloud Drive a thread is reading, or following the download of, by
    /// location.
    icloud_reading: HashSet<String>,
    /// The notebook locations a folder rename takes a notebook from and to, while it does.
    folder_rename: Option<(String, String)>,
    /// The notebook locations whose folders are on their way to the Trash.
    trashing: HashSet<String>,
    /// The iCloud notebook locations a listing missed, waiting to be confirmed gone.
    vanishing: HashSet<String>,
    /// The user's tag list, which the toolbar, menus and Ctrl+1 to Ctrl+9 apply.
    tags: Vec<canvas::editor::NoteTag>,
    /// The Customize Tags dialog's list while it is open.
    tag_list: Option<tags::TagList>,
    /// The page's context menu while it is open: what it was opened on, and where.
    text_menu: Option<(canvas::interaction::Context, [f32; 2])>,
    color_scheme: settings::ColorScheme,
    light_pages: bool,
    /// The system's spell checker, where it has one.
    spelling: Option<canvas::spelling::Spelling>,
    hide_spelling: bool,
    /// Options' "Use pen pressure sensitivity": a tablet pen's strokes follow its pressure.
    pen_pressure: bool,
    /// Options' "Page tabs appear on the left".
    page_tabs_left: bool,
    /// Options' "Navigation bar appears on the left" off: the notebooks on the right.
    navigation_bar_right: bool,
    /// Servers Open Notebook from Server signed in to, latest first.
    servers: Vec<String>,
    /// The word the Spelling pane shows.
    correction: Option<canvas::interaction::Correction>,
    /// The strip's fill with the window focused and not, continuing the system's title bar.
    titlebar: [[f32; 4]; 2],
    /// A section or group being renamed in the sidebar.
    renaming: Option<rename::Renaming>,
    /// The notebook shown while it has no section open, as one whose last section was
    /// deleted.
    sectionless: Option<Arc<Library>>,
    /// A password-protected section shown locked, in place of an open one.
    locked: Option<protection::Locked>,
    /// The password dialog while it is open.
    password: Option<protection::Asking>,
    /// Options' Passwords.
    passwords: settings::Passwords,
    /// The theme new notebooks take, by id.
    notebook_theme: Option<String>,
    /// What the pointer is dragging: a page's tab, a section tab or a sidebar row.
    drag: Option<menus::Drag>,
    /// Pages whose template strip was dismissed this run.
    dismissed: HashSet<ExGuid>,
    /// A page just created, whose title takes the caret once it opens.
    title_focus: Option<ExGuid>,
    /// The page colour a menu previews over the open page's own this frame.
    page_color_preview: Option<Option<u32>>,
    media: recording::Media,
    /// Whether playback highlights the notes linked to the moment playing.
    see_playback: bool,
    thumbnails: templates::Thumbnails,
    search: search::Search,
    /// Whether the page list is shown beside the page.
    pages_open: bool,
    /// Full Page View: the sidebar, section tabs and page list hidden around the page.
    full_page: bool,
    /// What Format Painter picked up, which the next selection made on the page takes.
    painter: Option<onestore::document::Format>,
    /// Installed font families, for the font box; listing them is slow.
    fonts: Vec<String>,
    /// Fonts picked from the font box this session, latest first.
    recent_fonts: Vec<String>,
    /// What the toolbar's buttons apply from their menus' last picks.
    toolbar: settings::Toolbar,
    commands: Vec<Command>,
    /// The page changed during this frame, to be saved and announced after it.
    changed: bool,
    /// Only the view scrolled or zoomed during this frame, to be announced after it.
    moved: bool,
    /// The stored page changed elsewhere while marked text was pending; it is shown once
    /// the composition ends.
    stale: bool,
    /// Whether the page last heard it had keyboard focus.
    page_focused: bool,
    /// The pointer in logical pixels, for window drags from the strip.
    pointer: [f32; 2],
    /// When the strip was last pressed, to zoom on a double click.
    strip_press: Option<Instant>,
    /// The strip is held and the window moves once the pointer does.
    strip_held: bool,
    /// The zoom typed in the zoom's field while it is open.
    zoom_typed: Option<String>,
    /// Where a replay asked the next frame to be written.
    snapshot: Option<PathBuf>,
    /// A replay waiting for nothing to be on its way, where it wants the accessibility tree
    /// written then, and the mark sent since the window was last found busy.
    replay_settle: Option<(
        Option<PathBuf>,
        std::sync::mpsc::Sender<()>,
        Option<Instant>,
    )>,
    /// `SNOWBOUND_FRAMES`: a directory every frame drawn is also written to, named by
    /// milliseconds since the window opened.
    frames: Option<(PathBuf, Instant)>,
    initial: Vec<(onestore::ExGuid, TextDocument)>,
    initial_layouts: Vec<(onestore::ExGuid, onestore::document::Layout)>,
    initial_date: Option<u64>,
    occluded: bool,
    /// The least width last given the window.
    min_width: f32,
    ime_allowed: bool,
    clipboard: Clipboard,
    access_adapter: AccessAdapter,
    accessibility: accessibility::Accessibility,
    /// Assistive technology holds the page's tree, grafted into the interface's.
    page_grafted: bool,
}

/// The page's accessibility tree, which the interface's holds at the page's box.
const PAGE_TREE: accesskit::TreeId = accesskit::TreeId(accesskit::Uuid::from_u128(1));

fn strip() -> Id {
    Id::ROOT.child("strip")
}

/// The section tabs' row, whose empty space drags the window as the strip's does.
fn tab_row() -> Id {
    Id::ROOT.child("tab row")
}

/// The page's box, only ever built as the graft of the page's accessibility tree, so the
/// tree is sent while it is laid out.
fn page() -> Id {
    Id::ROOT.child("page")
}

/// The section's colour around the page and its tabs.
fn frame() -> Id {
    Id::ROOT.child("frame")
}

fn sections() -> Id {
    Id::ROOT.child("sections")
}

/// Why no backend could draw the window: each one tried, and how it failed.
#[derive(Debug)]
struct NoGraphics(Vec<(settings::Backend, String)>);

impl std::fmt::Display for NoGraphics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let tried: Vec<_> = (self.0.iter())
            .map(|(backend, error)| format!("{} ({error})", backend.label()))
            .collect();
        write!(f, "Nothing could draw the window: {}", tried.join("; "))
    }
}

impl Error for NoGraphics {}

/// Notes what drawing started or failed with, where the platform keeps such notes.
fn note(message: &str) {
    #[cfg(target_arch = "wasm32")]
    web_sys::console::info_1(&message.into());
    #[cfg(not(target_arch = "wasm32"))]
    eprintln!("{message}");
}

/// Starts the window's surface with `chosen`, or else with the platform's backends in turn,
/// skipping those in `unavailable`, which gains each that fails. Answers the backend that
/// started and its adapter's name.
async fn start_surface(
    window: &Arc<Window>,
    backdrop: bool,
    chosen: settings::Backend,
    unavailable: &mut Vec<(settings::Backend, String)>,
) -> Result<(surface::Surface, Renderer, (settings::Backend, String)), Box<dyn Error>> {
    let mut order = vec![chosen];
    order.extend(settings::Backend::PLATFORM.iter().filter(|backend| {
        **backend != chosen && !unavailable.iter().any(|(failed, _)| failed == *backend)
    }));
    for backend in order {
        if backend == settings::Backend::Default {
            continue;
        }
        match surface::Surface::new(window.clone(), backdrop, backend).await {
            Ok((surface, renderer, adapter)) => {
                note(&format!("Drawing with {} ({adapter})", backend.label()));
                if let Ok(mut renderer) = crash::RENDERER.lock() {
                    *renderer = format!("{} ({adapter})", backend.label());
                }
                unavailable.retain(|(failed, _)| *failed != backend);
                return Ok((surface, renderer, (backend, adapter)));
            }
            Err(error) => {
                note(&format!("{} didn't start: {error}", backend.label()));
                unavailable.retain(|(failed, _)| *failed != backend);
                unavailable.push((backend, error.to_string()));
            }
        }
    }
    Err(Box::new(NoGraphics(unavailable.clone())))
}

impl State {
    async fn new(
        event_loop: &ActiveEventLoop,
        proxy: EventLoopProxy<UserEvent>,
        input: Input,
        substitutes: &[PathBuf],
        visible: bool,
        settings::Launch {
            file: settings,
            saved: stored,
            renderer: forced,
            cache,
        }: settings::Launch,
    ) -> Result<Self, Box<dyn Error>> {
        let window = Arc::new(
            event_loop.create_window(
                platform::window_attributes()
                    .with_visible(false)
                    .with_title(match &input {
                        Input::Notes {
                            reference: Some(page),
                            ..
                        } => format!("{} · Reference and temporary notes", page.title),
                        Input::Page(page) => format!("{} · Temporary page", page.title),
                        Input::Notes {
                            reference: None, ..
                        } => "Untitled · Temporary page".into(),
                        Input::Section { title, .. } => title.clone(),
                        Input::Notebooks { .. } => "Snowbound".into(),
                    })
                    .with_inner_size(LogicalSize::new(1180.0, 760.0))
                    .with_min_inner_size(LogicalSize::new(MIN_SIZE[0], MIN_SIZE[1])),
            )?,
        );
        platform::install_title_bar(&window);
        platform::install_text_input(&window);
        // Beneath the surface's layer, which is added over it.
        let backdrop = visible && platform::install_backdrop(&window);
        commands::Keymap::from_saved(&stored.keys).install();
        #[cfg(feature = "live")]
        live::configure(
            stored
                .user_name
                .as_deref()
                .unwrap_or(&platform::user_name()),
            &stored.live,
        );
        platform::install_menu();
        let access_adapter =
            AccessAdapter::with_event_loop_proxy(event_loop, &window, proxy.clone());
        window.set_visible(visible);
        let chosen = forced.unwrap_or(stored.renderer);
        let mut unavailable = Vec::new();
        let (surface, renderer, drawing) =
            start_surface(&window, backdrop, chosen, &mut unavailable).await?;
        let size = window.inner_size();
        let mut engine = TextEngine::default();
        let fallbacks = platform::symbol_fonts();
        draw::fall_back_to(&mut engine.fonts.collection, &fallbacks);
        for path in substitutes {
            let target = engine.register_substitute(parley::fontique::Blob::new(Arc::new(
                notebook::fs::read(path)?,
            )))?;
            eprintln!("Using {} for {target}", path.display());
        }
        let layouts = Arc::new(Mutex::new(engine.clone()));
        let temporary = matches!(input, Input::Notes { .. } | Input::Page(_));
        let background = proxy.clone();
        library::on_background(move || {
            let _ = background.send_event(UserEvent::Sync);
        });
        let account = proxy.clone();
        icloud::on_account_change(move || {
            let _ = account.send_event(UserEvent::ICloudAccount);
        });
        icloud::look_up(icloud_listed(proxy.clone()));
        let mut notebooks = Vec::new();
        let mut session = None;
        let mut sectionless = None;
        let mut locked = None;
        let (editor, scene) = match input {
            Input::Notes {
                document,
                width,
                reference,
            } => {
                let editor = CanvasEditor::new(&mut engine, document, width)?;
                let scene = reference
                    .map(|page| {
                        PageScene::new(page, &mut engine).map(|scene| (scene, [width + 24.0, 0.0]))
                    })
                    .transpose()?;
                (editor, scene)
            }
            Input::Page(page) => {
                let (scene, editor) = PageScene::from_page(page, &mut engine)?;
                (editor, Some((scene, [0.0; 2])))
            }
            Input::Section { file, title } => {
                let library = Arc::new(Library::section(&file, &cache));
                let section = library.open(&library.location, notify(proxy.clone()))?;
                let space = section
                    .pages()?
                    .into_iter()
                    .find(|(_, candidate, _)| *candidate == title)
                    .ok_or_else(|| format!("No page titled {title:?} in {}", file.display()))?
                    .0;
                notebooks.push(Arc::clone(&library));
                let path = library.location.clone();
                let (opened, page) = read_session(section, library, path, Some(space))?;
                let (scene, editor) = PageScene::from_page(page, &mut engine)?;
                session = Some(opened);
                (editor, Some((scene, [0.0; 2])))
            }
            Input::Notebooks { locations, current } => {
                notebooks = locations
                    .iter()
                    .map(|location| Arc::new(Library::notebook(location, &cache)))
                    .collect();
                for library in &notebooks {
                    if let Err(error) = &library.notebook {
                        eprintln!("Cannot open the notebook at {}: {error}", library.location);
                    }
                    // Drawn offscreen, the app leaves the notebooks as they are.
                    if visible {
                        library.purge_recycle_bin();
                    }
                }
                // Where the notebook was left, or its first section.
                let shown = notebooks
                    .iter()
                    .filter(|library| Some(&library.location) == current.as_ref())
                    .chain(&notebooks)
                    .find_map(|library| {
                        let left = stored.recent.iter().find(|place| {
                            place.notebook == library.location && library.contains(&place.section)
                        });
                        match left {
                            Some(place) => Some((library, place.section.clone(), Some(place.page))),
                            None => Some((library, library.first_section()?, None)),
                        }
                    });
                match shown {
                    Some((library, path, left)) if !library.locked(&path) => {
                        let section = library.open(&path, notify(proxy.clone()))?;
                        let (opened, page) =
                            read_session(section, Arc::clone(library), path, left)?;
                        let (scene, editor) = PageScene::from_page(page, &mut engine)?;
                        session = Some(opened);
                        (editor, Some((scene, [0.0; 2])))
                    }
                    shown => {
                        // A first section still locked shows its locked page.
                        locked = shown.map(|(library, path, _)| protection::Locked {
                            library: Arc::clone(library),
                            path,
                        });
                        sectionless = notebooks
                            .iter()
                            .find(|library| Some(&library.location) == current.as_ref())
                            .or(notebooks.first())
                            .filter(|_| locked.is_none())
                            .cloned();
                        let blank = TextDocument::new(vec![Paragraph::new(
                            String::new(),
                            Format::default(),
                        )])?;
                        (CanvasEditor::new(&mut engine, blank, 480.0)?, None)
                    }
                }
            }
        };
        let initial_date = editor.date().map(|date| date.timestamp());
        let initial_layouts = editor
            .object_layouts()
            .map(|(id, layout)| (id, layout.clone()))
            .collect();
        let initial = editor
            .outlines()
            .iter()
            .map(|outline| (outline.id, outline.document().clone()))
            .collect();
        let dpr = window.scale_factor() as f32;
        window.set_ime_allowed(true);
        window.request_redraw();
        eprintln!("Scale factor {dpr}");
        window.set_theme(stored.color_scheme.theme());
        let appearance = stored
            .color_scheme
            .theme()
            .unwrap_or_else(|| platform::appearance(&window));
        platform::follow_appearance(&window, appearance);
        let mut ui = Ui::new(
            theme(appearance, stored.light_pages, surface.translucent()),
            platform::double_click_interval(),
        );
        let titlebar = platform::titlebar(appearance)
            .filter(|_| !surface.translucent())
            .unwrap_or([ui.theme.strip; 2]);
        // A window shown but never focused hears no focus event; a hidden one draws as focused.
        ui.window_focused = !visible || window.has_focus();
        ui.fall_back_to(&fallbacks);
        ui.set_focus(Some(page()));
        for family in FONTS {
            for (face, _) in engine.substitute(family).map_or(&[][..], |s| &s.faces) {
                ui.preview_font(face.clone(), family);
            }
        }
        platform::system_interface(&mut ui);
        let mut fonts: Vec<String> = engine
            .fonts
            .collection
            .family_names()
            // Dot-named families are the system's private ones, hidden from font menus.
            .filter(|name| !name.starts_with('.'))
            .map(String::from)
            .collect();
        fonts.sort_unstable_by_key(|name| name.to_lowercase());
        fonts.dedup();
        // A replay's or hidden window's copies leave the user's clipboard alone.
        let clipboard = if visible && std::env::var_os("SNOWBOUND_REPLAY").is_none() {
            Clipboard::System(platform::Clipboard::new(&window)?)
        } else {
            Clipboard::Memory(String::new(), None)
        };
        let redraw: std::task::Waker = Arc::new(Redraw(proxy.clone())).into();
        let search = search::Search::new(stored.search_scope, redraw.clone());
        let spelling = spell::dictionary()
            .map(|dictionary| canvas::spelling::Spelling::new(dictionary, redraw.clone()));
        let updates = update::Updates::start(visible && !stored.manual_updates, proxy.clone());
        let prefetch = prefetch::Prefetch::new(Arc::clone(&layouts), redraw.clone());
        let reads = unread::Reads::load(&cache);
        let mut state = Self {
            author: stored.user_name.unwrap_or_else(platform::user_name),
            window,
            redraw,
            proxy,
            renderer,
            surface,
            backdrop,
            renderer_choice: stored.renderer,
            drawing,
            unavailable,
            ui,
            view: PageView::new(
                editor,
                engine,
                scene,
                [size.width, size.height],
                dpr,
                platform::double_click_interval(),
            ),
            notebooks,
            session,
            temporary,
            settings,
            cache,
            sidebar: stored.sidebar,
            folded: HashSet::new(),
            dismissed: HashSet::new(),
            title_focus: None,
            menu: None,
            sync_notebook: None,
            actions: None,
            after_open: None,
            link: None,
            properties: None,
            symbols: None,
            save_as: None,
            unpacking: None,
            #[cfg(target_os = "linux")]
            asking: Default::default(),
            reads,
            #[cfg(feature = "live")]
            peers: Default::default(),
            server: None,
            icloud_reading: HashSet::new(),
            folder_rename: None,
            trashing: HashSet::new(),
            vanishing: HashSet::new(),
            tags: stored
                .tags
                .unwrap_or_else(canvas::editor::NoteTag::defaults),
            tag_list: None,
            text_menu: None,
            options: None,
            themes: None,
            printing: print::Printing::default(),
            updates,
            color_scheme: stored.color_scheme,
            light_pages: stored.light_pages,
            titlebar,
            renaming: None,
            sectionless,
            locked,
            password: None,
            passwords: stored.passwords,
            notebook_theme: (stored.notebook_theme.0)
                .map(|id| notebook::sidecar::themes::successor(&id).to_owned()),
            live_options: stored.live,
            drag: None,
            page_color_preview: None,
            media: Default::default(),
            see_playback: true,
            thumbnails: templates::Thumbnails::default(),
            search,
            pages_open: true,
            full_page: false,
            painter: None,
            fonts,
            recent_fonts: stored.recent_fonts,
            toolbar: stored.toolbar,
            commands: Vec::new(),
            changed: false,
            moved: false,
            stale: false,
            page_focused: true,
            pointer: [0.0; 2],
            strip_press: None,
            strip_held: false,
            zoom_typed: None,
            snapshot: None,
            replay_settle: None,
            frames: std::env::var_os("SNOWBOUND_FRAMES").map(|dir| (dir.into(), Instant::now())),
            initial,
            initial_date,
            initial_layouts,
            occluded: false,
            min_width: MIN_SIZE[0],
            ime_allowed: true,
            clipboard,
            loads: mpsc::channel(),
            prefetch,
            layouts,
            loading: 0,
            opening: None,
            switching: None,
            places: HashMap::new(),
            last_pages: (stored.recent.iter().rev())
                .map(|place| (library::key(&place.notebook, &place.section), place.page))
                .collect(),
            trail: {
                let mut trail = navigation::Trail::default();
                trail.recent = stored.recent;
                trail
            },
            undo: undo::Timeline::default(),
            swipe: navigation::Swipe::default(),
            access_adapter,
            accessibility: accessibility::Accessibility::default(),
            page_grafted: false,
            spelling,
            hide_spelling: stored.hide_spelling,
            pen_pressure: !stored.ignore_pen_pressure,
            page_tabs_left: stored.page_tabs_left,
            navigation_bar_right: stored.navigation_bar_right,
            servers: stored.servers,
            correction: None,
        };
        state.view.snap_to_grid = !stored.ignore_grid;
        state.view.editor.default_font = stored.default_font;
        state.view.editor.markdown = commands::markdown(!stored.ignore_markdown);
        // A notebook opened from its server that couldn't sign in asks to, as the Finder does.
        let unsigned = state.notebooks.iter().find(|library| {
            library.notebook.is_err() && library::server_address(&library.location).is_some()
        });
        if let Some(location) = unsigned.map(|library| library.location.clone()) {
            state.commands.push(Command::OpenFromServer(Some(location)));
        }
        state.visited();
        state.prefetch_around();
        state.show_spelling();
        state.title();
        platform::update_tag_menu(&state.tags);
        state.tell_fallback(chosen);
        Ok(state)
    }

    /// Says so where `chosen` didn't start and another backend draws instead.
    fn tell_fallback(&self, chosen: settings::Backend) {
        if chosen != settings::Backend::Default && self.drawing.0 != chosen {
            platform::alert(
                &format!("Couldn't start {}", chosen.label()),
                &format!(
                    "Snowbound is drawing with {} instead.",
                    self.drawing.0.label()
                ),
            );
        }
    }

    /// Draws with `chosen` from now on, in place of what draws now, the window's state
    /// staying as it is. The window takes one swap chain or context at a time, so the old
    /// renderer and surface go first; where nothing starts, the window can't go on.
    #[cfg(not(target_arch = "wasm32"))]
    fn switch_renderer(mut self, chosen: settings::Backend) -> Result<Self, Box<dyn Error>> {
        drop(self.renderer);
        drop(self.surface);
        let (surface, renderer, drawing) = pollster::block_on(start_surface(
            &self.window,
            self.backdrop,
            chosen,
            &mut self.unavailable,
        ))?;
        self.surface = surface;
        self.renderer = renderer;
        self.drawing = drawing;
        self.started_surface(chosen);
        Ok(self)
    }

    /// Switches to the renderer the settings file names, where it was edited to name another
    /// while the window was away.
    fn follow_renderer_setting(&mut self) {
        let Some(path) = &self.settings else {
            return;
        };
        let chosen = settings::Settings::load(path).renderer;
        if chosen != self.renderer_choice {
            self.renderer_choice = chosen;
            let _ = self.proxy.send_event(UserEvent::Renderer(chosen));
        }
    }

    /// Fits a surface just started to the window, and says where it isn't what was chosen.
    fn started_surface(&mut self, chosen: settings::Backend) {
        let ratio = self.scale() / self.window.scale_factor() as f32;
        let size = self.window.inner_size();
        self.surface.size = [size.width, size.height].map(|side| (side as f32 * ratio) as u32);
        self.surface.configure(&self.renderer);
        // Its colours follow whether the system's material shows through.
        self.follow_color_scheme();
        self.tell_fallback(chosen);
        self.window.request_redraw();
    }

    /// Builds, lays out and paints one frame, then does what it asked for and asks for the
    /// frame that shows the result.
    fn frame(&mut self) -> Result<(), Box<dyn Error>> {
        let start = Instant::now();
        if let Err(error) = self.open_loaded() {
            eprintln!("{error}");
        }
        if let Err(error) = self.take_waiting() {
            eprintln!("{error}");
        }
        lap("open", start);
        self.follow_reading();
        let size = self.window.inner_size();
        let scale = self.scale();
        if scale != self.ui.scale() {
            self.renderer.clear_glyph_cache();
            let response = self.view.scale_factor_changed(scale)?;
            self.respond(response);
        }
        #[cfg(feature = "live")]
        self.follow_peers();
        self.layout(
            [size.width, size.height].map(|side| side as f32 / self.window.scale_factor() as f32),
            scale,
        )?;
        self.update_accessibility(false)?;
        lap("build", start);
        self.window.set_cursor(
            platform::resize_direction(&self.window, self.pointer)
                .map(CursorIcon::from)
                .or_else(|| self.ui.cursor())
                .unwrap_or_else(|| cursor_icon(self.view.cursor())),
        );
        let ime = if self.ui.focused() == Some(page()) {
            self.view.accepts_text()
        } else {
            search::takes_text(self.ui.focused())
        };
        if self.ime_allowed != ime {
            self.ime_allowed = ime;
            self.window.set_ime_allowed(ime);
        }
        self.lock_idle()?;
        self.draw()?;
        lap("drawn", start);
        self.sync_index(false, Vec::new());
        let commands = std::mem::take(&mut self.commands);
        let follow = !commands.is_empty()
            || self.ui.wants_frame()
            || self.opening.is_some()
            || self.switching.is_some();
        for command in commands {
            self.apply(command)?;
        }
        if std::mem::take(&mut self.changed) {
            self.after_edit()?;
        } else if self.moved {
            self.after_move()?;
        }
        self.moved = false;
        if self.stale {
            self.refresh()?;
        }
        // A covered window shows nothing, so animations wait for it to be uncovered.
        if follow && !self.occluded {
            self.window.request_redraw();
        }
        lap("frame", start);
        Ok(())
    }

    /// Lays out the interface in a window `size` logical pixels big and fits the page to
    /// its box.
    fn layout(&mut self, size: [f32; 2], scale: f32) -> Result<(), Box<dyn Error>> {
        self.ui.begin(size, scale, Instant::now());
        platform::cover_border_line(&mut self.ui, size[0]);
        let previewed = self.page_color_preview;
        let (section, open_tab, open_page) = self.build()?;
        // The page's frame and tab were built with last frame's preview.
        if self.page_color_preview != previewed {
            self.ui.wake_after(std::time::Duration::ZERO);
        }
        self.options_dialog();
        self.themes_dialog();
        self.print_dialog();
        self.server_dialog();
        #[cfg(feature = "live")]
        self.live_dialogs();
        self.link_dialog()?;
        self.properties_dialog();
        self.symbols_dialog();
        self.save_as_dialog();
        self.unpack_dialog();
        self.password_dialog()?;
        #[cfg(target_os = "linux")]
        self.own_dialog();
        self.customize_tags();
        self.palette();
        self.sync_popup()?;
        self.text_menu()?;
        platform::resize_grip(&mut self.ui, &self.window, size);
        self.ui.end();
        self.edges(section, open_tab, open_page);
        if platform::cuts_corners() {
            let [width, height] = size;
            let radius = platform::corner_radius(&self.window);
            // Where the toolbar's row is the title bar, its corners are the window's too.
            let top = if platform::system_titlebar(&self.window) {
                0.0
            } else {
                radius
            };
            self.ui.round_corners(
                &[
                    ([0.0, 0.0], top),
                    ([width, 0.0], top),
                    ([width, height], radius),
                    ([0.0, height], radius),
                ],
                [0.0; 4],
                |_| [0.0; 4],
            );
        }
        if let Some(rect) = self.ui.laid_out(page()) {
            let size = [
                ((rect[2] - rect[0]) * scale).round() as u32,
                ((rect[3] - rect[1]) * scale).round() as u32,
            ];
            if size != self.view.viewport.size {
                let response = self.view.resized(size)?;
                self.respond(response);
            }
        }
        Ok(())
    }

    /// Takes `appearance`'s colours.
    fn set_appearance(&mut self, appearance: winit::window::Theme) {
        platform::follow_appearance(&self.window, appearance);
        self.ui.theme = theme(appearance, self.light_pages, self.surface.translucent());
        self.titlebar = platform::titlebar(appearance)
            .filter(|_| !self.surface.translucent())
            .unwrap_or([self.ui.theme.strip; 2]);
    }

    /// Takes the colour scheme chosen, or the system's where it follows the system.
    fn follow_color_scheme(&mut self) {
        self.window.set_theme(self.color_scheme.theme());
        let appearance = self
            .color_scheme
            .theme()
            .unwrap_or_else(|| platform::appearance(&self.window));
        self.set_appearance(appearance);
    }

    /// Builds the frame's boxes and returns the section's colours as they ease, the open
    /// section tab and the open page's tab, for the edges drawn once they are laid out.
    fn build(&mut self) -> Result<(ui::Section, Id, Option<Id>), Box<dyn Error>> {
        let target = self.ui.theme.section(section_color(
            self.session
                .as_ref()
                .zip(self.open_tab())
                .and_then(|(session, tab)| session.tabs[tab].color),
        ));
        self.ui.icon_palette = self.ui.theme.icon_palette(
            target.accent,
            self.toolbar.highlight.map_or([1.0; 4], colorref),
        );
        let strip = self.titlebar[usize::from(!self.ui.window_focused)];
        let mut ease = |part: &str, color: [f32; 4]| -> [f32; 4] {
            std::array::from_fn(|channel| {
                self.ui
                    .animate(Id::ROOT.child((part, channel)), color[channel])
            })
        };
        let section = ui::Section {
            frame: [
                ease("top", target.frame[0]),
                ease("bottom", target.frame[1]),
            ],
            tab: ease("tab", target.tab),
            edge: ease("edge", target.edge),
            accent: ease("accent", target.accent),
        };
        self.ui.theme.strip = ease("strip", strip);
        self.ui.theme.accent = section.accent;
        // Elsewhere the theme holds the platform's colours.
        #[cfg(target_os = "macos")]
        {
            [
                self.ui.theme.caret,
                self.ui.theme.selection,
                self.ui.theme.inactive_selection,
            ] = platform::text_colors(&self.window);
        }
        // The page, and the open page's tab joined to it, take the page's colour.
        let mut theme = self.ui.theme.clone();
        theme.paper = self.paper().color;
        self.page_color_preview = None;
        platform::update_menu(|| self.statuses());
        let welcome = self.session.is_none()
            && !self.temporary
            && self.sectionless.is_none()
            && self.locked.is_none();
        self.toolbar(&theme, !welcome)?;
        if welcome {
            self.welcome();
            return Ok((section, Id::ROOT, None));
        }
        self.ui.open(
            "body",
            Spec {
                size: [fill(), fill()],
                ..Spec::default()
            },
        );
        let sidebar = self.sidebar_width();
        if !self.navigation_bar_right {
            self.sidebar(&theme, sidebar);
        }
        // Where the buttons float over the tab row while the sidebar is shut.
        let beside = 1.0 - sidebar / sidebar::WIDTH;
        self.ui.open(
            "main",
            Spec {
                axis: Axis::Y,
                size: [fill(), fill()],
                ..Spec::default()
            },
        );
        let height = self
            .ui
            .animate(tab_row(), if self.full_page { 0.0 } else { TAB_ROW });
        let drags = self.chrome_drags();
        self.ui.open_as(
            tab_row(),
            Spec {
                flags: if drags {
                    Flags::CLIP | Flags::CLICKABLE
                } else {
                    Flags::CLIP
                },
                size: [fill(), px(height)],
                fill: Some(theme.strip),
                pad: [FRAME, 0.0],
                ..Spec::default()
            },
        );
        if !self.temporary {
            // Past the row's padding, the frame's corner and the first tab's shadow, the tabs'
            // outlines start where Back and Forward end, after the notebook button's square on
            // the left; that room shrinks on the sidebar's easing, as they move into its
            // header, so the tabs ease with it.
            let room = sidebar::NAV + TAB_ROW - FRAME - self.rounding() - ui::SHADOW[0];
            let room = if self.navigation_bar_right {
                room - TAB_ROW
            } else {
                room * beside
            };
            self.ui.leaf(
                "rail",
                Spec {
                    size: [px(room), px(1.0)],
                    ..Spec::default()
                },
            );
        }
        self.ui.leaf(
            "corner",
            Spec {
                size: [px(self.rounding()), px(1.0)],
                ..Spec::default()
            },
        );
        self.recycle_heading(&theme);
        let row = sections();
        let unread = self.unread_keys();
        let (clicked, open_tab) = match &self.session {
            Some(session) => {
                let lit = self.page_drop(row);
                let dragged = self.dragged_tab(row);
                let shown = self.open_tab().unwrap_or(session.tab);
                // A tab being renamed takes the name typed, which its field covers.
                let renaming = self.renaming.as_mut().and_then(|renaming| {
                    let tab = (session.tabs.iter())
                        .position(|tab| renaming.entry(&session.library, &tab.path, true))?;
                    Some((tab, &mut renaming.name))
                });
                let typed = renaming
                    .as_ref()
                    .map(|(tab, name)| (*tab, name.to_string()));
                let tabs: Vec<_> = session
                    .tabs
                    .iter()
                    .enumerate()
                    .map(|(index, tab)| {
                        let name = match &typed {
                            Some((renamed, name)) if *renamed == index => name.as_str(),
                            _ => tab.name.as_str(),
                        };
                        let unread = unread.contains(&session.library.key(&tab.path));
                        (name, section_color(tab.color), unread)
                    })
                    .collect();
                let mut kept = None;
                let mut field;
                let renaming = match renaming {
                    Some((tab, name)) => {
                        field =
                            |ui: &mut Ui, tall| kept = rename::tab_field(ui, &theme, name, tall);
                        Some((tab, &mut field as &mut dyn FnMut(&mut Ui, f32)))
                    }
                    None => None,
                };
                let ui::shell::Tabs {
                    clicked,
                    context,
                    held,
                    renamed,
                    slot,
                    settled,
                    open: open_tab,
                } = ui::shell::section_tabs(
                    &mut self.ui,
                    row,
                    &tabs,
                    shown,
                    lit,
                    dragged,
                    renaming,
                    &section,
                    TAB_ROW,
                    theme.strip,
                );
                if let Some(keep) = kept {
                    self.finish_renaming(keep);
                }
                name(&mut self.ui, row, "Sections");
                self.drag_tabs(held, slot, settled, row);
                let Some(session) = &self.session else {
                    unreachable!("The tabs are the session's")
                };
                let open = |tab: usize| {
                    Command::OpenSection(
                        Arc::clone(&session.library),
                        session.tabs[tab].path.clone(),
                    )
                };
                if let Some((tab, point)) = context {
                    self.menu = Some((
                        menus::Target::Section {
                            library: Arc::clone(&session.library),
                            path: session.tabs[tab].path.clone(),
                        },
                        point,
                    ));
                    self.ui.open_popup(menus::id());
                }
                // Letting go of a dragged tab isn't a click on it.
                let clicked = clicked.filter(|_| !self.dragged()).map(open);
                if let Some(tab) = renamed {
                    let target = rename::Target::Entry {
                        library: Arc::clone(&session.library),
                        path: session.tabs[tab].path.clone(),
                        in_tab: true,
                    };
                    self.rename(target);
                }
                (clicked, open_tab)
            }
            None if self.locked.is_some() => self.locked_tabs(row, &section, theme.strip),
            None => {
                let tabs = [("Temporary page", section_color(None), false)];
                let shown = if self.temporary { &tabs[..] } else { &[] };
                let open_tab = ui::shell::section_tabs(
                    &mut self.ui,
                    row,
                    shown,
                    0,
                    None,
                    None,
                    None,
                    &section,
                    TAB_ROW,
                    theme.strip,
                )
                .open;
                (None, open_tab)
            }
        };
        self.commands.extend(clicked);
        if let Some(count) = self.session.as_ref().map(|session| session.tabs.len())
            && let Some(tab) =
                (0..count).find(|tab| self.ui.signal(ui::shell::tab_id(row, *tab)).hovered)
        {
            self.prefetch_section(tab);
        }
        self.ui.leaf(
            "space",
            Spec {
                size: [fill(), px(1.0)],
                ..Spec::default()
            },
        );
        #[cfg(feature = "live")]
        self.avatars();
        if self.session.is_some() {
            self.zoom(&theme)?;
            self.page_tools(&theme);
        }
        let button = if self.navigation_bar_right && !self.temporary {
            TAB_ROW * beside
        } else {
            0.0
        };
        self.ui.leaf(
            "trail",
            Spec {
                size: [px(self.trailing() - FRAME + button), px(1.0)],
                ..Spec::default()
            },
        );
        self.ui.close();

        self.ui.open_as(
            frame(),
            Spec {
                size: [fill(), fill()],
                fill: Some(section.frame[0]),
                gradient: Some(section.frame[1]),
                pad: [FRAME, FRAME],
                ..Spec::default()
            },
        );
        // OneNote's "Page tabs appear on the left" lists them before the page.
        let mut open_page = None;
        if self.page_tabs_left {
            open_page = self.page_list(&theme, &section, row);
        }
        self.ui.open(
            "column",
            Spec {
                axis: Axis::Y,
                size: [fill(), fill()],
                ..Spec::default()
            },
        );
        if let Some(session) = &self.session
            && let Some(bar) = session.bar()
        {
            let changes = self.changes().len();
            let sections: Vec<&str> = session.tabs.iter().map(|tab| tab.name.as_str()).collect();
            let steps = [
                session.change.is_some_and(|at| at > 0),
                session.change.map_or(changes > 0, |at| at + 1 < changes),
            ];
            self.commands
                .extend(conflict_bar(&mut self.ui, bar, &sections, steps));
        }
        if self.in_recycle_bin() {
            recycle::bar(&mut self.ui);
        }
        if self.session.is_none() && self.locked.is_some() {
            self.locked_page();
            self.ui.close();
            self.ui.close();
            self.ui.close();
            if self.navigation_bar_right {
                self.sidebar(&theme, sidebar);
            }
            self.sidebar_button(height, sidebar);
            self.ui.close();
            self.context_menu();
            return Ok((section, open_tab, None));
        }
        if let Some(library) = self.sectionless.clone().filter(|_| self.session.is_none()) {
            self.no_sections(library);
            self.ui.close();
            self.ui.close();
            self.ui.close();
            if self.navigation_bar_right {
                self.sidebar(&theme, sidebar);
            }
            self.sidebar_button(height, sidebar);
            self.ui.close();
            self.context_menu();
            return Ok((section, open_tab, None));
        }
        self.ui.open_as(
            page(),
            Spec {
                flags: Flags::CUSTOM | Flags::FOCUSABLE,
                size: [fill(), fill()],
                fill: Some(theme.paper),
                role: Some(accesskit::Role::GenericContainer),
                ..Spec::default()
            },
        );
        if let Some(node) = self.ui.access(page()) {
            node.set_tree_id(PAGE_TREE);
        }
        // A page opening has neither the templates nor the scroll of the page leaving.
        let opening = self.loading().is_some();
        if !opening {
            self.template_strip(&theme);
        }
        self.transport(&theme)?;
        let scroll = self.view.scroll();
        for (index, axis) in [Axis::X, Axis::Y]
            .into_iter()
            .filter(|_| !opening)
            .enumerate()
        {
            if let Some(offset) = ui::scrollbar(
                &mut self.ui,
                index,
                axis,
                -self.view.viewport.origin[index],
                [scroll.min[index], scroll.max[index]],
                self.view.viewport.size[index] as f32,
                scroll.max[1 - index] > scroll.min[1 - index],
                [0.18, 0.18, 0.18, 0.45],
            ) {
                let response = self.view.scroll_to(index, offset)?;
                self.respond(response);
            }
        }
        #[cfg(feature = "live")]
        if !opening {
            self.peer_carets();
        }
        let task = self.view.task_under_pointer().map(|[x0, y0, x1, y1]| {
            let scale = self.ui.scale();
            self.ui.leaf(
                "task",
                Spec {
                    flags: Flags::FLOAT,
                    size: [px((x1 - x0) / scale), px((y1 - y0) / scale)],
                    position: [x0 / scale, y0 / scale],
                    ..Spec::default()
                },
            );
            page().child("task")
        });
        self.ui.close();
        if let Some(task) = task {
            ui::popup::tooltip_over(
                &mut self.ui,
                task,
                "Outlook task",
                Some("Edit it in OneNote with Outlook"),
            );
        }
        let signal = self.ui.signal(page());
        let focused = self.ui.window_focused && self.ui.focused() == Some(page());
        if focused != self.page_focused {
            self.page_focused = focused;
            let response = self.view.focus_changed(focused)?;
            self.respond(response);
        }
        self.page_events(signal.events)?;
        self.ui.close();
        if !self.page_tabs_left {
            open_page = self.page_list(&theme, &section, row);
        }
        self.ui.close();
        self.ui.close();
        if self.navigation_bar_right {
            self.sidebar(&theme, sidebar);
        }
        self.task_pane(&theme);
        self.sidebar_button(height, sidebar);
        self.ui.close();
        self.context_menu();
        // No tab stands on the frame in Full Page View.
        let open_tab = if self.full_page {
            frame().child("no tab")
        } else {
            open_tab
        };
        Ok((section, open_tab, open_page))
    }

    /// The page's corner radius, sharing its centres with the window's corners.
    fn rounding(&self) -> f32 {
        (platform::corner_radius(&self.window) - FRAME).max(0.0)
    }

    /// Borders the open section tab and the frame's top, and rounds and borders the page
    /// together with the open page's tab, joined where they meet.
    fn edges(&mut self, section: ui::Section, open_tab: Id, open_page: Option<Id>) {
        let panel = self.ui.laid_out(frame().child("panel"));
        let (Some(frame), Some(page)) = (self.ui.laid_out(frame()), self.ui.laid_out(page()))
        else {
            return;
        };
        let rounding = self.rounding();
        // Where the open page tab meets the page, concentric with its neighbours.
        let join = PILL_MARGIN + rounding;
        let [left, top, right, bottom] = page;
        let left_tabs = self.page_tabs_left;
        let tab = open_page
            .and_then(|id| self.ui.laid_out(id))
            .map(|row| [row[0], row[1], row[2], row[3] - ROW_GAP])
            .zip(panel)
            .and_then(|(row, panel)| {
                let [start, end] = [
                    row[1].max(panel[1]).max(top),
                    row[3].min(panel[3]).min(bottom),
                ];
                // How far the tab reaches out from the page's side.
                let (reach, out) = match left_tabs {
                    true => (row[0].max(panel[0]), left - row[0].max(panel[0])),
                    false => (row[2].min(panel[2]), row[2].min(panel[2]) - right),
                };
                (out > 2.0 * join && end - start > 2.0 * rounding).then(|| {
                    // A tab too near the page's corner joins it along the edge.
                    let start = if start < top + rounding + join {
                        top
                    } else {
                        start
                    };
                    let end = if end > bottom - rounding - join {
                        bottom
                    } else {
                        end
                    };
                    [start, end, reach]
                })
            });
        // Clockwise from the top left, the open tab's bump on the side it stands.
        let mut outline = Vec::new();
        match tab {
            Some([start, end, reach]) if left_tabs => {
                if start > top {
                    outline.push(([left, top], rounding));
                }
                outline.extend([([right, top], rounding), ([right, bottom], rounding)]);
                if end < bottom {
                    outline.extend([([left, bottom], rounding), ([left, end], join)]);
                }
                outline.extend([([reach, end], rounding), ([reach, start], rounding)]);
                if start > top {
                    outline.push(([left, start], join));
                }
            }
            Some([start, end, reach]) => {
                outline.push(([left, top], rounding));
                if start > top {
                    outline.extend([([right, top], rounding), ([right, start], join)]);
                }
                outline.extend([([reach, start], rounding), ([reach, end], rounding)]);
                if end < bottom {
                    outline.extend([([right, end], join), ([right, bottom], rounding)]);
                }
                outline.push(([left, bottom], rounding));
            }
            None => outline.extend([
                ([left, top], rounding),
                ([right, top], rounding),
                ([right, bottom], rounding),
                ([left, bottom], rounding),
            ]),
        }
        let height = (frame[3] - frame[1]).max(1.0);
        let paper = self.paper().color;
        self.ui.round_corners(&outline, paper, |y| {
            ui::mix(section.frame[0], section.frame[1], (y - frame[1]) / height)
        });
        self.ui.border(&outline, true, section.edge);
        // The frame's top corners share the page's centres, and beside the sidebar so does
        // its bottom one, rounding in as the sidebar opens; the frame's top edge breaks where
        // the open tab stands on it.
        let [start, end] = [frame[0], frame[2]];
        let outer = platform::corner_radius(&self.window);
        let window = self.ui.size()[0];
        let [beside, beside_right] =
            [start, window - end].map(|gap| (outer * gap / sidebar::WIDTH).min(outer));
        let strip = self.ui.theme.strip;
        // The border runs half its width inside the frame, on whole device pixels at any
        // scale, its corners concentric with the fill's. The fill is cut at the border's
        // outer edge, then again at its middle, so none shows through its antialiased rim.
        let [west, north, east] = [start + 0.5, frame[1] + 0.5, end - 0.5];
        let inner = |radius: f32| (radius - 0.5).max(0.0);
        for [left, top, right, cut] in [[start, frame[1], end, 0.0], [west, north, east, 0.5]] {
            self.ui.round_corners(
                &[
                    ([left, top], (outer - cut).max(0.0)),
                    ([right, top], (outer - cut).max(0.0)),
                    ([right, frame[3]], (beside_right - cut).max(0.0)),
                    ([left, frame[3]], (beside - cut).max(0.0)),
                ],
                paper,
                |_| strip,
            );
        }
        // The border breaks where the open tab shows, which the row may scroll it out of.
        let [left, right] = self
            .ui
            .laid_out(sections())
            .map_or([end; 2], |row| [row[0], row[2]]);
        let [foot, toe] = self
            .ui
            .laid_out(open_tab)
            .map_or([end; 2], |tab| ui::shell::tab_base(tab, TAB_ROW))
            .map(|x| x.clamp(left, right));
        // The border runs down both sides to the window's bottom, round the bottom corner
        // beside the sidebar, on whichever side it stands.
        self.ui.border(
            &[
                ([west + beside, frame[3]], 0.0),
                ([west, frame[3]], inner(beside)),
                ([west, north], inner(outer)),
                ([foot, north], 0.0),
            ],
            false,
            section.edge,
        );
        self.ui.border(
            &[
                ([toe, north], 0.0),
                ([east, north], inner(outer)),
                ([east, frame[3]], inner(beside_right)),
                ([east - beside_right, frame[3]], 0.0),
            ],
            false,
            section.edge,
        );
    }

    /// The toolbar and tab row's margin at the window's trailing side, which matches the
    /// leading gap where the row is the title bar.
    fn trailing(&self) -> f32 {
        if platform::system_titlebar(&self.window) {
            FRAME
        } else {
            platform::TRAILING
        }
    }

    /// Whether the chrome's empty space drags the window: where it is the title bar, or
    /// over the title's gradient.
    fn chrome_drags(&self) -> bool {
        !platform::system_titlebar(&self.window) || self.surface.translucent()
    }

    /// The toolbar: one row of groups that fold as the window narrows. Where the platform
    /// lets it, the row is the window's title bar, and on the welcome page, without `tools`,
    /// that is all it is.
    fn toolbar(&mut self, theme: &Theme, tools: bool) -> Result<(), Box<dyn Error>> {
        let title = !platform::system_titlebar(&self.window);
        if !tools && !title {
            return Ok(());
        }
        let strip_row = self.chrome_drags();
        let spec = Spec {
            flags: if strip_row {
                Flags::CLICKABLE
            } else {
                Flags::default()
            },
            size: [fill(), px(TOOLBAR)],
            pad: [0.0, (TITLE - ui::shell::TOOL) / 2.0],
            gap: GAP,
            role: Some(accesskit::Role::Toolbar),
            ..Spec::default()
        };
        let row = if strip_row {
            self.ui.open_as(strip(), spec)
        } else {
            self.ui.open("toolbar", spec)
        };
        // The strip stops short of the window's controls, where Windows 7 draws its caption
        // buttons beneath the window's pixels.
        let edge = (TITLE - ui::shell::TOOL) / 2.0;
        self.ui.open(
            "bar",
            Spec {
                size: [fill(), px(TOOLBAR)],
                fill: Some(theme.strip),
                pad: [0.0, edge],
                offset: [0.0, -edge],
                gap: GAP,
                ..Spec::default()
            },
        );
        let lead = if title { platform::LEADING } else { FRAME };
        self.ui.leaf(
            "lead",
            Spec {
                size: [px(lead - GAP), px(1.0)],
                ..Spec::default()
            },
        );
        if title {
            platform::window_controls(&mut self.ui, &self.window, false);
        }
        if tools {
            // The groups fold in the room the window's controls leave, and whatever still
            // overflows is cut there, so the controls always show.
            let panel = theme.tool[3] > 0.0 && theme.tool_panel;
            let face = tool_face(theme, panel, FACE_RISE - edge);
            self.ui.open(
                "tools",
                Spec {
                    flags: Flags::CLIP,
                    size: [fill(), px(TOOLBAR)],
                    pad: [face.pad[0], edge],
                    offset: [0.0, -edge],
                    gap: if theme.tool[3] > 0.0 && !panel {
                        FACE_GAP
                    } else {
                        GAP
                    },
                    ..face
                },
            );
            self.tools(theme);
            self.ui.close();
        } else {
            self.ui.leaf(
                "space",
                Spec {
                    size: [fill(), px(1.0)],
                    ..Spec::default()
                },
            );
        }
        #[cfg(target_arch = "wasm32")]
        if ui::button(&mut self.ui, "download", "Download Desktop App").clicked {
            platform::reveal("https://file.paperclover.net/shr/snowbound/latest/");
        }
        self.ui.close();
        if title {
            platform::window_controls(&mut self.ui, &self.window, true);
        }
        self.ui.leaf(
            "trail",
            Spec {
                size: [px(self.trailing() - GAP), px(1.0)],
                ..Spec::default()
            },
        );
        self.ui.close();
        let narrowest = self
            .ui
            .narrowest(row)
            .map_or(MIN_SIZE[0], |row| row.ceil().max(MIN_SIZE[0]));
        if narrowest != self.min_width {
            self.min_width = narrowest;
            platform::set_min_size(&self.window, [narrowest, MIN_SIZE[1]]);
        }
        Ok(())
    }

    /// The toolbar's groups, as OneNote's ribbon groups its buttons. The groups used most in
    /// taking notes fold last, and those with chords or a place in the menu bar first.
    fn tools(&mut self, theme: &Theme) {
        use Entry::{Open, Rule, Run};
        use canvas::editor::{Alignment, BULLET_LIBRARY, ListStyle, NUMBER_LIBRARY, Toggle};
        use commands::{Choice, Id as Cmd};
        // A focused picture has no text to show a format for.
        let state = self.format_state();
        let statuses: Vec<_> = commands::COMMANDS
            .iter()
            .map(|command| command.id)
            .chain((0..self.tags.len()).map(Cmd::Tag))
            .map(|id| (id, self.status(&Choice::Command(id), &state)))
            .collect();
        let status_of = |id| {
            statuses
                .iter()
                .find(|(listed, _)| *listed == id)
                .map_or_else(commands::Status::default, |(_, status)| *status)
        };
        let status_of = &status_of;
        // A font or size applies where the font commands do.
        let fonts_apply = self.status(&Choice::Font(String::new()), &state).enabled;
        let fonts = &self.fonts;
        let recent_fonts = &self.recent_fonts;
        let pens = &self.toolbar;
        let engine = &self.view.engine;
        let session = self.session.as_ref();
        let update = self.updates.status();
        let sheet = self.gallery_sheet();
        let section = self.section_color();
        let drawing_pens = self.pens();
        let ui = &mut self.ui;
        let text = theme.text;
        let tags: Vec<_> = self.tags.iter().enumerate().collect();
        let face = tool_face(theme, grouped(theme), FACE_RISE);
        let mut choice = group(
            ui,
            "navigate",
            6,
            face.clone(),
            |ui| {
                let mut choice = None;
                for id in [Cmd::Undo, Cmd::Redo] {
                    choice = tool(ui, id, status_of(id)).or(choice);
                }
                choice
            },
            |ui| {
                dropdown(
                    ui,
                    "menu",
                    Head::Split(Cmd::Undo),
                    &[Run(Cmd::Redo)],
                    status_of,
                )
            },
        );
        ui.open(
            "clipboard",
            Spec {
                flags: Flags::CLICKABLE,
                ..face.clone()
            },
        );
        divider(ui, theme);
        let entries = [
            Run(Cmd::Paste),
            Open("paste", Cmd::Paste),
            Rule,
            Run(Cmd::Cut),
            Run(Cmd::Copy),
        ];
        let head = Head::Menu("Clipboard", art::PASTE);
        choice = dropdown(ui, "menu", head, &entries, status_of).or(choice);
        // Pasting keeps only the text until #25.
        let options = [
            ("Keep Source Formatting", false, art::KEEP_SOURCE_FORMATTING),
            ("Merge Formatting", false, art::MERGE_FORMATTING),
            ("Keep Text Only", true, art::KEEP_TEXT_ONLY),
            ("Picture", false, art::PASTE_PICTURE),
        ];
        let mut items = vec![ui::popup::Item {
            text: "Paste Options",
            heading: true,
            ..Default::default()
        }];
        items.extend(options.map(|(text, enabled, icon)| ui::popup::Item {
            text,
            icon: Some(icon),
            disabled: !enabled,
            ..Default::default()
        }));
        let anchor = ui::Anchor::Below(ui.id("menu"));
        if ui::popup::menu(ui, toolbar_popup("paste"), anchor, &items, None).is_some() {
            choice = Some(Choice::Command(Cmd::Paste));
        }
        ui.close();
        let font = state.font.clone().unwrap_or_default();
        let size = state
            .font_size
            .map_or(String::new(), |size| format!("{size}"));
        ui.open(
            "font",
            Spec {
                flags: Flags::CLICKABLE,
                gap: 1.0,
                ..face.clone()
            },
        );
        divider(ui, theme);
        // Styles, the font and the size never fold.
        let styles = toolbar_popup("styles");
        let anchor = if status_of(Cmd::Styles).enabled {
            ui::shell::menu_button(ui, "styles", art::STYLES, None, styles)
        } else {
            ui::shell::unavailable(ui, "styles", art::STYLES, text, true);
            ui::Anchor::Below(ui.id("styles"))
        };
        let shown = notebook::sidecar::themes::STYLES
            .iter()
            .find(|(name, _)| state.style.as_deref() == Some(*name))
            .map_or("Styles".to_owned(), |(_, label)| format!("Styles: {label}"));
        ui::popup::tooltip(ui, &shown, "", None);
        if let Some(id) =
            themes::gallery(ui, styles, anchor, &sheet, section, state.style.as_deref())
        {
            choice = Some(Choice::Command(id));
        }
        let combo = ui.id("font");
        // It gives up room last of all, once every group has folded.
        let width = ui::Extent {
            size: ui::Size::Pixels(120.0),
            strictness: 64.0 / 120.0,
        };
        let menu = toolbar_popup("font");
        ui::shell::combo(ui, "font", "Font", &font, width, menu, fonts_apply);
        // Each family is named with the substitute it shows in, and picked by its own name; a
        // family of None heads a group. Typing searches the full list.
        let named = |name: &str| {
            engine.substitute(name).map_or_else(
                || name.to_owned(),
                |substitute| format!("{} ({name})", substitute.name),
            )
        };
        let mut choices: Vec<(String, Option<&str>, bool)> = Vec::new();
        if ui::popup::query(ui, toolbar_popup("font")).is_none_or(str::is_empty) {
            choices.extend(FONTS.map(|name| (named(name), Some(name), false)));
            if !recent_fonts.is_empty() {
                choices.push(("Recent".to_owned(), None, true));
                choices.extend(
                    recent_fonts
                        .iter()
                        .map(|name| (named(name), Some(name.as_str()), false)),
                );
            }
        }
        let listed = choices.len();
        choices.extend(
            fonts
                .iter()
                .enumerate()
                .map(|(index, name)| (named(name), Some(name.as_str()), index == 0 && listed > 0)),
        );
        let items: Vec<_> = choices
            .iter()
            .map(|(label, name, separated)| ui::popup::Item {
                text: label,
                font: *name,
                current: *name == Some(font.as_str()),
                heading: name.is_none(),
                separated: *separated,
                shortcut: if name.is_some() && *name == state.style_font.as_deref() {
                    "Default"
                } else {
                    ""
                },
                ..Default::default()
            })
            .collect();
        let anchor = ui::Anchor::Over(combo);
        if let Some(index) =
            ui::popup::menu(ui, toolbar_popup("font"), anchor, &items, Some("Font"))
            && let Some(name) = choices[index].1
        {
            choice = Some(Choice::Font(name.to_owned()));
        }
        let combo = ui.id("size");
        let menu = toolbar_popup("size");
        ui::shell::combo(ui, "size", "Font Size", &size, 44.0, menu, fonts_apply);
        // A number typed offers that size alone, cut to the half point below, or else the
        // range OneNote 2010 takes, as the lab's OneNote 2010 does.
        let typed = ui::popup::query(ui, toolbar_popup("size")).map(str::trim);
        let number = typed.and_then(|typed| typed.parse::<f32>().ok());
        let sizes = match number.map(|typed| (typed * 2.0).floor() / 2.0) {
            Some(typed) if onestore::FONT_SIZES.contains(&typed) => vec![typed],
            Some(_) => Vec::new(),
            None => SIZES.to_vec(),
        };
        let labels: Vec<_> = sizes.iter().map(|size| format!("{size}")).collect();
        let mut items: Vec<_> = labels
            .iter()
            .zip(&sizes)
            .map(|(label, points)| ui::popup::Item {
                text: label,
                checked: Some(*label == size),
                current: *label == size,
                shortcut: if state.style_size == Some(*points) {
                    "Default"
                } else {
                    ""
                },
                fallback: number.is_some(),
                ..Default::default()
            })
            .collect();
        let range = format!(
            "Type a size from {} to {}",
            onestore::FONT_SIZES.start(),
            onestore::FONT_SIZES.end()
        );
        if items.is_empty() || typed.is_some_and(|typed| !typed.is_empty()) && number.is_none() {
            items.push(ui::popup::Item {
                text: &range,
                disabled: true,
                fallback: true,
                ..Default::default()
            });
        }
        let anchor = ui::Anchor::Over(combo);
        if let Some(index) = ui::popup::menu(ui, toolbar_popup("size"), anchor, &items, Some(&size))
        {
            choice = Some(Choice::Size(sizes[index]));
        }
        for toggle in [Toggle::Bold, Toggle::Italic, Toggle::Underline] {
            let id = Cmd::Toggle(toggle);
            choice = tool(ui, id, status_of(id)).or(choice);
        }
        let effects = [
            Run(Cmd::Toggle(Toggle::Strikethrough)),
            Run(Cmd::Toggle(Toggle::Subscript)),
            Run(Cmd::Toggle(Toggle::Superscript)),
            Rule,
            Run(Cmd::ClearFormatting),
        ];
        // The rest of the group folds inside it, on its face.
        choice = group(
            ui,
            "character",
            8,
            Spec::default(),
            |ui| {
                let head = Head::Menu("Text Effects", art::STRIKETHROUGH);
                let mut choice = dropdown(ui, "script", head, &effects, status_of);
                // Each button applies its menu's last pick: the highlighter shows it in its
                // artwork, the font colour in a bar.
                for (part, id, swatches, columns, none, bar) in [
                    (
                        "highlight",
                        Cmd::Highlight,
                        &HIGHLIGHTS[..],
                        5,
                        "No Color",
                        None,
                    ),
                    (
                        "color",
                        Cmd::FontColor,
                        &FONT_COLORS[..],
                        10,
                        "Automatic",
                        Some(pens.font_color.map_or(text, colorref)),
                    ),
                ] {
                    let split = ui.id(part);
                    let icon = artwork(id).unwrap_or_default();
                    if !status_of(id).enabled {
                        ui::shell::unavailable(ui, part, icon, text, true);
                    } else if ui::shell::split_button(
                        ui,
                        part,
                        commands::command(id).title,
                        icon,
                        bar,
                        None,
                        toolbar_popup(part),
                    )
                    .clicked
                    {
                        choice = Some(Choice::Command(id));
                    }
                    tip(ui, id);
                    let colors: Vec<_> = swatches
                        .iter()
                        .map(|&(color, name)| (colorref(color), name))
                        .collect();
                    let anchor = ui::Anchor::Below(split);
                    if let Some(chosen) =
                        ui::popup::colors(ui, toolbar_popup(part), anchor, none, &colors, columns)
                    {
                        let color = chosen.and_then(|chosen| {
                            swatches
                                .iter()
                                .zip(&colors)
                                .find(|(_, (color, _))| *color == chosen)
                                .map(|((stored, _), _)| *stored)
                        });
                        choice = Some(if id == Cmd::Highlight {
                            Choice::Highlight(color)
                        } else {
                            Choice::Color(color)
                        });
                    }
                }
                tool(ui, Cmd::FormatPainter, status_of(Cmd::FormatPainter)).or(choice)
            },
            |ui| {
                let mut entries = effects.to_vec();
                entries.extend([
                    Rule,
                    Open("highlight", Cmd::Highlight),
                    Run(Cmd::FontColor),
                    Open("color", Cmd::FontColor),
                    Rule,
                    Run(Cmd::FormatPainter),
                ]);
                dropdown(ui, "menu", Head::Split(Cmd::Highlight), &entries, status_of)
            },
        )
        .or(choice);
        ui.close();
        let alignments = [Alignment::Left, Alignment::Center, Alignment::Right].map(Cmd::Align);
        let paragraph = [
            Rule,
            Run(alignments[0]),
            Run(alignments[1]),
            Run(alignments[2]),
            Rule,
            Run(Cmd::Outdent),
            Run(Cmd::Indent),
        ];
        choice = group(
            ui,
            "paragraph",
            7,
            face.clone(),
            |ui| {
                divider(ui, theme);
                let mut choice = None;
                // The buttons apply OneNote's default bullet and number, as its own do.
                for id in [Cmd::Bullets, Cmd::Numbering] {
                    let part = if id == Cmd::Bullets {
                        "bullets"
                    } else {
                        "numbering"
                    };
                    let icon = artwork(id).unwrap_or_default();
                    let status = status_of(id);
                    if !status.enabled {
                        ui::shell::unavailable(ui, part, icon, text, true);
                        toggled(ui, part, status);
                    } else if ui::shell::split_button(
                        ui,
                        part,
                        commands::command(id).title,
                        icon,
                        None,
                        status.checked,
                        toolbar_popup(part),
                    )
                    .clicked
                    {
                        choice = Some(Choice::Command(id));
                    }
                    tip(ui, id);
                }
                let [bullets, numbering] = ["bullets", "numbering"].map(|part| ui.id(part));
                let current = |bullet| match state.list {
                    Some(ListStyle::Bullet(place)) if bullet => Some(place),
                    Some(ListStyle::Number(place)) if !bullet => Some(place),
                    _ => None,
                };
                if let Some(place) = list_gallery(
                    ui,
                    toolbar_popup("bullets"),
                    ui::Anchor::Below(bullets),
                    ["Recently Used Bullets", "Bullet Library"],
                    (&pens.bullets, BULLET_LIBRARY.len()),
                    current(true),
                    [36.0, 36.0],
                    |ui, place| {
                        let (font, glyph, _) = BULLET_LIBRARY[place];
                        ui.leaf(
                            "glyph",
                            Spec {
                                size: [fill(), fill()],
                                text: Some(&canvas::outline::symbol_text(font, glyph)),
                                font: Some(font),
                                font_size: Some(18.0),
                                center: true,
                                ..Spec::default()
                            },
                        );
                    },
                ) {
                    choice = Some(Choice::List(place.map(ListStyle::Bullet)));
                }
                if let Some(place) = list_gallery(
                    ui,
                    toolbar_popup("numbering"),
                    ui::Anchor::Below(numbering),
                    ["Recently Used Number Formats", "Numbering Library"],
                    (&pens.numbering, NUMBER_LIBRARY.len()),
                    current(false),
                    [72.0, 3.0 * NUMBER_LINE + 8.0],
                    |ui, place| {
                        for number in 1..=3 {
                            let label = canvas::outline::numbered(NUMBER_LIBRARY[place], number)
                                .unwrap_or_default();
                            ui.open(
                                ("line", number),
                                Spec {
                                    size: [fill(), px(NUMBER_LINE)],
                                    gap: 3.0,
                                    ..Spec::default()
                                },
                            );
                            ui.leaf(
                                "number",
                                Spec {
                                    size: [fit(), fill()],
                                    text: Some(&label),
                                    font_size: Some(11.0),
                                    ..Spec::default()
                                },
                            );
                            // A line of text after the number, as OneNote's previews draw it.
                            ui.leaf(
                                "text",
                                Spec {
                                    size: [fill(), fill()],
                                    fill: Some(theme.text_dim),
                                    inset: [0.0, NUMBER_LINE / 2.0, 2.0, NUMBER_LINE / 2.0 - 1.0],
                                    ..Spec::default()
                                },
                            );
                            ui.close();
                        }
                    },
                ) {
                    choice = Some(Choice::List(place.map(ListStyle::Number)));
                }
                let aligned = alignments
                    .into_iter()
                    .find(|id| status_of(*id).checked == Some(true))
                    .unwrap_or(alignments[0]);
                let icon = artwork(aligned).unwrap_or_default();
                dropdown(
                    ui,
                    "align",
                    Head::Menu("Alignment", icon),
                    &paragraph[1..],
                    status_of,
                )
                .or(choice)
            },
            |ui| {
                let mut entries = vec![
                    Open("bullets", Cmd::Bullets),
                    Run(Cmd::Numbering),
                    Open("numbering", Cmd::Numbering),
                ];
                entries.extend(paragraph);
                dropdown(ui, "menu", Head::Split(Cmd::Bullets), &entries, status_of)
            },
        )
        .or(choice);
        // The gallery's menu: every tag, then managing them.
        let mut all_tags: Vec<_> = tags
            .iter()
            .map(|&(place, tag)| Entry::Tag(place, tag))
            .collect();
        all_tags.extend([
            Rule,
            Run(Cmd::CustomizeTags),
            Run(Cmd::RemoveTags),
            Rule,
            Run(Cmd::FindTags),
        ]);
        choice = group(
            ui,
            "tags",
            9,
            face.clone(),
            |ui| {
                divider(ui, theme);
                let mut choice = None;
                for &(place, tag) in tags.iter().take(3) {
                    choice = tag_tool(ui, place, tag, status_of(Cmd::Tag(place))).or(choice);
                }
                dropdown(ui, "tags", Head::More("More Tags"), &all_tags, status_of).or(choice)
            },
            |ui| {
                let head = match tags.first() {
                    Some(&(place, tag)) => Head::Tag(place, tag),
                    None => Head::More("Tags"),
                };
                dropdown(ui, "menu", head, &all_tags, status_of)
            },
        )
        .or(choice);
        // The Draw tab's tools, which Insert's menus list for when they fold, before Insert.
        // OneNote's shape gallery ends with Snap To Grid.
        let shapes = [
            ShapeKind::Line,
            ShapeKind::Arrow,
            ShapeKind::Rectangle,
            ShapeKind::Ellipse,
        ]
        .map(|kind| Run(Cmd::Shape(kind)))
        .into_iter()
        .chain([Rule, Run(Cmd::SnapToGrid)])
        .collect::<Vec<_>>();
        let drawing = [
            Run(Cmd::SelectType),
            Open("pen", Cmd::Pen),
            Run(Cmd::Eraser),
            Run(Cmd::Lasso),
            Rule,
        ]
        .into_iter()
        .chain(shapes.iter().copied());
        let inserted = [Cmd::Picture, Cmd::Link, Cmd::Date, Cmd::Equation];
        let more = [
            Cmd::ScreenClipping,
            Cmd::Attachment,
            Cmd::InsertSpace,
            Cmd::Time,
            Cmd::DateTime,
            Cmd::RecordAudio,
            Cmd::RecordVideo,
        ]
        .into_iter()
        .filter(|id| commands::offered(*id))
        .map(Run)
        .collect::<Vec<_>>();
        choice = group(
            ui,
            "insert",
            4,
            face.clone(),
            |ui| {
                divider(ui, theme);
                let mut choice = None;
                if status_of(Cmd::Table).enabled {
                    let anchor = ui::shell::menu_button(
                        ui,
                        "table",
                        art::TABLE,
                        None,
                        toolbar_popup("table"),
                    );
                    tip(ui, Cmd::Table);
                    if let Some([columns, rows]) =
                        ui::popup::table_picker(ui, toolbar_popup("table"), anchor, [10, 8])
                    {
                        choice = Some(Choice::Table([rows, columns]));
                    }
                } else {
                    ui::shell::unavailable(ui, "table", art::TABLE, text, true);
                    tip(ui, Cmd::Table);
                }
                for id in inserted {
                    choice = tool(ui, id, status_of(id)).or(choice);
                }
                if status_of(Cmd::Symbol).enabled {
                    let anchor = ui::shell::menu_button(
                        ui,
                        "symbol",
                        art::SYMBOL,
                        None,
                        toolbar_popup("symbol"),
                    );
                    tip(ui, Cmd::Symbol);
                    let symbols = symbol::recent(&pens.symbols);
                    if let Some(pick) =
                        symbol::gallery(ui, toolbar_popup("symbol"), anchor, &symbols)
                    {
                        choice = Some(Choice::Symbol(pick));
                    }
                } else {
                    ui::shell::unavailable(ui, "symbol", art::SYMBOL, text, true);
                    tip(ui, Cmd::Symbol);
                }
                let mut entries = more.clone();
                entries.push(Rule);
                entries.extend(drawing.clone());
                dropdown(
                    ui,
                    "more",
                    Head::More("More Insert Options"),
                    &entries,
                    status_of,
                )
                .or(choice)
            },
            |ui| {
                let mut entries = vec![Open("table", Cmd::Table)];
                entries.extend(inserted.map(Run));
                entries.push(Open("symbol", Cmd::Symbol));
                entries.push(Rule);
                entries.extend(more.iter().copied());
                entries.push(Rule);
                entries.extend(drawing.clone());
                dropdown(
                    ui,
                    "menu",
                    Head::Menu("Insert", art::PLUS),
                    &entries,
                    status_of,
                )
            },
        )
        .or(choice);
        // Where the row has room, some of Insert's menu shows as buttons too, and folds away
        // first, as the Draw group does.
        for (part, priority, ids) in [
            ("files", 0, [Cmd::ScreenClipping, Cmd::Attachment]),
            ("recording", 3, [Cmd::RecordAudio, Cmd::RecordVideo]),
        ] {
            choice = group(
                ui,
                part,
                priority,
                face.clone(),
                |ui| {
                    divider(ui, theme);
                    let mut choice = None;
                    for id in ids.into_iter().filter(|id| commands::offered(*id)) {
                        choice = tool(ui, id, status_of(id)).or(choice);
                    }
                    choice
                },
                |_| None,
            )
            .or(choice);
        }
        // Pen draws with the gallery's last pick. Folded, the group leaves its tools to
        // Insert's menus, so the narrowest row keeps its width.
        choice = group(
            ui,
            "draw",
            2,
            face.clone(),
            |ui| {
                divider(ui, theme);
                let mut choice = tool(ui, Cmd::SelectType, status_of(Cmd::SelectType));
                let status = status_of(Cmd::Pen);
                if status.enabled {
                    let pen = drawing_pens[pens.pen.min(drawing_pens.len() - 1)];
                    let bar = Some(pen.color.map_or(ui.theme.paper_ink, colorref));
                    let split = ui.id("pen");
                    if ui::shell::split_button(
                        ui,
                        "pen",
                        commands::command(Cmd::Pen).title,
                        art::PEN,
                        bar,
                        status.checked,
                        toolbar_popup("pen"),
                    )
                    .clicked
                    {
                        choice = Some(Choice::Command(Cmd::Pen));
                    }
                    tip(ui, Cmd::Pen);
                    let anchor = ui::Anchor::Below(split);
                    if let Some(place) = pen_gallery(ui, anchor, &drawing_pens, pens.pen) {
                        choice = Some(Choice::Pen(place));
                    }
                } else {
                    ui::shell::unavailable(ui, "pen", art::PEN, text, true);
                    tip(ui, Cmd::Pen);
                }
                for id in [Cmd::Eraser, Cmd::Lasso] {
                    choice = tool(ui, id, status_of(id)).or(choice);
                }
                dropdown(
                    ui,
                    "shapes",
                    Head::Tools("Shapes", art::SHAPES),
                    &shapes,
                    status_of,
                )
                .or(choice)
            },
            |_| None,
        )
        .or(choice);
        // Folded, Page Color stays in the view's menu at the row's end.
        choice = group(
            ui,
            "page color",
            1,
            face.clone(),
            |ui| {
                divider(ui, theme);
                if !status_of(Cmd::PageColor).enabled {
                    ui::shell::unavailable(ui, "page color", art::PAGE_COLOR, text, true);
                    tip(ui, Cmd::PageColor);
                    return None;
                }
                let menu = toolbar_popup("page color");
                let anchor = ui::shell::menu_button(ui, "page color", art::PAGE_COLOR, None, menu);
                tip(ui, Cmd::PageColor);
                background::menu(
                    ui,
                    menu,
                    anchor,
                    &mut self.thumbnails,
                    // The page's own colour aside.
                    canvas::gpu::Paper {
                        color: ui.theme.paper,
                        ink: ui.theme.paper_ink,
                    },
                    self.view.editor.page_color(),
                    self.view.editor.rule_lines(),
                    false,
                    &mut self.page_color_preview,
                )
            },
            |_| None,
        )
        .or(choice);
        ui.leaf(
            "space",
            Spec {
                size: [fill(), px(1.0)],
                ..Spec::default()
            },
        );
        if let Some(session) = session {
            sync::control(ui, session, &update, theme);
        }
        // The view's commands, with zoom and Page Color for where theirs have folded away.
        let views = [
            Run(Cmd::ZoomIn),
            Run(Cmd::ZoomOut),
            Run(Cmd::ActualSize),
            Rule,
            Open("page color", Cmd::PageColor),
            Rule,
            Run(Cmd::Sidebar),
            Run(Cmd::PageList),
            Run(Cmd::PagesMatchTheme),
            Run(Cmd::FullPageView),
            Rule,
            Run(Cmd::HideSpelling),
            Run(Cmd::Spelling),
            Run(Cmd::MarkdownShortcuts),
        ];
        ui.open(
            "view",
            Spec {
                flags: Flags::CLICKABLE,
                ..face.clone()
            },
        );
        let head = Head::More("More View Options");
        choice = dropdown(ui, "more", head, &views, status_of).or(choice);
        ui.close();
        if let Some(choice) = choice {
            self.choose(choice);
        }
    }

    /// The page's zoom, left of the search box: Zoom Out, the level and Zoom In joined as one
    /// control, which folds away first where the tab row lacks room. Clicking the level opens
    /// it as a field, where Enter zooms to the percentage typed and Escape keeps the zoom.
    fn zoom(&mut self, theme: &Theme) -> Result<(), Box<dyn Error>> {
        use commands::{Choice, Id as Cmd};
        use ui::shell::TOOL;
        const LEVEL: f32 = 44.0;
        let format = self.format_state();
        let [out, into] =
            [Cmd::ZoomOut, Cmd::ZoomIn].map(|id| self.status(&Choice::Command(id), &format));
        let level = format!("{:.0}%", self.view.zoom() * 100.0);
        let field = self.ui.id("zoom field");
        if self.ui.focused() != Some(field) {
            self.zoom_typed = None;
        }
        self.ui.open(
            "zoom",
            Spec {
                flags: Flags::CLICKABLE,
                fold: Some(0),
                ..Spec::default()
            },
        );
        self.ui.open(
            "full",
            Spec {
                size: [children(), px(TAB_ROW)],
                // As far from the search box as the page list's buttons are apart.
                pad: [3.0, (TAB_ROW - TOOL) / 2.0],
                ..Spec::default()
            },
        );
        let typing = self.zoom_typed.is_some();
        self.ui.open(
            "control",
            Spec {
                size: [children(), px(TOOL)],
                fill: Some(theme.base),
                border: Some(if typing { theme.accent } else { theme.chip }),
                radius: 4.0,
                ..Spec::default()
            },
        );
        let line = |ui: &mut Ui, part| {
            ui.leaf(
                part,
                Spec {
                    size: [px(1.0), px(TOOL)],
                    fill: Some(theme.chip),
                    ..Spec::default()
                },
            );
        };
        let mut choice = tool(&mut self.ui, Cmd::ZoomOut, out);
        line(&mut self.ui, "before");
        let mut zoom = None;
        if let Some(typed) = &mut self.zoom_typed {
            let signal = ui::text_field(
                &mut self.ui,
                field,
                typed,
                "",
                Spec {
                    size: [px(LEVEL), px(TOOL)],
                    pad: [4.0, 0.0],
                    ..Spec::default()
                },
            );
            name(&mut self.ui, field, "Zoom");
            let key = signal.events.iter().find_map(|event| match event {
                ui::Event::Key {
                    key: Key::Named(key @ (NamedKey::Enter | NamedKey::Escape)),
                    ..
                } => Some(*key),
                _ => None,
            });
            if let Some(key) = key {
                let percent = typed.trim().trim_end_matches('%').trim().parse::<f32>();
                zoom = percent.ok().filter(|_| key == NamedKey::Enter);
                self.zoom_typed = None;
                self.ui.set_focus(Some(page()));
            }
        } else {
            let id = self.ui.id("level");
            let signal = self.ui.leaf(
                "level",
                Spec {
                    // Zoom applies where its buttons do.
                    flags: if out.enabled {
                        Flags::CLICKABLE
                    } else {
                        Flags::default()
                    },
                    size: [px(LEVEL), px(TOOL)],
                    text: Some(&level),
                    color: Some(if out.enabled {
                        theme.text
                    } else {
                        theme.text_dim
                    }),
                    hover_fill: Some(theme.hover()),
                    center: true,
                    role: Some(accesskit::Role::Button),
                    ..Spec::default()
                },
            );
            if let Some(node) = self.ui.access(id) {
                node.set_label("Zoom");
                node.set_value(level.as_str());
            }
            if signal.clicked {
                self.zoom_typed = Some(level.trim_end_matches('%').to_owned());
                self.ui.focus_all(field);
            }
        }
        line(&mut self.ui, "after");
        choice = tool(&mut self.ui, Cmd::ZoomIn, into).or(choice);
        self.ui.close();
        self.ui.close();
        self.ui.open("folded", Spec::default());
        self.ui.close();
        self.ui.close();
        if let Some(choice) = choice {
            self.choose(choice);
        }
        if let Some(percent) = zoom {
            let response = self.view.set_zoom(percent / 100.0)?;
            self.respond(response);
        }
        Ok(())
    }

    /// The search box and the page list's buttons, above the list. Where the tab row lacks
    /// room the box folds to a button, which opens the search over the row; while finding
    /// on the page it stays.
    fn page_tools(&mut self, theme: &Theme) {
        let finding = self.search.finding;
        // It folds after the zoom.
        self.ui.open(
            "tools",
            Spec {
                flags: Flags::CLICKABLE,
                fold: (!finding).then_some(1),
                ..Spec::default()
            },
        );
        // Finding widens the box over the tab row's spare room, for the match count beside
        // the query.
        let wide = if finding {
            PAGE_LIST + 120.0
        } else {
            PAGE_LIST
        };
        let width = ui::Extent {
            size: ui::Size::Pixels(wide),
            strictness: PAGE_LIST / wide,
        };
        for folded in [false, true].into_iter().take(if finding { 1 } else { 2 }) {
            self.ui.open(
                folded,
                Spec {
                    size: [if folded { children() } else { width }, px(TAB_ROW)],
                    pad: [0.0, (TAB_ROW - ui::shell::TOOL) / 2.0],
                    gap: 3.0,
                    ..Spec::default()
                },
            );
            if folded {
                let open = self.ui.popup_open(search::results());
                if ui::shell::tool_button(
                    &mut self.ui,
                    "search",
                    art::SEARCH,
                    theme.text,
                    Some(open),
                )
                .pressed
                {
                    self.start_search();
                }
                tip(&mut self.ui, commands::Id::Search);
            } else if let Err(error) = self.search_box(theme) {
                eprintln!("{error}");
            }
            if ui::shell::tool_button(&mut self.ui, "new", art::PLUS, theme.text, None).clicked {
                self.choose(commands::Choice::Command(commands::Id::NewPage));
            }
            tip(&mut self.ui, commands::Id::NewPage);
            let toggle = if self.pages_open {
                art::SIDEBAR_COLLAPSE
            } else {
                art::SIDEBAR_EXPAND
            };
            if ui::shell::tool_button(&mut self.ui, "toggle", toggle, theme.text, None).clicked {
                self.pages_open = !self.pages_open;
            }
            tip(&mut self.ui, commands::Id::PageList);
            self.ui.close();
        }
        self.ui.close();
    }

    /// The section's pages as tabs down the frame's right side, or its left with OneNote's
    /// "Page tabs appear on the left", returning the open page's tab, which is the page's
    /// colour and joins it.
    fn page_list(&mut self, theme: &Theme, section: &ui::Section, tabs: Id) -> Option<Id> {
        let session = self.session.as_ref()?;
        let panel = self.ui.id("panel");
        let open = self.pages_open && !self.full_page;
        let width = self.ui.animate(panel, if open { PAGE_LIST } else { 0.0 });
        self.ui.open(
            "panel",
            Spec {
                flags: Flags::SCROLL | Flags::CLIP,
                axis: Axis::Y,
                size: [px(width), fill()],
                role: Some(accesskit::Role::TabList),
                ..Spec::default()
            },
        );
        if let Some(node) = self.ui.access(panel) {
            node.set_label("Pages");
            node.set_orientation(accesskit::Orientation::Vertical);
        }
        // Another section's pages are on their way.
        if self.loading().is_some() && self.switching.is_some_and(|(tab, _)| tab.is_some()) {
            self.ui.close();
            return None;
        }
        let found = self.search.found_in(&session.key());
        let unread = self.unread_pages();
        #[cfg(feature = "live")]
        let peers = self.peer_pages();
        #[cfg(not(feature = "live"))]
        let peers = HashMap::new();
        let rounding = self.rounding();
        let dragged = self.dragged_page();
        let rows = page_rows(
            &mut self.ui,
            theme,
            section,
            session,
            [&found, &unread],
            &peers,
            (rounding, self.page_tabs_left),
            self.renaming.as_mut(),
            dragged,
        );
        self.commands.extend(
            rows.version
                .map(|(page, version)| Command::OpenVersion { page, version }),
        );
        if let Some((space, point)) = rows.context {
            let target = menus::Target::Page {
                library: Arc::clone(&session.library),
                path: session.tabs[session.tab].path.clone(),
                space,
            };
            self.menu = Some((target, point));
            self.ui.open_popup(menus::id());
        }
        if let Some(space) = rows.renamed {
            self.rename(rename::Target::Page(space));
        }
        if let Some(keep) = rows.kept {
            self.finish_renaming(keep);
        }
        self.drag_pages(rows.held, &rows.places, rows.settled, tabs);
        if !self.dragged() {
            self.commands.extend(rows.clicked.map(Command::OpenPage));
        }
        if let Some(space) = rows.hovered {
            self.prefetch_page(space);
        }
        self.ui.close();
        rows.open
    }

    /// Hands the page the events routed to its box, in its device pixels.
    fn page_events(&mut self, events: Vec<ui::Event>) -> Result<(), Box<dyn Error>> {
        // The page shown is on its way out.
        if self.loading().is_some() {
            return Ok(());
        }
        let read_only = self.session.as_ref().is_some_and(Session::read_only);
        let scale = self.ui.scale();
        let corner = self.ui.laid_out(page()).unwrap_or_default();
        let device = |point: [f32; 2]| {
            [
                (point[0] - corner[0]) * scale,
                (point[1] - corner[1]) * scale,
            ]
        };
        for event in events {
            let response = match event {
                ui::Event::PointerMoved(point) => self.view.pointer_moved(device(point))?,
                ui::Event::Pressure(pressure) => {
                    self.view
                        .set_pressure(pressure.filter(|_| self.pen_pressure));
                    continue;
                }
                ui::Event::PointerLeft => self.view.pointer_left(),
                ui::Event::Button {
                    button: MouseButton::Left,
                    pressed,
                    at,
                } => {
                    if pressed {
                        let response = self.view.pointer_pressed(at)?;
                        // Drawing or clicking anywhere but the title puts the templates away.
                        if (self.view.inking() || !self.view.editor.active_outline().title)
                            && let Some(session) = &self.session
                        {
                            self.dismissed.insert(session.space);
                        }
                        response
                    } else {
                        let response = self.view.pointer_released()?;
                        let [anchor, focus] = self.view.editor.selection().positions;
                        if anchor != focus
                            && let Some(painted) = self.painter.take()
                        {
                            self.respond(response);
                            self.format(canvas::editor::Formatting::Paint(painted))?;
                            continue;
                        }
                        response
                    }
                }
                ui::Event::Button {
                    button: MouseButton::Right,
                    pressed: true,
                    ..
                } if !read_only => {
                    self.open_text_menu()?;
                    continue;
                }
                ui::Event::Button { .. } | ui::Event::Ime(Ime::Enabled) | ui::Event::Access(_) => {
                    continue;
                }
                // A conflict page takes no typing; commands and caret moves still apply.
                ui::Event::Ime(Ime::Preedit(..) | Ime::Commit(_)) if read_only => continue,
                ui::Event::Key { ref key, .. }
                    if read_only
                        && !self.view.modifiers().command
                        && !matches!(
                            key,
                            Key::Named(
                                NamedKey::ArrowLeft
                                    | NamedKey::ArrowRight
                                    | NamedKey::ArrowUp
                                    | NamedKey::ArrowDown
                                    | NamedKey::Home
                                    | NamedKey::End
                                    | NamedKey::PageUp
                                    | NamedKey::PageDown
                            )
                        ) =>
                {
                    continue;
                }
                ui::Event::Wheel(delta) => self.view.wheel([delta[0] * scale, delta[1] * scale])?,
                ui::Event::Key { key, text } => {
                    self.view.key(&ui::edit_key(&key), text.as_deref())?
                }
                ui::Event::Ime(Ime::Preedit(text, cursor)) => self.view.compose(text, cursor)?,
                ui::Event::Ime(Ime::Commit(text)) => self.view.commit_text(text)?,
                ui::Event::Ime(Ime::Disabled) => self.view.cancel_composition()?,
                ui::Event::Modifiers(modifiers) => {
                    self.view.modifiers_changed(ui::edit_modifiers(modifiers))?
                }
            };
            self.respond(response);
        }
        if self.view.editor.marked_range().is_none() {
            platform::clear_marked_text(&self.window);
        }
        Ok(())
    }

    /// Notes a page change for after the frame and queues what the page asked of the host.
    fn respond(&mut self, response: Response) {
        self.changed |= response.changed;
        self.moved |= response.moved;
        if let Some(request) = response.request {
            self.commands.push(Command::Page(request));
        }
    }

    fn apply(&mut self, command: Command) -> Result<(), Box<dyn Error>> {
        match command {
            Command::OpenSection(library, path) => {
                // Its files are on their way to the renamed folder.
                if self.folder_renaming(&library.location) {
                    return Ok(());
                }
                if library.locked(&path) {
                    return self.show_locked(library, path);
                }
                let tab = match &self.session {
                    // Back to the open section before another one opened.
                    Some(session)
                        if self.switching.is_some()
                            && Arc::ptr_eq(&session.library, &library)
                            && session.tabs[session.tab].path == path =>
                    {
                        self.stop_loading();
                        return Ok(());
                    }
                    Some(session) if session.library.location == library.location => {
                        session.tabs.iter().position(|tab| tab.path == path)
                    }
                    _ => None,
                };
                self.switching = Some((tab, Instant::now()));
                let notify = notify(self.proxy.clone());
                let last = self.last_pages.get(&library.key(&path)).copied();
                self.load(move || {
                    let start = Instant::now();
                    let section = library.open(&path, notify)?;
                    lap("switch section open", start);
                    let (session, page) = read_session(section, library, path, last)?;
                    Ok(Loaded::Section(Box::new(session), page))
                });
            }
            Command::OpenPage(space) => {
                let session = self.session.as_ref().ok_or("No section is open")?;
                if self.switching.is_some() && session.space == space && session.version.is_none() {
                    self.stop_loading();
                    return Ok(());
                }
                self.switching = Some((None, Instant::now()));
                let read = session.reader(space);
                self.load(move || Ok(Loaded::Page(space, read()?)));
            }
            Command::Versions { page, show } => {
                let session = self.session.as_mut().ok_or("No section is open")?;
                session.shown = show.then_some(page);
                let open = if show {
                    session.versions(page).first().map(|version| version.space)
                } else {
                    (session.space != page).then_some(page)
                };
                self.commands.extend(open.map(Command::OpenPage));
            }
            Command::DeleteVersion { page, version } => {
                let session = self.session.as_mut().ok_or("No section is open")?;
                session.section.delete_pages(&[version])?;
                session.refresh_conflicts()?;
                self.commands.push(Command::OpenPage(page));
            }
            Command::CopyVersion { version, tab } => {
                let session = self.session.as_mut().ok_or("No section is open")?;
                let page = match session.version {
                    Some(open) if open == version => {
                        session.section.version(session.space, open)?
                    }
                    _ => session.section.page(version)?,
                };
                if tab == session.tab {
                    session.section.import_page(&page, &self.author)?;
                    session.pages = session.section.pages()?;
                    self.edited(Vec::new());
                } else {
                    let library = Arc::clone(&session.library);
                    let path = session.tabs[tab].path.clone();
                    let (notify, author) = (notify(self.proxy.clone()), self.author.clone());
                    crate::spawn(move || {
                        let copied = library.open(&path, notify).and_then(|section| {
                            section.import_page(&page, &author)?;
                            Ok(section.close()?)
                        });
                        if let Err(error) = copied {
                            eprintln!("Copying the page failed: {error}");
                        }
                    });
                }
            }
            Command::SelectChange { forward } => self.select_change(forward)?,
            Command::History { page, show } => self.show_history(page, show)?,
            Command::OpenVersion { page, version } => self.open_version(page, version)?,
            Command::RestoreVersion { page, version } => self.restore_version(page, version)?,
            Command::DeletePageVersion { page, version } => self.delete_version(page, version)?,
            Command::DeleteAllVersions(scope) => self.delete_all_versions(scope)?,
            Command::OpenNotebook => platform::pick_notebook(
                "Open Notebook",
                self.reply(|state, path: PathBuf| {
                    state.open_path(&path);
                    Ok(())
                }),
            ),
            Command::OpenFromServer(location) => self.open_server(location.as_deref()),
            #[cfg(feature = "live")]
            Command::OpenShared => self.open_shared(),
            Command::NewNotebook => self.new_notebook(),
            Command::OpenGuide => self.open_guide()?,
            #[cfg(target_os = "linux")]
            Command::Install => desktop::install(),
            Command::CloseNotebook(library) => self.close_notebook(&library),
            Command::Structure(library, change) => {
                if let Some(undo) = undo::structure_undo(&library, &change) {
                    self.undo.record(undo);
                }
                self.restructure(library, change);
            }
            // The index follows the section's page list.
            Command::NewPage { under, title } => {
                self.new_page(under, &title)?;
                self.edited(Vec::new());
            }
            Command::DeletePages(pages) => {
                self.delete_pages(pages)?;
                self.edited(Vec::new());
            }
            Command::Recycle(request) => self.recycle(request)?,
            Command::Pages(edits) => self.edit_pages(edits)?,
            Command::MovePage { space, path } => {
                self.move_page(space, path)?;
                self.edited(Vec::new());
            }
            Command::Template(name) => self.apply_template(name)?,
            Command::Page(Request::EditDate(field)) => self.edit_date(field),
            Command::Page(Request::Copy(clip)) => self.clipboard.set(clip)?,
            Command::Page(Request::Paste) => self.paste()?,
            Command::Page(Request::OpenLink(address)) => self.open_link(&address)?,
            Command::Page(Request::OpenAttachment(file)) => self.open_attachment(&file)?,
            Command::Page(Request::Play { file, at_ms }) => self.play(&file, at_ms)?,
            Command::Choose(choice) => self.run(choice)?,
            Command::TagPicture => self.pick_tag_picture(),
            Command::Text(text) => {
                if let Some(context) = self.view.caret_context() {
                    self.text_command(context, text)?;
                }
            }
        }
        Ok(())
    }

    /// Runs `read` on a thread of its own, which lays out the page it read; `open_loaded`
    /// shows it unless a newer read was asked for meanwhile.
    fn load(&mut self, read: impl FnOnce() -> Result<Loaded, Box<dyn Error>> + Send + 'static) {
        self.loading += 1;
        let (id, sender, redraw) = (self.loading, self.loads.0.clone(), self.redraw.clone());
        let layouts = Arc::clone(&self.layouts);
        let requested = Instant::now();
        let scenes = Arc::clone(&self.prefetch.scenes);
        let open = self.session.as_ref().map(Session::key);
        crate::spawn(move || {
            let laid = read().and_then(|loaded| {
                lap("switch read", requested);
                let (shown, page) = match loaded {
                    Loaded::Section(session, page) => (Shown::Section(session), page),
                    Loaded::Created(session, page) => (Shown::Created(session), page),
                    Loaded::Page(space, page) => (Shown::Page(space), page),
                    Loaded::Version(space, version, page) => (Shown::Version(space, version), page),
                    Loaded::Library(library, path) => return Ok(Laid::Library(library, path)),
                };
                let mut engine = layouts.lock().map_err(|_| "Page layout failed")?;
                let start = Instant::now();
                let key = match &shown {
                    Shown::Section(session) | Shown::Created(session) => {
                        Some((session.key(), session.space))
                    }
                    Shown::Page(space) => open.map(|open| (open, *space)),
                    Shown::Version(..) => None,
                };
                // A page shown or prepared lately keeps the pictures it drew.
                let kept =
                    key.and_then(|(key, space)| prefetch::Prefetch::take(&scenes, &key, space));
                let (scene, editor) = match kept {
                    Some(mut scene) => {
                        let mut editor = CanvasEditor::from_page(page, &mut engine)?;
                        scene.refresh(&mut editor, &mut engine)?;
                        (scene, editor)
                    }
                    None => PageScene::from_page(page, &mut engine)?,
                };
                lap("switch layout", start);
                Ok(Laid::Page(Box::new(Opening {
                    requested,
                    loaded: shown,
                    scene: (scene, [0.0; 2]),
                    editor,
                    since: Instant::now(),
                })))
            });
            let laid = laid.map_err(|error| {
                eprintln!("{error}");
                plain(&*error, "page")
            });
            let _ = sender.send((id, laid));
            redraw.wake();
        });
    }

    /// Shows the newest page read once the pictures it shows first are drawn, or after
    /// `HOLD`.
    fn open_loaded(&mut self) -> Result<(), Box<dyn Error>> {
        let reads: Vec<Read> = self.loads.1.try_iter().collect();
        for (id, loaded) in reads {
            match loaded {
                // A later load supersedes what this one shows, but not its failure.
                Ok(_) if id != self.loading => {}
                Ok(Laid::Page(opening)) => self.opening = Some(*opening),
                Ok(Laid::Library(library, path)) => self.adopt(library, path.as_deref())?,
                Err(error) => {
                    if id == self.loading {
                        self.switching = None;
                    }
                    platform::alert("Couldn't open", &error);
                }
            }
        }
        let paper = canvas::gpu::Paper {
            color: self.ui.theme.paper,
            ink: self.ui.theme.paper_ink,
        };
        self.prefetch.draw(&self.view, paper, &self.redraw);
        let Some(mut opening) = self.opening.take() else {
            return Ok(());
        };
        let paper = paper.colored(opening.editor.page_color());
        if !self
            .view
            .prepare(&mut opening.scene, &opening.editor, paper, &self.redraw)
            && opening.since.elapsed() < HOLD
        {
            self.opening = Some(opening);
            return Ok(());
        }
        lap("switch pictures", opening.since);
        let requested = opening.requested;
        // A recording goes on the page it started on.
        self.stop_recording(true)?;
        // The page shown so far keeps what was typed while the next one loaded.
        self.persist()?;
        let mut leaving = self
            .session
            .as_ref()
            .filter(|session| !session.read_only())
            .map(|session| session.space);
        if let Some(session) = &self.session {
            let key = session.key();
            self.places
                .insert((key.clone(), session.space), self.view.place());
            self.last_pages.insert(key.clone(), session.space);
            if session.version.is_none()
                && let Some(scene) = self.view.scene.take()
            {
                self.prefetch.keep(key, session.space, scene);
            }
        }
        let created = matches!(opening.loaded, Shown::Created(_));
        match opening.loaded {
            Shown::Section(session) | Shown::Created(session) => {
                // The settings keep the notebook shown, not the section.
                let other_notebook = self
                    .session
                    .as_ref()
                    .is_none_or(|open| open.library.location != session.library.location);
                self.list(&session.library);
                // The section left stays open a while for coming back to; the notebook's
                // background sync takes it over once it is closed.
                self.locked = None;
                if let Some(previous) = self.session.replace(*session) {
                    let path = &previous.tabs[previous.tab].path;
                    let locked = self.leave(&previous.library, path);
                    let library = &previous.library;
                    if !locked && self.notebooks.iter().any(|open| Arc::ptr_eq(open, library)) {
                        library.keep(&previous.tabs[previous.tab].path, previous.section);
                    } else {
                        let section = previous.section;
                        crate::spawn(move || {
                            if let Err(error) = section.close() {
                                eprintln!("Synchronization stopped: {error}");
                            }
                        });
                    }
                }
                self.sectionless = None;
                if other_notebook {
                    self.save_settings();
                }
            }
            Shown::Page(space) | Shown::Version(space, _) => {
                let session = self.session.as_mut().ok_or("No section is open")?;
                session.space = space;
                session.version = match opening.loaded {
                    Shown::Version(_, version) => Some(version),
                    _ => None,
                };
                session.change = None;
                // Pages created, moved or deleted since the list was read.
                session.pages = session.section.pages()?;
                leaving = leaving.filter(|left| session.pages.iter().any(|(s, ..)| s == left));
            }
        }
        let place = self.session.as_ref().and_then(|session| {
            let key = session.key();
            self.places.get(&(key, session.space)).copied()
        });
        let left = self.view.open(opening.editor, Some(opening.scene), place);
        // Each page takes up the history it was left with.
        if let Some(leaving) = leaving {
            self.undo.park(leaving, left);
        }
        let shown = self
            .session
            .as_ref()
            .filter(|session| !session.read_only())
            .map(|session| session.space);
        if let Some(parked) = shown.and_then(|space| self.undo.resume(space)) {
            self.view.resume(parked)?;
        }
        lap("switch shown", requested);
        self.switching = None;
        self.visited();
        if let Some((space, action)) = self.after_open.take()
            && let Some(session) = self
                .session
                .as_ref()
                .filter(|session| session.space == space)
        {
            let target = menus::Target::Page {
                library: Arc::clone(&session.library),
                path: session.tabs[session.tab].path.clone(),
                space,
            };
            self.act_on(target, action);
        }
        self.opened()?;
        self.prefetch_around();
        self.refind(true)?;
        if self.title_focus.take().is_some_and(|space| {
            self.session
                .as_ref()
                .is_some_and(|session| session.space == space)
        }) && let Some(title) = self
            .view
            .editor
            .outlines()
            .iter()
            .find(|outline| outline.title)
        {
            self.view.editor.focus_outline(title.id)?;
            let response = self.view.focus_text()?;
            self.respond(response);
        }
        if created && let Some(session) = &self.session {
            let target = rename::Target::Entry {
                library: Arc::clone(&session.library),
                path: session.tabs[session.tab].path.clone(),
                in_tab: true,
            };
            self.rename(target);
        }
        Ok(())
    }

    /// Takes `library`, a notebook read again after a change, in place of the one it was;
    /// the open section, now at catalog `path` in it, stays open, reopened where it moved.
    /// Without `path` the notebook has no sections left, and one of its shows none.
    fn adopt(&mut self, library: Arc<Library>, path: Option<&str>) -> Result<(), Box<dyn Error>> {
        self.list(&library);
        let shown = |shown: &Library| shown.location == library.location;
        let Some(path) = path else {
            if self.sectionless.as_deref().is_some_and(shown)
                || self
                    .session
                    .as_ref()
                    .is_some_and(|session| shown(&session.library))
            {
                self.persist()?;
                if let Some(session) = self.session.take() {
                    session.section.close()?;
                }
                self.sectionless = Some(Arc::clone(&library));
                self.title();
            }
            self.show_arrived(&library);
            return Ok(());
        };
        let Some(session) = &mut self.session else {
            return Ok(());
        };
        let folder = path.rsplit_once('/').map_or("", |(folder, _)| folder);
        let space = session.space;
        if session.tabs[session.tab].path == path {
            session.tabs = library.tabs(folder);
            session.tab = session
                .tabs
                .iter()
                .position(|tab| tab.path == path)
                .ok_or("The section is not in its notebook")?;
            session.library = library;
        } else {
            // Its file moved: the replica it holds reopens on the file's new path.
            self.persist()?;
            if let Some(session) = self.session.take() {
                session.section.close()?;
            }
            let section = library.open(path, notify(self.proxy.clone()))?;
            let (session, page) = read_session(section, library, path.to_owned(), Some(space))?;
            self.session = Some(session);
            let response = self.view.refresh(page)?;
            self.respond(response);
        }
        self.save_settings();
        Ok(())
    }

    /// Lists `library` in place of the notebook read before at its location, or last, and
    /// brings down what iCloud Drive keeps elsewhere of it.
    fn list(&mut self, library: &Arc<Library>) {
        match (self.notebooks.iter_mut()).find(|open| open.location == library.location) {
            Some(open) => *open = Arc::clone(library),
            None => self.notebooks.push(Arc::clone(library)),
        }
        if library.in_icloud() && library.downloading() {
            self.fetch(Arc::clone(library), None);
        }
    }

    /// Follows a page shown in place of another.
    fn opened(&mut self) -> Result<(), Box<dyn Error>> {
        if let Err(error) = self.wear_theme(true) {
            eprintln!("Restyling the page failed: {error}");
        }
        // A page a search result shows leaves the keys with the search.
        if !search::takes_text(self.ui.focused()) {
            self.ui.set_focus(Some(page()));
        }
        self.title();
        self.update_accessibility(true)?;
        self.window.request_redraw();
        Ok(())
    }

    /// Follows an edit: saving, the title, then the input method's position and accessibility,
    /// whose failures must not cost the edit.
    fn after_edit(&mut self) -> Result<(), Box<dyn Error>> {
        let start = Instant::now();
        let saved = self.persist();
        lap("save", start);
        self.refind(false)?;
        self.title();
        self.after_move()?;
        saved
    }

    /// Follows the view moving: the input method's position and accessibility.
    fn after_move(&mut self) -> Result<(), Box<dyn Error>> {
        let scale = self.ui.scale();
        let corner = self.ui.laid_out(page()).unwrap_or_default();
        let [x0, y0, x1, y1] = self.view.caret_area()?;
        self.window.set_ime_cursor_area(
            LogicalPosition::new(x0 / scale + corner[0], y0 / scale + corner[1]),
            LogicalSize::new((x1 - x0) / scale, (y1 - y0) / scale),
        );
        let start = Instant::now();
        self.update_accessibility(true)?;
        lap("accessibility", start);
        Ok(())
    }

    /// The window's title names the page: a temporary page takes its title's first line.
    fn title(&self) {
        let title = match &self.session {
            Some(session) => {
                let file = session
                    .section
                    .file()
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                // OneNote titles a page of the recycle bin so.
                let binned = recycle::binned(&session.tabs[session.tab].path);
                let bin = if binned {
                    " (Read-Only - Recycle Bin)"
                } else {
                    ""
                };
                format!("{}{bin} · {file}", session.title())
            }
            None if !self.temporary => "Snowbound".to_owned(),
            None => {
                let editor = &self.view.editor;
                if !editor.active_outline().title {
                    return;
                }
                let title = editor
                    .active_outline()
                    .document()
                    .paragraphs()
                    .next()
                    .unwrap()
                    .text()
                    .split(['\u{000b}', '\n', '\r'])
                    .next()
                    .unwrap()
                    .to_owned();
                let title = if title.trim().is_empty() {
                    "Untitled".into()
                } else {
                    title
                };
                format!("{title} · Temporary page")
            }
        };
        if self.window.title() != title {
            self.window.set_title(&title);
            platform::represent(
                &self.window,
                self.session.as_ref().map(|session| session.section.file()),
            );
        }
    }

    fn edit_date(&mut self, field: DateField) {
        let Some(date) = self.view.editor.date() else {
            return;
        };
        let timestamp = date.timestamp();
        self.view.editor.finish_composition();
        platform::clear_marked_text(&self.window);
        let title = match field {
            DateField::Date => "Change Page Date",
            DateField::Time => "Change Page Time",
        };
        let page = self.session.as_ref().map(|session| session.space);
        let reply = self.reply(
            move |state, chosen: Result<(u64, [String; 2]), &'static str>| {
                let (changed, text) = chosen?;
                // The answer belongs to the page that asked, unless another has opened since.
                if state.session.as_ref().map(|session| session.space) != page
                    || state.view.editor.date().map(|date| date.timestamp()) != Some(timestamp)
                {
                    return Ok(());
                }
                let response = state.view.change_date(changed, text)?;
                state.respond(response);
                Ok(())
            },
        );
        platform::edit_date(timestamp, field, title, reply);
    }

    /// Where a dialog's answer goes: `then`, run with it on the event loop.
    fn reply<T: Send + 'static>(
        &self,
        then: impl FnOnce(&mut State, T) -> Result<(), Box<dyn Error>> + Send + 'static,
    ) -> Reply<T> {
        Reply::new(&self.proxy, then)
    }

    /// The open conflict page's conflicting changes in page order: each text's outline,
    /// paragraph position and length.
    fn changes(&self) -> Vec<(ExGuid, usize, u32)> {
        let Some((_, version)) = self
            .session
            .as_ref()
            .and_then(|session| session.conflict(session.space))
        else {
            return Vec::new();
        };
        let marked = |id: &ExGuid| version.objects.contains(id);
        let mut changes = Vec::new();
        for outline in self.view.editor.outlines() {
            for (at, node) in outline.document().text_nodes().enumerate() {
                let Some(text) = node.text() else {
                    continue;
                };
                if marked(&node.id) || marked(&text.id) {
                    let length = text.text.text().encode_utf16().count() as u32;
                    changes.push((outline.id, at, length));
                }
            }
        }
        changes
    }

    /// Selects the next or previous conflicting change, as OneNote's conflict menu does.
    fn select_change(&mut self, forward: bool) -> Result<(), Box<dyn Error>> {
        let changes = self.changes();
        let Some(session) = &mut self.session else {
            return Ok(());
        };
        let next = match (session.change, forward) {
            (None, true) => 0,
            (Some(at), true) => at + 1,
            (Some(at), false) if at > 0 => at - 1,
            _ => return Ok(()),
        };
        let Some(&(outline, paragraph, length)) = changes.get(next) else {
            return Ok(());
        };
        session.change = Some(next);
        let at = |offset| canvas::document::TextPosition { paragraph, offset };
        self.view.editor.focus_outline(outline)?;
        self.view.editor.select([at(0), at(length)].into())?;
        self.ui.set_focus(Some(page()));
        let response = self.view.focus_text()?;
        self.respond(response);
        self.window.request_redraw();
        Ok(())
    }

    /// Publishes the open section's stored edits now, rather than after a pause in typing,
    /// and waits up to `wait` for them to reach the file.
    fn publish_now(&self, wait: std::time::Duration) {
        let Some(session) = &self.session else {
            return;
        };
        let deadline = Instant::now() + wait;
        loop {
            session.section.wake();
            if Instant::now() >= deadline
                || session
                    .section
                    .pending()
                    .is_ok_and(|pending| pending.is_empty())
            {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    /// Hands the section the ops the editor recorded since the last call, as one edit; the
    /// section thread stores it.
    fn persist(&mut self) -> Result<(), Box<dyn Error>> {
        let ops = match self.view.editor.take_ops() {
            Ok(ops) => ops,
            Err(error) => return self.refused(&error.to_string()),
        };
        let Some(session) = &mut self.session else {
            return Ok(());
        };
        if ops.is_empty() {
            return Ok(());
        }
        if session.read_only() {
            // A conflict page or a version is read-only: it returns to what is stored.
            self.stale = true;
            return Ok(());
        }
        let space = session.space;
        session.section.apply(
            &self.author,
            onestore::op::Edit {
                at: filetime(),
                ops: ops
                    .into_iter()
                    .map(|op| onestore::op::Op::Page { space, op })
                    .collect(),
            },
        )?;
        // The page list shows the title as it is typed.
        if self.view.editor.active_outline().title {
            session.pages = session.section.pages()?;
        }
        self.edited(vec![space]);
        let editor = &self.view.editor;
        self.undo
            .edited(space, editor.history_depth(), editor.typing().is_some());
        Ok(())
    }

    /// Shows the page as stored after a change made elsewhere, in place: the caret,
    /// selection, scroll and drawn pictures stay. Waits for marked text to end.
    fn refresh(&mut self) -> Result<(), Box<dyn Error>> {
        if self.view.editor.marked_range().is_some() {
            self.stale = true;
            return Ok(());
        }
        self.stale = false;
        self.persist()?;
        let Some(session) = &self.session else {
            return Ok(());
        };
        let page = session.reader(session.space)()?;
        let response = self.view.refresh(page)?;
        self.respond(response);
        self.title();
        self.window.request_redraw();
        Ok(())
    }

    /// An edit the section could not store: the page returns to what is stored, and the
    /// text the editor showed goes on the clipboard so nothing typed is lost unannounced.
    fn refused(&mut self, error: &str) -> Result<(), Box<dyn Error>> {
        eprintln!("Saving failed: {error}");
        self.clipboard.set_text(page_text(&self.view.editor))?;
        // What the editor recorded since builds on the refused edit.
        let _ = self.view.editor.take_ops();
        self.view.editor.cancel_composition(&mut self.view.engine)?;
        platform::clear_marked_text(&self.window);
        self.refresh()?;
        platform::alert(
            "Change not saved",
            "The page now shows what was last saved. Your text is on the clipboard to paste back.",
        );
        Ok(())
    }

    /// Applies what the section, and each notebook's closed sections, reported since the
    /// last poll.
    fn synced(&mut self) -> Result<(), Box<dyn Error>> {
        self.wear_theme(false)?;
        // Every notebook's changes are taken, so none is reported again.
        let reported: Vec<(Arc<Library>, Vec<String>)> = (self.notebooks.iter())
            .filter_map(|library| {
                Some((Arc::clone(library), library.background.as_ref()?.changed()))
            })
            .filter(|(_, paths)| !paths.is_empty())
            .collect();
        for (library, paths) in &reported {
            #[cfg(all(feature = "live", target_arch = "wasm32"))]
            if library.joined.is_some() && paths.iter().any(|path| !path.ends_with(".one")) {
                let refreshed = Arc::new(library.with(library.reopen()?));
                let shown = self
                    .session
                    .as_ref()
                    .filter(|session| session.library.location == library.location)
                    .and_then(|session| {
                        let path = &session.tabs[session.tab].path;
                        if refreshed.contains(path) {
                            Some(path.clone())
                        } else {
                            refreshed.first_section()
                        }
                    });
                self.adopt(refreshed, shown.as_deref())?;
            }
            self.sections_changed(library, paths.clone());
        }
        let changed: Vec<String> = (reported.iter())
            .flat_map(|(library, paths)| paths.iter().map(|path| library.key(path)))
            .collect();
        if !changed.is_empty() {
            self.sync_index(false, changed);
        }
        // Reports are rare: a status changed, or a section did.
        self.window.request_redraw();
        let Some(session) = &mut self.session else {
            return Ok(());
        };
        let mut listed = false;
        let mut changed = false;
        let mut rejected = None;
        let mut elsewhere = Vec::new();
        for event in session.section.events() {
            use notebook::session::Event;
            match event {
                Event::Changed(spaces) => {
                    listed = true;
                    changed |= spaces.contains(&session.space);
                    elsewhere.extend_from_slice(&spaces);
                    self.search
                        .changed(session.key(), session.section.replica(), spaces);
                }
                Event::Rejected { spaces, error } => {
                    if spaces.contains(&session.space) {
                        rejected = Some(error);
                    } else {
                        eprintln!("Saving failed: {error}");
                    }
                }
                Event::Failed(error) => eprintln!("Synchronization stopped: {error}"),
                // Its guests hear of it sooner than the folder's watch would tell them.
                #[cfg(feature = "live")]
                Event::Attempt {
                    status: notebook::EditStatus::Published { .. },
                    ..
                } => {
                    if let Some(host) = self.peers.hosts.get(&session.library.location) {
                        host.touched(&[session.tabs[session.tab].path.clone()]);
                    }
                }
                Event::Attempt { .. } | Event::Unreachable(_) => {}
            }
        }
        session.sync = session.section.sync_status()?;
        if !elsewhere.is_empty() {
            self.changed_elsewhere(&elsewhere);
        }
        let Some(session) = &mut self.session else {
            return Ok(());
        };
        if listed {
            let back = session.shown;
            session.pages = session.section.pages()?;
            session.refresh_conflicts()?;
            session.refresh_history()?;
            let listed = |space: ExGuid| session.pages.iter().any(|(page, ..)| *page == space);
            let gone = session.version.is_some_and(|version| {
                !session
                    .page_versions(session.space)
                    .iter()
                    .any(|listed| listed.context == version)
            });
            if gone && listed(session.space) {
                // A version deleted elsewhere returns to its page.
                self.commands.push(Command::OpenPage(session.space));
                return Ok(());
            }
            if !listed(session.space) && session.conflict(session.space).is_none() {
                // A page removed elsewhere, or a conflict page deleted elsewhere, which
                // returns to its page.
                let open = back
                    .filter(|page| listed(*page))
                    .or(session.pages.first().map(|(page, ..)| *page))
                    .ok_or("The section has no pages")?;
                self.commands.push(Command::OpenPage(open));
                return Ok(());
            }
        }
        if let Some(error) = rejected {
            self.refused(&error)?;
        } else if changed {
            self.refresh()?;
        }
        Ok(())
    }

    /// Sends assistive technology what changed in the interface, then in the page where
    /// `page` asks or its tree is not grafted yet. The interface's tree goes first, since it
    /// holds the page's.
    fn update_accessibility(&mut self, page: bool) -> Result<(), Box<dyn Error>> {
        let built = self.ui.laid_out(self::page()).is_some();
        if !built && self.page_grafted {
            // Its graft is gone, and with it the tree, which must be sent whole again.
            self.accessibility.deactivate();
            self.page_grafted = false;
        }
        let graft = self::page().node();
        let title = self.window.title();
        let scale = self.window.scale_factor();
        let grafted = self.page_grafted;
        let mut focus_page = false;
        self.access_adapter.update_if_active(|| {
            let mut update = self.ui.accessibility(&title, scale);
            // The focus may rest on the graft only once it holds the page's tree.
            if update.focus == graft && !grafted {
                update.focus = ui::Id::ROOT.node();
                focus_page = true;
            }
            update
        });
        if !built || (!page && grafted) {
            return Ok(());
        }
        let mut error = None;
        let mut sent = false;
        let (view, ui, window) = (&self.view, &self.ui, &self.window);
        self.access_adapter.update_if_active(|| {
            sent = true;
            page_tree(view, ui, window, &mut self.accessibility).unwrap_or_else(|failure| {
                error = Some(failure);
                self.accessibility.deactivate();
                let mut root = accesskit::Node::new(accesskit::Role::Group);
                root.set_label("Page");
                accesskit::TreeUpdate {
                    nodes: vec![(accessibility::ROOT, root)],
                    tree: Some(accesskit::TreeInfo::new(accessibility::ROOT)),
                    tree_id: PAGE_TREE,
                    focus: accessibility::ROOT,
                }
            })
        });
        self.page_grafted = sent;
        if focus_page {
            self.access_adapter
                .update_if_active(|| accesskit::TreeUpdate {
                    nodes: Vec::new(),
                    tree: None,
                    tree_id: accesskit::TreeId::ROOT,
                    focus: graft,
                });
        }
        if let Some(error) = error {
            return Err(error.into());
        }
        Ok(())
    }

    /// Writes the window's whole accessibility tree to `path` as text, a line a node as the
    /// platform shows them, for a replay to check what assistive technology is told.
    fn write_accessibility(&self, path: &Path) -> Result<(), Box<dyn Error>> {
        use accesskit_consumer::{NodeRef, Tree, TreeChangeHandler, common_filter};
        struct Unwatched;
        impl TreeChangeHandler for Unwatched {
            fn node_added(&mut self, _: &NodeRef) {}
            fn node_updated(&mut self, _: &NodeRef, _: &NodeRef) {}
            fn focus_moved(&mut self, _: Option<&NodeRef>, _: Option<&NodeRef>) {}
            fn node_removed(&mut self, _: &NodeRef) {}
        }
        fn write(node: &NodeRef, depth: usize, out: &mut String) {
            let data = node.data();
            *out += &format!("{}{:?}", "  ".repeat(depth), node.role());
            for (name, text) in [
                ("", data.label()),
                ("= ", data.value()),
                ("keys ", data.keyboard_shortcut()),
                ("-- ", data.description()),
            ] {
                if let Some(text) = text {
                    *out += &format!(" {name}{text:?}");
                }
            }
            for (state, on) in [
                ("toggled", data.toggled().is_some()),
                ("on", data.toggled() == Some(accesskit::Toggled::True)),
                ("expanded", data.is_expanded() == Some(true)),
                ("selected", data.is_selected() == Some(true)),
                ("disabled", data.is_disabled()),
                ("focused", node.is_focused()),
            ] {
                if on {
                    *out += &format!(" [{state}]");
                }
            }
            out.push('\n');
            for child in node.filtered_children(common_filter) {
                write(&child, depth + 1, out);
            }
        }
        let graft = page().node();
        let mut chrome = self
            .ui
            .accessibility_tree(&self.window.title(), self.window.scale_factor());
        let focus = std::mem::replace(&mut chrome.focus, ui::Id::ROOT.node());
        let mut tree = Tree::new(chrome, true);
        if self.ui.laid_out(page()).is_some() {
            let mut fresh = accessibility::Accessibility::default();
            let page = page_tree(&self.view, &self.ui, &self.window, &mut fresh)?;
            tree.update_and_process_changes(page, &mut Unwatched);
        }
        if focus != graft || self.ui.laid_out(page()).is_some() {
            let update = accesskit::TreeUpdate {
                nodes: Vec::new(),
                tree: None,
                tree_id: accesskit::TreeId::ROOT,
                focus,
            };
            tree.update_and_process_changes(update, &mut Unwatched);
        }
        let mut out = String::new();
        write(&tree.state().root(), 0, &mut out);
        notebook::fs::write(path, out)?;
        Ok(())
    }

    /// Forgets what assistive technology was sent, so the next update sends both trees whole.
    fn deactivate_accessibility(&mut self) {
        self.ui.deactivate_accessibility();
        self.accessibility.deactivate();
        self.page_grafted = false;
    }

    fn access_action(&mut self, request: accesskit::ActionRequest) -> Result<(), Box<dyn Error>> {
        trace_input(&request);
        use accesskit::{Action, ActionData};
        if request.target_tree == accesskit::TreeId::ROOT {
            self.ui.event(ui::Event::Access(request));
            self.window.request_redraw();
            return Ok(());
        }
        if request.target_tree != PAGE_TREE {
            return Ok(());
        }
        if let Some(field) = self.accessibility.date_for_node(request.target_node) {
            if request.action == Action::Click {
                self.edit_date(field);
            }
            return Ok(());
        }
        if let Some(index) = self.accessibility.read_only_for_node(request.target_node) {
            if request.action == Action::Focus {
                self.window.focus_window();
                self.ui.set_focus(Some(page()));
                let response = self.view.focus_read_only(index)?;
                self.respond(response);
                self.window.request_redraw();
            }
            return Ok(());
        }
        let Some(outline) = self.accessibility.outline_for_node(request.target_node) else {
            return Ok(());
        };
        let view = &mut self.view;
        match (request.action, request.data) {
            (Action::Focus, _) => {
                view.editor.focus_outline(outline)?;
                self.window.focus_window();
            }
            (Action::SetTextSelection, Some(ActionData::SetTextSelection(selection))) => {
                let selection = self.accessibility.selection(outline, selection)?;
                view.editor.focus_outline(outline)?;
                view.editor.select(selection)?;
                trace_input(&view.editor.selection());
            }
            (Action::SetValue, Some(ActionData::Value(text))) => {
                view.editor.focus_outline(outline)?;
                view.editor.select_all()?;
                view.editor.commit_text(&mut view.engine, text.into())?;
            }
            (Action::ReplaceSelectedText, Some(ActionData::Value(text))) => {
                view.editor.focus_outline(outline)?;
                view.editor.commit_text(&mut view.engine, text.into())?;
            }
            _ => return Ok(()),
        }
        self.ui.set_focus(Some(page()));
        let response = self.view.focus_text()?;
        self.respond(response);
        self.window.request_redraw();
        Ok(())
    }

    fn draw(&mut self) -> Result<(), Box<dyn Error>> {
        let start = Instant::now();
        if let Some(path) = self.snapshot.take() {
            // A hidden window never paints, so nothing polls the pictures between snapshots.
            let paper = self.paper();
            if let Some((scene, _)) = &mut self.view.scene {
                scene.settle(Some(&self.view.editor), self.view.viewport.scale, paper);
            }
            self.snapshot(&path)?;
        }
        if let Some((dir, opened)) = &self.frames {
            let path = dir.join(format!("{:09.2}.png", opened.elapsed().as_secs_f64() * 1e3));
            let pixels = self.capture()?;
            let size = self.surface.size;
            // Encoding off the frame keeps the frames at the pace they are drawn.
            crate::spawn(move || {
                if let Err(error) = write_png(&path, size, &pixels) {
                    eprintln!("{error}");
                }
            });
        }
        if self.occluded {
            return Ok(());
        }
        let Some(frame) = self.surface.frame(&self.renderer)? else {
            return Ok(());
        };
        lap("acquire", start);
        self.paint(&frame.target)?;
        let start = Instant::now();
        self.surface.present(&self.renderer, frame);
        lap("present", start);
        trace_input(&"Present submitted");
        Ok(())
    }

    /// The window's pixels per point, lowered where the window is larger than the GPU's
    /// largest texture, so the surface renders scaled rather than failing.
    fn scale(&self) -> f32 {
        let size = self.window.inner_size();
        let most = self.renderer.max_texture_dimension() as f32;
        let longest = size.width.max(size.height) as f32;
        self.window.scale_factor() as f32 * (most / longest).min(1.0)
    }

    /// Marks misspelled words on the page unless Hide Spelling Errors is on.
    fn show_spelling(&mut self) {
        self.view.spelling = self.spelling.clone().filter(|_| !self.hide_spelling);
        self.window.request_redraw();
    }

    /// The paper the open page lies on: the theme's, in the page's colour or the one previewed.
    fn paper(&self) -> canvas::gpu::Paper {
        canvas::gpu::Paper {
            color: self.ui.theme.paper,
            ink: self.ui.theme.paper_ink,
        }
        .colored(
            self.page_color_preview
                .unwrap_or_else(|| self.view.editor.page_color()),
        )
    }

    /// The colours of what the page area shows in place of a page, which follow the page's:
    /// the light theme's on white pages, as Pages Match UI Theme off keeps them in a dark
    /// appearance.
    pub(crate) fn page_area_theme(&self) -> Theme {
        if self.light_pages {
            Theme {
                accent: self.ui.theme.accent,
                ..Theme::light()
            }
        } else {
            self.ui.theme.clone()
        }
    }

    /// Paints the interface with the page in its box.
    fn paint(&mut self, target: &draw::Target) -> Result<(), Box<dyn Error>> {
        let start = Instant::now();
        let paper = self.paper();
        self.view.update_pictures(paper, &self.redraw);
        if let Some(session) = &self.session {
            self.view.tag_art = session.library.tag_art();
        }
        let theme = &self.ui.theme;
        let scale = self.ui.scale();
        let corner = self.ui.laid_out(page()).unwrap_or_default();
        // A page taking a while to open shows its outline, drawn in points from the corner.
        let (page_primitives, viewport) = match self.loading() {
            Some(since) => (
                prefetch::skeleton(paper, since),
                canvas::gpu::Viewport {
                    scale,
                    origin: [0.0; 2],
                    ..self.view.viewport
                },
            ),
            None => (
                self.view.primitives(TextColors {
                    caret: theme.caret,
                    selection: if self.page_focused {
                        theme.selection
                    } else {
                        theme.inactive_selection
                    },
                    paper,
                })?,
                self.view.viewport,
            ),
        };
        lap("page primitives", start);
        let interface = self.ui.layers();
        let layers: Vec<_> = interface
            .iter()
            .map(|layer| match layer {
                ui::Layer::Primitives(primitives) => primitives.layer(scale),
                ui::Layer::Custom { rect, .. } => draw::Layer {
                    scale: viewport.scale,
                    origin: [
                        viewport.origin[0] + corner[0] * scale,
                        viewport.origin[1] + corner[1] * scale,
                    ],
                    clip: Some(rect.map(|value| value * scale)),
                    backdrop: Some(paper.color),
                    round: None,
                    motion: None,
                    primitives: &page_primitives,
                },
            })
            .collect();
        trace_input(&("Draw", viewport.origin, viewport.scale));
        // Where the system's material shows through, only what the chrome fills covers it.
        let clear = if self.surface.translucent() {
            [0.0; 4]
        } else {
            self.ui.theme.strip
        };
        let start = Instant::now();
        self.renderer
            .draw(target, self.surface.size, clear, &layers)
            .map_err(|error| format!("Canvas drawing failed: {error:?}"))?;
        lap("render", start);
        Ok(())
    }

    /// Writes the frame to a PNG, so a covered window can be reviewed.
    fn snapshot(&mut self, path: &Path) -> Result<(), Box<dyn Error>> {
        let pixels = self.capture()?;
        write_png(path, self.surface.size, &pixels)
    }

    /// The frame's sRGB RGBA rows, drawn offscreen.
    fn capture(&mut self) -> Result<Vec<u8>, Box<dyn Error>> {
        let offscreen = self.surface.offscreen(&self.renderer)?;
        self.paint(&offscreen.target)?;
        self.surface.read(&self.renderer, offscreen)
    }

    fn over_page(&self) -> bool {
        let [x0, y0, x1, y1] = self.ui.laid_out(page()).unwrap_or_default();
        let [x, y] = self.pointer;
        (x0..x1).contains(&x) && (y0..y1).contains(&y)
    }

    /// Window point `point` in the page's device pixels.
    fn page_point(&self, point: [f32; 2]) -> [f32; 2] {
        let [left, top, ..] = self.ui.laid_out(page()).unwrap_or_default();
        let scale = self.ui.scale();
        [(point[0] - left) * scale, (point[1] - top) * scale]
    }

    /// Zooms the page by `factor` about the pointer, when it is over the page.
    fn pinch(&mut self, factor: f32) -> Result<(), Box<dyn Error>> {
        if self.over_page() {
            // The pointer here, as the page's own lags by the moves queued for the next frame.
            let response = self.view.pinch(factor, self.page_point(self.pointer))?;
            self.respond(response);
        }
        Ok(())
    }

    fn swipe_scroll(&mut self, phase: winit::event::TouchPhase, delta: [f32; 2]) {
        let scroll = self.view.scroll();
        let armed = scroll.max[0] <= scroll.min[0] && self.over_page();
        if let Some(forward) = self.swipe.scroll(phase, delta, armed, Instant::now()) {
            self.travel(forward);
            self.window.request_redraw();
        }
    }

    /// Queues input for the next frame. A press on the strip moves the window: at once on
    /// macOS, while AppKit still holds the press, and elsewhere once the pointer moves, as
    /// the window manager's move would swallow a second click; a double press zooms it.
    fn input(&mut self, event: ui::Event) {
        if let ui::Event::PointerMoved(point) = event {
            self.pointer = point;
            if std::mem::take(&mut self.strip_held) {
                let _ = self.window.drag_window();
            }
        }
        if let ui::Event::Button { pressed: false, .. } = event {
            self.strip_held = false;
        }
        // Escape calls a drag off: what it held slides back.
        if let ui::Event::Key {
            key: Key::Named(NamedKey::Escape),
            ..
        } = event
            && let Some(drag) = self.drag.as_mut().filter(|drag| drag.live())
        {
            drag.cancelled = true;
            self.window.request_redraw();
            return;
        }
        if let ui::Event::Button { pressed: true, .. } = event
            && self.renaming.is_some()
            && self.ui.box_at(self.pointer) != Some(rename::field())
        {
            self.finish_renaming(true);
        }
        if let ui::Event::Button {
            button: MouseButton::Left,
            pressed: true,
            ..
        } = event
            && let Some(direction) = platform::resize_direction(&self.window, self.pointer)
        {
            let _ = self.window.drag_resize_window(direction);
            return;
        }
        if let ui::Event::Button {
            button: MouseButton::Left,
            pressed: true,
            at,
        } = event
            && matches!(self.ui.box_at(self.pointer), Some(id) if id == strip() || id == tab_row() || id == sidebar::header())
        {
            let double = self.strip_press.is_some_and(|last| {
                at.saturating_duration_since(last) <= platform::double_click_interval()
            });
            self.strip_press = (!double).then_some(at);
            if double {
                platform::zoom(&self.window);
            } else if cfg!(target_os = "macos") {
                let _ = self.window.drag_window();
            } else {
                self.strip_held = true;
            }
            return;
        }
        if self.record_chord(&event) {
            return;
        }
        // On macOS the menu bar takes the chords of the items it enables first. The open
        // palette takes ⌘K, Link's chord, for its actions.
        if let ui::Event::Key { key, .. } = &event
            && let Some(id) =
                commands::find(&ui::edit_key(key), ui::edit_modifiers(self.ui.modifiers()))
            && !(id == commands::Id::Link && self.ui.popup_open(palette::id()))
        {
            self.choose(commands::Choice::Command(id));
        } else {
            self.ui.event(event);
        }
        self.window.request_redraw();
    }
}

/// The interface's colours in `appearance`, on white pages when `light_pages`, and over the
/// system's material where it shows through as a `backdrop`; menus as the desktop's.
fn theme(appearance: winit::window::Theme, light_pages: bool, backdrop: bool) -> Theme {
    let mut theme = match appearance {
        winit::window::Theme::Dark => Theme::dark(),
        winit::window::Theme::Light => Theme::light(),
    };
    theme.desktop_menu = platform::menu(appearance);
    if light_pages {
        let light = Theme::light();
        [theme.paper, theme.paper_ink] = [light.paper, light.paper_ink];
    }
    if backdrop {
        platform::over_backdrop(theme, appearance)
    } else {
        theme
    }
}

/// The page's accessibility tree, as changed since `access` last sent it, for the graft at
/// the page's box: bounds in the window's pixels, which the interface's scale renders fewer
/// of when capped.
fn page_tree(
    view: &PageView,
    ui: &Ui,
    window: &Window,
    access: &mut accessibility::Accessibility,
) -> Result<accesskit::TreeUpdate, onestore::page::text::EditError> {
    let corner = ui.laid_out(page()).unwrap_or_default();
    let scale = ui.scale();
    let ratio = window.scale_factor() as f32 / scale;
    let viewport = canvas::gpu::Viewport {
        size: view.viewport.size.map(|side| (side as f32 * ratio) as u32),
        scale: view.viewport.scale * ratio,
        origin: [0, 1].map(|axis| (view.viewport.origin[axis] + corner[axis] * scale) * ratio),
    };
    let update = access.update(
        &view.editor,
        view.scene.as_ref(),
        viewport,
        "Page",
        view.outline_preview(),
        view.object_focus().and_then(|focus| focus.read_only()),
    )?;
    Ok(accesskit::TreeUpdate {
        tree_id: PAGE_TREE,
        ..update
    })
}

/// A section's colour as linear RGBA; sections without one take OneNote's default blue.
fn section_color(color: Option<u32>) -> [f32; 4] {
    color.map_or(draw::srgb(0x8a, 0xa8, 0xe4), canvas::gpu::colorref)
}

/// The colour a notebook's cover takes in `theme`: its COLORREF's hue at the shade a section's
/// takes, or the glyph's own orange without one.
fn notebook_color(theme: &Theme, color: Option<u32>) -> [f32; 4] {
    color.map_or(draw::srgb(0xf3, 0x9c, 0x28), |color| {
        theme.section(canvas::gpu::colorref(color)).accent
    })
}

/// OneNote's information bar for conflicting changes, clicked anywhere: above a page with
/// conflict pages it shows or hides them; above a conflict page it opens OneNote's menu,
/// whose previous and next changes `steps` enables and whose Copy Page To lists `sections`.
fn conflict_bar(ui: &mut Ui, bar: Bar, sections: &[&str], steps: [bool; 2]) -> Option<Command> {
    let row = ui.open(
        "conflict",
        Spec {
            flags: Flags::CLICKABLE,
            size: [fill(), children()],
            fill: Some(draw::srgb(0xff, 0xee, 0xc2)),
            hover_fill: Some(draw::srgb(0xff, 0xe4, 0xa6)),
            pad: [12.0, 6.0],
            gap: 8.0,
            role: Some(accesskit::Role::Button),
            ..Spec::default()
        },
    );
    // OneNote marks conflicts, not versions.
    if !matches!(bar, Bar::History { .. }) {
        ui.leaf(
            "icon",
            Spec {
                size: [fit(), px(16.0)],
                icon: Some(art::CONFLICT),
                color: Some(draw::srgb(0xc0, 0x48, 0x20)),
                ..Spec::default()
            },
        );
    }
    // OneNote 2010's words (corpus/conflict-page/native, corpus/page-versions/native).
    let [said, action] = match bar {
        Bar::Page { shown, .. } => [
            "This page has changes that could not be merged during synchronization.",
            if shown {
                "Click here to hide versions of the page with unmerged changes."
            } else {
                "Click here to show versions of the page with unmerged changes."
            },
        ],
        Bar::Version { .. } => [
            "Conflicting changes are highlighted in red. This page cannot be edited, but you can copy changes to the primary page.",
            "Click here for more options.",
        ],
        Bar::History { .. } => [
            "This is an earlier version of the page. It will be deleted over time.",
            "Click here to restore or delete this version.",
        ],
    };
    ui.open(
        "message",
        Spec {
            axis: Axis::Y,
            size: [fill(), children()],
            ..Spec::default()
        },
    );
    let line = |ui: &mut Ui, part: &str, text| {
        ui.open(
            part,
            Spec {
                size: [fill(), fit()],
                text: Some(text),
                overflow: ui::Overflow::Wrap,
                color: Some(draw::srgb(0x20, 0x20, 0x20)),
                ..Spec::default()
            },
        );
    };
    line(ui, "said", said);
    ui.close();
    line(ui, "action", action);
    // The menu opens under the line the click asks for, from its leading edge, as wide as
    // its items.
    let from = ui.id("menu");
    ui.leaf(
        "menu",
        Spec {
            flags: Flags::FLOAT,
            size: [px(0.0), fill()],
            ..Spec::default()
        },
    );
    ui.close();
    ui.close();
    ui.close();
    let clicked = ui.signal(row).clicked;
    let menu = Id::ROOT.child("conflict-menu");
    let copy = Id::ROOT.child("conflict-copy");
    let anchor = ui::Anchor::Below(from);
    match bar {
        Bar::Page { page, shown } => clicked.then_some(Command::Versions { page, show: !shown }),
        Bar::History {
            page,
            version,
            grouped,
        } => {
            if clicked {
                ui.open_popup(menu);
            }
            history::menu(ui, anchor, [page, version], grouped, sections)
        }
        Bar::Version { page, version } => {
            if clicked {
                ui.open_popup(menu);
            }
            let items = [
                ui::popup::Item {
                    text: "Delete Conflict Page",
                    icon: Some(art::DELETE),
                    ..Default::default()
                },
                ui::popup::Item {
                    text: "Copy Page To",
                    icon: Some(art::COPY),
                    disabled: sections.is_empty(),
                    ..Default::default()
                },
                ui::popup::Item {
                    text: "Select Previous Conflicting Change",
                    icon: Some(art::MOVE_UP),
                    disabled: !steps[0],
                    separated: true,
                    ..Default::default()
                },
                ui::popup::Item {
                    text: "Select Next Conflicting Change",
                    icon: Some(art::MOVE_DOWN),
                    disabled: !steps[1],
                    ..Default::default()
                },
                ui::popup::Item {
                    text: "Collapse Conflict Pages",
                    separated: true,
                    ..Default::default()
                },
            ];
            let chosen = match ui::popup::menu(ui, menu, anchor, &items, None) {
                Some(0) => Some(Command::DeleteVersion { page, version }),
                Some(1) => {
                    ui.open_popup(copy);
                    None
                }
                Some(2) => Some(Command::SelectChange { forward: false }),
                Some(3) => Some(Command::SelectChange { forward: true }),
                Some(_) => Some(Command::Versions { page, show: false }),
                None => None,
            };
            let targets: Vec<_> = sections
                .iter()
                .map(|name| ui::popup::Item {
                    text: name,
                    ..Default::default()
                })
                .collect();
            let tab = ui::popup::menu(ui, copy, anchor, &targets, None);
            chosen.or(tab.map(|tab| Command::CopyVersion { version, tab }))
        }
    }
}

/// What the page list's rows were asked this frame.
#[derive(Default)]
struct Rows {
    /// The open page's tab.
    open: Option<Id>,
    clicked: Option<ExGuid>,
    /// The page whose context menu was asked for, and where.
    context: Option<(ExGuid, [f32; 2])>,
    /// The page held down, which a drag moves.
    held: Option<ExGuid>,
    /// A page other than the open one under the pointer.
    hovered: Option<ExGuid>,
    /// A page version clicked: the page, and the version's context.
    version: Option<(ExGuid, ExGuid)>,
    /// The page whose tab was pressed twice in a row, which renames it.
    renamed: Option<ExGuid>,
    /// Whether the rename field kept or dropped the name typed, once it closes.
    kept: Option<bool>,
    /// Where each page's tab stands down the list with none dragged.
    places: Vec<f32>,
    /// The dragged tab, let go, stands in its place.
    settled: bool,
}

/// A page's tab being dragged: the page, the top it follows the pointer to down the list
/// until let go, and where among the other pages it would land.
#[derive(Clone, Copy)]
struct PageDrag {
    space: ExGuid,
    top: Option<f32>,
    slot: Option<usize>,
}

/// The page list's zero-height first box, where its rows are measured from.
fn page_list_top() -> &'static str {
    "top"
}

/// The page list's rows, a page's conflict pages beneath it while shown, and `marked` the
/// pages a search found and those unread. A `dragged` page's tab follows the pointer over the others, which
/// slide aside to open the gap it would land in; once let go it eases into its place.
#[allow(clippy::too_many_arguments)]
fn page_rows(
    ui: &mut Ui,
    theme: &Theme,
    section: &ui::Section,
    session: &Session,
    marked: [&HashSet<ExGuid>; 2],
    peers: &HashMap<ExGuid, Vec<[f32; 4]>>,
    shape: (f32, bool),
    mut renaming: Option<&mut rename::Renaming>,
    dragged: Option<PageDrag>,
) -> Rows {
    let mut rows = Rows {
        settled: true,
        ..Rows::default()
    };
    ui.leaf(page_list_top(), Spec::default());
    let from = dragged.and_then(|dragged| {
        session
            .pages
            .iter()
            .position(|(space, ..)| *space == dragged.space)
    });
    let slot = dragged.and_then(|dragged| dragged.slot).or(from);
    // Down the list as laid out without the dragged tab, and as it stands with it.
    let (mut laid, mut natural, mut gap) = (0.0, 0.0, None);
    let mut others = 0;
    let mut lifted = None;
    for (index, (space, title, level)) in session.pages.iter().enumerate() {
        rows.places.push(natural);
        if from == Some(index) {
            lifted = Some((space, title, level));
            natural += ROW;
            continue;
        }
        if Some(others) == slot {
            gap = Some(laid);
        }
        // Eased only while a tab is dragged: the list lays out anew once it ends.
        let shift = match (from, slot) {
            (Some(_), Some(slot)) => ui.animate(
                ui.id((space, "slide")),
                if others >= slot { ROW } else { 0.0 },
            ),
            _ => 0.0,
        };
        others += 1;
        let height = page_row(
            ui,
            theme,
            section,
            session,
            marked,
            peers,
            shape,
            renaming.as_deref_mut(),
            (space, title, *level),
            shift,
            false,
            &mut rows,
        );
        laid += height;
        natural += height;
    }
    if let (Some((space, title, level)), Some(dragged)) = (lifted, dragged) {
        let gap = gap.unwrap_or(laid);
        let key = ui.id((space, "dragged"));
        let top = match dragged.top {
            Some(top) => ui.hold(key, top),
            None => {
                let top = ui.animate(key, gap);
                rows.settled = (top - gap).abs() < 0.5;
                top
            }
        };
        page_row(
            ui,
            theme,
            section,
            session,
            marked,
            peers,
            shape,
            renaming,
            (space, title, *level),
            top - laid,
            dragged.top.is_some(),
            &mut rows,
        );
    }
    rows
}

/// The tab of page `space` with its versions or conflict pages beneath it while shown,
/// drawn `shift` down from where they are laid out, and `lifted` as a dragged tab is:
/// how tall they are.
#[allow(clippy::too_many_arguments)]
fn page_row(
    ui: &mut Ui,
    theme: &Theme,
    section: &ui::Section,
    session: &Session,
    [found, unread]: [&HashSet<ExGuid>; 2],
    peers: &HashMap<ExGuid, Vec<[f32; 4]>>,
    shape: (f32, bool),
    renaming: Option<&mut rename::Renaming>,
    (space, title, level): (&ExGuid, &String, u32),
    shift: f32,
    lifted: bool,
    rows: &mut Rows,
) -> f32 {
    let mut height = ROW;
    let versions = session.versions(*space);
    let tab = PageTab {
        label: if title.is_empty() {
            "Untitled page"
        } else {
            title
        },
        dim: title.is_empty(),
        indent: level.saturating_sub(1),
        conflicted: !versions.is_empty(),
        found: found.contains(space),
        unread: unread.contains(space),
        peers: peers.get(space).map_or(&[], Vec::as_slice),
        renaming: renaming
            .filter(|renaming| renaming.page(*space))
            .map(|renaming| &mut renaming.name),
        shift,
        lifted,
    };
    let selected = *space == session.space && session.version.is_none();
    if selected && !lifted {
        rows.open = Some(ui.id(space));
    }
    let (signal, kept) = page_tab(ui, theme, section, space, tab, selected, shape);
    rows.kept = rows.kept.or(kept);
    if signal.hovered && !selected {
        rows.hovered = Some(*space);
    }
    if signal.clicked && !selected {
        rows.clicked = Some(*space);
    }
    if signal.pressed && signal.unit != draw::edit::SelectionUnit::Grapheme {
        rows.renamed = Some(*space);
    }
    if let Some(point) = signal.context {
        rows.context = Some((*space, point));
    }
    if signal.dragging {
        rows.held = Some(*space);
    }
    if lifted {
        return height;
    }
    // OneNote lists a page's conflict pages beneath it, each by its date and whose
    // version it is, and its page versions alike.
    let muted = ui::Section {
        tab: ui::mix(section.tab, theme.chip, 0.6),
        ..*section
    };
    // Each row: its id, its label, whether it is open, and whether it is a page version.
    let history = (session.page_versions(*space).iter())
        .filter(|_| session.shown_history == Some(*space))
        .map(|version| {
            let open = session.version == Some(version.context);
            (version.context, history::label(version), open, true)
        });
    let conflicts = (versions.iter())
        .filter(|_| session.shown == Some(*space))
        .map(|version| {
            let label = match version.created {
                Some(created) => format!("{} {}", platform::short_date(created), version.user),
                None => version.user.clone(),
            };
            (version.space, label, version.space == session.space, false)
        });
    for (id, label, selected, version) in history.chain(conflicts) {
        let tab = PageTab {
            label: &label,
            dim: false,
            indent: level.saturating_sub(1),
            conflicted: false,
            found: false,
            unread: false,
            peers: &[],
            renaming: None,
            shift,
            lifted: false,
        };
        if selected {
            rows.open = Some(ui.id(id));
        }
        height += ROW;
        let clicked = page_tab(ui, theme, &muted, &id, tab, selected, shape)
            .0
            .clicked;
        if clicked && !selected && version {
            rows.version = Some((*space, id));
        } else if clicked && !selected {
            rows.clicked = Some(id);
        }
    }
    height
}

/// A row of the page list.
struct PageTab<'a> {
    label: &'a str,
    /// An untitled page's placeholder label.
    dim: bool,
    indent: u32,
    /// The page has conflict pages, which OneNote marks at the tab's end.
    conflicted: bool,
    /// A search found the page, which OneNote marks yellow.
    found: bool,
    /// Another author changed it since it was last viewed, which its dot marks.
    unread: bool,
    /// The colours of the others who have it open.
    peers: &'a [[f32; 4]],
    /// The name typed in the tab's rename field, while it shows one.
    renaming: Option<&'a mut String>,
    /// How far down from its place in the list it is drawn.
    shift: f32,
    /// Dragged: lifted over the others with a shadow.
    lifted: bool,
}

/// A page's tab down the frame's side, the left where `shape` says so beside its `rounding`:
/// the open one is the page's colour and joins it, the others float free of it as pills
/// rounded like the page.
fn page_tab(
    ui: &mut Ui,
    theme: &Theme,
    section: &ui::Section,
    id: &ExGuid,
    tab: PageTab,
    selected: bool,
    (rounding, left): (f32, bool),
) -> (ui::Signal, Option<bool>) {
    // A rename field's text stands where the label did, past room for the unread dot.
    let pad = 16.0 + 16.0 * tab.indent as f32
        - if tab.renaming.is_some() {
            rename::PAD
        } else {
            0.0
        };
    let spec = Spec {
        flags: Flags::CLICKABLE,
        size: [px(PAGE_LIST), px(ROW)],
        pad: [pad, 0.0],
        offset: [0.0, tab.shift],
        shadow: tab.lifted.then_some([0.0, 0.0, 0.0, 0.35]),
        role: Some(accesskit::Role::Tab),
        ..Spec::default()
    };
    let (spec, color) = if selected {
        let spec = Spec {
            fill: Some(theme.paper),
            inset: [0.0, 0.0, 0.0, ROW_GAP],
            ..spec
        };
        (spec, theme.paper_ink)
    } else {
        // A found page's yellow lies over the tab, hovered or not.
        let tint = |fill| {
            if tab.found {
                ui::mix(fill, draw::srgb(0xff, 0xd8, 0x30), 0.6)
            } else {
                fill
            }
        };
        // A pill stands off the page by its margin and off the window's edge a little more.
        let [page_side, edge_side] = [PILL_MARGIN, 6.0];
        let [start, end] = if left {
            [edge_side, page_side]
        } else {
            [page_side, edge_side]
        };
        let spec = Spec {
            fill: Some(tint(section.tab)),
            hover_fill: Some(tint(section.hover())),
            radius: rounding,
            inset: [start, 0.0, end, ROW_GAP],
            // The label keeps its place as the tab opens and closes.
            pad: [if left { pad } else { pad - start }, 0.0],
            ..spec
        };
        let color = if tab.dim {
            ui::mix(theme.ink, section.tab, 0.5)
        } else {
            theme.ink
        };
        (spec, color)
    };
    // Just before the label, in the tab's leading pad.
    let dot = ui::shell::DOT;
    let unread = [
        (spec.pad[0] - dot - 2.0).max(spec.inset[0]),
        (ROW - ROW_GAP - dot) / 2.0,
    ];
    let row = ui.open(id, spec);
    if let Some(node) = ui.access(row) {
        node.set_selected(selected);
        node.set_level(tab.indent as usize + 1);
    }
    if tab.unread {
        ui::shell::unread_dot(ui, "unread", unread);
    }
    let kept = match tab.renaming {
        Some(name) => rename::edit(ui, theme, name, ROW - ROW_GAP),
        None => {
            ui.leaf(
                "label",
                Spec {
                    size: [fill(), px(ROW - ROW_GAP)],
                    text: Some(tab.label),
                    overflow: ui::Overflow::Ellipsis,
                    color: Some(color),
                    ..Spec::default()
                },
            );
            None
        }
    };
    // A dot for each of the first few people on the page, then how many more.
    const DOTS: usize = 4;
    for (index, dot) in tab.peers.iter().take(DOTS).enumerate() {
        let inset = (ROW - ROW_GAP - 8.0) / 2.0;
        ui.leaf(
            ("peer", index),
            Spec {
                size: [px(10.0), px(ROW - ROW_GAP)],
                fill: Some(*dot),
                radius: 4.0,
                inset: [1.0, inset, 1.0, inset],
                ..Spec::default()
            },
        );
    }
    if tab.peers.len() > DOTS {
        ui.leaf(
            "more peers",
            Spec {
                size: [fit(), px(ROW - ROW_GAP)],
                text: Some(&format!("+{}", tab.peers.len() - DOTS)),
                font_size: Some(10.0),
                color: Some(color),
                pad: [2.0, 0.0],
                ..Spec::default()
            },
        );
    }
    if tab.conflicted {
        ui.leaf(
            "conflict",
            Spec {
                size: [fit(), px(ROW - ROW_GAP)],
                icon: Some(art::CONFLICT),
                color: Some(draw::srgb(0xc0, 0x48, 0x20)),
                pad: [8.0, 0.0],
                ..Spec::default()
            },
        );
    }
    ui.close();
    (ui.signal(row), kept)
}

/// Puts `item` first among `recent` picks.
fn remember<T: PartialEq>(recent: &mut Vec<T>, item: T) {
    recent.retain(|known| *known != item);
    recent.insert(0, item);
    recent.truncate(RECENT);
}

/// Builds popup `id` as a gallery of a list library of `count` styles after None, under
/// `headings[1]`, and above it the library places `recent` under `headings[0]`, with library
/// place `current` outlined; `preview` builds a place's cell. Returns the place chosen, or
/// None for None.
#[allow(clippy::too_many_arguments)]
fn list_gallery(
    ui: &mut Ui,
    id: Id,
    anchor: ui::Anchor,
    headings: [&str; 2],
    (recent, count): (&[usize], usize),
    current: Option<usize>,
    size: [f32; 2],
    preview: impl Fn(&mut Ui, usize),
) -> Option<Option<usize>> {
    let groups = [(headings[0], recent.len()), (headings[1], count + 1)].map(|(heading, cells)| {
        ui::popup::Group {
            heading,
            ruled: false,
            cells,
            columns: 5,
            size,
        }
    });
    let place = |index: usize| match index.checked_sub(recent.len()) {
        None => Some(recent[index]),
        Some(at) => at.checked_sub(1),
    };
    let chosen = ui::popup::gallery(
        ui,
        id,
        anchor,
        &groups[usize::from(recent.is_empty())..],
        current.map(|place| recent.len() + 1 + place).as_slice(),
        |ui, index| match place(index) {
            Some(place) => preview(ui, place),
            None => {
                ui.leaf(
                    "none",
                    Spec {
                        size: [fill(), fill()],
                        text: Some("None"),
                        center: true,
                        ..Spec::default()
                    },
                );
            }
        },
    )?;
    Some(place(chosen))
}

/// The pen gallery below `anchor` while open: each pen's stroke in its colour, as thick as it
/// draws, `current` outlined. Returns the place chosen.
fn pen_gallery(
    ui: &mut Ui,
    anchor: ui::Anchor,
    pens: &[canvas::editor::Pen],
    current: usize,
) -> Option<usize> {
    const CELL: [f32; 2] = [40.0, 24.0];
    let groups = [ui::popup::Group {
        heading: "Pens",
        ruled: false,
        cells: pens.len(),
        columns: 5,
        size: CELL,
    }];
    let ink = ui.theme.paper_ink;
    ui::popup::gallery(
        ui,
        toolbar_popup("pen"),
        anchor,
        &groups,
        &[current],
        |ui, place| {
            let pen = pens[place];
            let tall = if pen.highlighter {
                10.0
            } else {
                (pen.width / 25.0).clamp(1.5, 4.0)
            };
            // Centred between two spacers, which share what the stroke leaves of the cell.
            let space = Spec {
                size: [fill(), fill()],
                ..Spec::default()
            };
            ui.leaf("above", space.clone());
            ui.leaf(
                "stroke",
                Spec {
                    size: [fill(), px(tall)],
                    fill: Some(pen.color.map_or(ink, colorref)),
                    inset: [2.0, 0.0, 2.0, 0.0],
                    radius: tall / 2.0,
                    ..Spec::default()
                },
            );
            ui.leaf("below", space);
        },
    )
}

/// A toolbar group on `face` that folds by `priority`: `full` builds its full form, `folded`
/// the form it folds to. Returns what either chose. Like every group, it takes presses
/// between its buttons, so only the row's empty space drags and zooms the window.
fn group(
    ui: &mut Ui,
    part: &str,
    priority: u32,
    face: Spec<'static>,
    full: impl FnOnce(&mut Ui) -> Option<commands::Choice>,
    folded: impl FnOnce(&mut Ui) -> Option<commands::Choice>,
) -> Option<commands::Choice> {
    ui.open(
        part,
        Spec {
            flags: Flags::CLICKABLE,
            fold: Some(priority),
            ..face
        },
    );
    ui.open(
        "full",
        Spec {
            gap: 1.0,
            ..Spec::default()
        },
    );
    let shown = full(ui);
    ui.close();
    ui.open("folded", Spec::default());
    let choice = folded(ui).or(shown);
    ui.close();
    ui.close();
    choice
}

/// A row of a toolbar menu.
#[derive(Clone, Copy)]
enum Entry<'a> {
    Run(commands::Id),
    /// Applies the tag at this place in the tag list.
    Tag(usize, &'a canvas::editor::NoteTag),
    /// Opens the toolbar's popup `name`, which applies the command, as a submenu.
    Open(&'static str, commands::Id),
    /// Rules off the entries after it.
    Rule,
}

/// What opens a toolbar menu: the arrow of a button running a command, a button showing
/// artwork, or a bare arrow; the last two named for assistive technology.
enum Head<'a> {
    Split(commands::Id),
    /// A button applying the tag at this place in the tag list.
    Tag(usize, &'a canvas::editor::NoteTag),
    Menu(&'static str, &'static [&'static str]),
    /// A `Menu` of tools, lit while one of them is on.
    Tools(&'static str, &'static [&'static str]),
    More(&'static str),
}

/// A toolbar menu `part` of `entries` opened by `head`, whose command enabled and checked as
/// `status_of` says. Returns the command chosen, or the head's when clicked.
fn dropdown(
    ui: &mut Ui,
    part: &str,
    head: Head,
    entries: &[Entry],
    status_of: &impl Fn(commands::Id) -> commands::Status,
) -> Option<commands::Choice> {
    use commands::Choice;
    let button = ui.id(part);
    // Not the split button's arrow, `button.child("menu")`.
    let menu = button.child("popup");
    let mut choice = None;
    let usable = entries.iter().any(|entry| match *entry {
        Entry::Run(id) | Entry::Open(_, id) => status_of(id).enabled,
        Entry::Tag(place, _) => status_of(commands::Id::Tag(place)).enabled,
        Entry::Rule => false,
    });
    let anchor = match head {
        Head::Split(id) if status_of(id).enabled => {
            let icon = artwork(id).unwrap_or_default();
            let on = status_of(id).checked;
            if ui::shell::split_button(ui, part, commands::command(id).title, icon, None, on, menu)
                .clicked
            {
                choice = Some(Choice::Command(id));
            }
            tip(ui, id);
            ui::Anchor::Below(button)
        }
        Head::Tag(place, tag) if status_of(commands::Id::Tag(place)).enabled => {
            let on = status_of(commands::Id::Tag(place)).checked;
            let art = tag_art(tag);
            if ui::shell::split_button(ui, part, &tag.label, art, None, on, menu).clicked {
                choice = Some(Choice::Command(commands::Id::Tag(place)));
            }
            tag_tip(ui, place, tag);
            ui::Anchor::Below(button)
        }
        // Where its command does not apply, the button only opens the menu.
        Head::Split(id) => {
            let icon = artwork(id).unwrap_or_default();
            let anchor = ui::shell::menu_button(ui, part, icon, None, menu);
            name(ui, button, commands::command(id).title);
            anchor
        }
        Head::Tag(_, tag) => {
            let anchor = ui::shell::menu_button(ui, part, tag_art(tag), None, menu);
            name(ui, button, &tag.label);
            anchor
        }
        // A menu of nothing that applies fades as a button would.
        Head::Menu(label, icon) | Head::Tools(label, icon) if !usable => {
            let tint = ui.theme.text;
            ui::shell::unavailable(ui, part, icon, tint, true);
            name(ui, button, label);
            ui::Anchor::Below(button)
        }
        Head::Menu(label, icon) => {
            let anchor = ui::shell::menu_button(ui, part, icon, None, menu);
            name(ui, button, label);
            anchor
        }
        Head::Tools(label, icon) => {
            let on = entries.iter().any(|entry| match *entry {
                Entry::Run(id) | Entry::Open(_, id) => status_of(id).checked == Some(true),
                _ => false,
            });
            let anchor = ui::shell::menu_button(ui, part, icon, Some(on), menu);
            name(ui, button, label);
            anchor
        }
        Head::More(label) => {
            let anchor = ui::shell::more_button(ui, part, menu, usable);
            name(ui, button, label);
            anchor
        }
    };
    let keys: Vec<_> = entries
        .iter()
        .map(|entry| match entry {
            Entry::Run(id) => commands::shortcut(*id),
            Entry::Tag(place, _) => commands::shortcut(commands::Id::Tag(*place)),
            _ => String::new(),
        })
        .collect();
    // A gallery row beside its command's own row, or under its button, is named for the
    // split button's arrow it stands in for.
    let runs = |id| {
        matches!(head, Head::Split(head) if head == id)
            || entries
                .iter()
                .any(|entry| matches!(entry, Entry::Run(run) if *run == id))
    };
    let options: Vec<_> = entries
        .iter()
        .map(|entry| match *entry {
            Entry::Open(_, id) if runs(id) => format!("{} Options", commands::command(id).title),
            _ => String::new(),
        })
        .collect();
    let (mut items, mut actions) = (Vec::new(), Vec::new());
    let mut separated = false;
    for ((entry, key), options) in entries.iter().zip(&keys).zip(&options) {
        let item = match *entry {
            Entry::Rule => {
                separated = true;
                continue;
            }
            Entry::Run(id) => ui::popup::Item {
                text: commands::command(id).title,
                icon: artwork(id),
                shortcut: key,
                checked: status_of(id).checked,
                disabled: !status_of(id).enabled,
                ..Default::default()
            },
            Entry::Tag(place, tag) => ui::popup::Item {
                text: &tag.label,
                icon: tags::artwork(tag),
                ink: tag.color.map(colorref),
                highlight: tag.highlight.map(colorref),
                tint: Some([1.0; 4]),
                shortcut: key,
                checked: status_of(commands::Id::Tag(place)).checked,
                disabled: !status_of(commands::Id::Tag(place)).enabled,
                ..Default::default()
            },
            Entry::Open(_, id) => ui::popup::Item {
                text: if options.is_empty() {
                    commands::command(id).title
                } else {
                    options
                },
                icon: artwork(id),
                submenu: true,
                disabled: !status_of(id).enabled,
                ..Default::default()
            },
        };
        items.push(ui::popup::Item {
            separated: std::mem::take(&mut separated),
            ..item
        });
        actions.push(*entry);
    }
    let chosen = ui::popup::menu(ui, menu, anchor, &items, None).map(|index| actions[index]);
    ui::popup::submenus(ui, menu, &items, |index| match actions[index] {
        Entry::Open(name, _) => Some(toolbar_popup(name)),
        _ => None,
    });
    match chosen {
        Some(Entry::Run(id)) => Some(Choice::Command(id)),
        Some(Entry::Tag(place, _)) => Some(Choice::Command(commands::Id::Tag(place))),
        _ => choice,
    }
}

/// Opens the trailing row of a dialog, whose buttons then stand at its right.
fn buttons(ui: &mut Ui) {
    ui.open(
        "buttons",
        Spec {
            size: [fill(), children()],
            pad: [0.0, 8.0],
            gap: 8.0,
            ..Spec::default()
        },
    );
    ui.leaf(
        "space",
        Spec {
            size: [fill(), px(1.0)],
            ..Spec::default()
        },
    );
}

/// Names box `id` to assistive technology, where no tooltip does.
fn name(ui: &mut Ui, id: Id, label: &str) {
    if let Some(node) = ui.access(id) {
        node.set_label(label);
    }
}

/// A tool button for command `id`, lit while `status` checks it and faded while it does not
/// apply; returns the command when clicked.
fn tool(ui: &mut Ui, id: commands::Id, status: commands::Status) -> Option<commands::Choice> {
    let part = commands::COMMANDS
        .iter()
        .position(|command| command.id == id);
    let icon = artwork(id).unwrap_or_default();
    let tint = ui.theme.text;
    if !status.enabled {
        ui::shell::unavailable(ui, part, icon, tint, false);
        toggled(ui, part, status);
        tip(ui, id);
        return None;
    }
    let clicked = ui::shell::tool_button(ui, part, icon, tint, status.checked).clicked;
    tip(ui, id);
    clicked.then_some(commands::Choice::Command(id))
}

/// Keeps a faded toggle `part` exposed as one, whether or not it is on.
fn toggled(ui: &mut Ui, part: impl std::hash::Hash, status: commands::Status) {
    if let Some(checked) = status.checked
        && let Some(node) = ui.access(ui.id(part))
    {
        node.set_toggled(checked.into());
    }
}

/// A tool button applying the tag at `place` in the tag list, as `tool` builds a command's.
fn tag_tool(
    ui: &mut Ui,
    place: usize,
    tag: &canvas::editor::NoteTag,
    status: commands::Status,
) -> Option<commands::Choice> {
    let part = ("tag", place);
    // Artwork in its own colours takes white.
    let tint = [1.0; 4];
    if !status.enabled {
        ui::shell::unavailable(ui, part, tag_art(tag), tint, false);
        toggled(ui, part, status);
        tag_tip(ui, place, tag);
        return None;
    }
    let clicked = ui::shell::tool_button(ui, part, tag_art(tag), tint, status.checked).clicked;
    tag_tip(ui, place, tag);
    clicked.then_some(commands::Choice::Command(commands::Id::Tag(place)))
}

/// The artwork the toolbar, the menus and the palette show for command `id`; none for a
/// tag, which shows its own.
fn artwork(id: commands::Id) -> Option<&'static [&'static str]> {
    use canvas::editor::{Alignment, Toggle};
    use commands::Id as Cmd;
    use recording::Transport;
    Some(match id {
        Cmd::Back => art::BACK,
        Cmd::Forward => art::FORWARD,
        Cmd::Undo => art::UNDO,
        Cmd::Redo => art::REDO,
        Cmd::Cut => art::CUT,
        Cmd::Copy => art::COPY,
        Cmd::Paste => art::PASTE,
        Cmd::FormatPainter => art::FORMAT_PAINTER,
        Cmd::Toggle(Toggle::Bold) => art::BOLD,
        Cmd::Toggle(Toggle::Italic) => art::ITALIC,
        Cmd::Toggle(Toggle::Underline) => art::UNDERLINE,
        Cmd::Toggle(Toggle::Strikethrough) => art::STRIKETHROUGH,
        Cmd::Toggle(Toggle::Subscript) => art::SUBSCRIPT,
        Cmd::Toggle(Toggle::Superscript) => art::SUPERSCRIPT,
        Cmd::ClearFormatting => art::CLEAR_FORMATTING,
        Cmd::Styles => art::STYLES,
        Cmd::Highlight => art::HIGHLIGHTER,
        Cmd::FontColor => art::FONT_COLOR,
        Cmd::Bullets => art::BULLETS,
        Cmd::Numbering => art::NUMBERING,
        Cmd::Align(Alignment::Left) => art::ALIGN_LEFT,
        Cmd::Align(Alignment::Center) => art::ALIGN_CENTER,
        Cmd::Align(Alignment::Right) => art::ALIGN_RIGHT,
        Cmd::Outdent => art::OUTDENT,
        Cmd::Indent => art::INDENT,
        Cmd::FindTags => art::FIND_TAGS,
        Cmd::Table => art::TABLE,
        Cmd::Picture => art::PICTURE,
        Cmd::ScreenClipping => art::SCREEN_CLIPPING,
        Cmd::Attachment => art::ATTACHMENT,
        Cmd::Link => art::LINK,
        Cmd::InsertSpace => art::INSERT_SPACE,
        Cmd::Date => art::CALENDAR,
        Cmd::Time => art::CLOCK,
        Cmd::DateTime => art::DATE_TIME,
        Cmd::Equation => art::EQUATION,
        Cmd::Symbol => art::SYMBOL,
        Cmd::RecordAudio => art::RECORD_AUDIO,
        Cmd::RecordVideo => art::RECORD_VIDEO,
        Cmd::SelectType => art::SELECT,
        Cmd::Pen => art::PEN,
        Cmd::Eraser => art::ERASER,
        Cmd::Lasso => art::LASSO,
        Cmd::Shape(ShapeKind::Line) => art::SHAPE_LINE,
        Cmd::Shape(ShapeKind::Arrow) => art::SHAPE_ARROW,
        Cmd::Shape(ShapeKind::Rectangle) => art::SHAPE_RECTANGLE,
        Cmd::Shape(ShapeKind::Ellipse) => art::SHAPE_OVAL,
        Cmd::PageColor => art::PAGE_COLOR,
        Cmd::ZoomIn => art::ZOOM_IN,
        Cmd::ZoomOut => art::ZOOM_OUT,
        Cmd::HideSpelling | Cmd::Spelling => art::SPELLING,
        Cmd::Settings => art::OPTIONS,
        Cmd::NewNotebook => art::NEW_NOTEBOOK,
        Cmd::OpenNotebook => art::OPEN,
        Cmd::OpenFromServer => art::SERVER,
        Cmd::CloseNotebook => art::CLOSE_NOTEBOOK,
        Cmd::NewSection => art::NEW_SECTION,
        Cmd::NewSectionGroup => art::NEW_SECTION_GROUP,
        Cmd::NewPage => art::NEW_PAGE,
        Cmd::NewSubpage => art::NEW_SUBPAGE,
        Cmd::PageVersions => art::PAGE_VERSIONS,
        Cmd::CopyPageLink => art::COPY_LINK,
        Cmd::PasswordProtect => art::PASSWORD,
        Cmd::LockAll => art::LOCK,
        Cmd::ShowNotebook => art::FOLDER,
        Cmd::SaveAs => art::SAVE,
        Cmd::RecycleBin => art::RECYCLE_BIN,
        Cmd::EmptyRecycleBin => art::EMPTY_RECYCLE_BIN,
        Cmd::MarkRead => art::MARK_READ,
        Cmd::MarkNotebookRead => art::MARK_NOTEBOOK_READ,
        Cmd::ShowUnread => art::SHOW_UNREAD,
        Cmd::NextUnread => art::NEXT_UNREAD,
        Cmd::ExportPdf => art::EXPORT_PDF,
        Cmd::Print => art::PRINT,
        Cmd::SelectAll => art::SELECT_ALL,
        Cmd::Find => art::FIND,
        Cmd::Search => art::SEARCH,
        Cmd::SearchResults => art::SEARCH_RESULTS,
        Cmd::GoTo => art::GO_TO,
        Cmd::CommandPalette => art::COMMAND_PALETTE,
        Cmd::ActualSize => art::ZOOM_ACTUAL,
        Cmd::Sidebar => art::SIDEBAR_EXPAND,
        Cmd::PageList => art::PAGE_LIST,
        Cmd::PagesMatchTheme => art::PAGES_MATCH_THEME,
        Cmd::FullPageView => art::FULL_PAGE,
        Cmd::Transport(Transport::Pause) => art::PAUSE,
        Cmd::Transport(Transport::Stop) => art::STOP,
        Cmd::Transport(Transport::Skip(seconds)) if seconds < 0 => art::REWIND,
        Cmd::Transport(Transport::Skip(_)) => art::FAST_FORWARD,
        Cmd::Transport(Transport::SeekTo) => art::SEEK,
        Cmd::Transport(Transport::SeePlayback) => art::SEE_PLAYBACK,
        Cmd::SnapToGrid => art::SNAP_TO_GRID,
        Cmd::Style(level @ 0..6) => art::HEADINGS[level],
        Cmd::Style(6) => art::PAGE_TITLE,
        Cmd::Style(7) => art::CITATION,
        Cmd::Style(8) => art::QUOTE,
        Cmd::Style(9) => art::CODE,
        Cmd::Style(_) => art::NORMAL,
        Cmd::Themes => art::STYLES,
        Cmd::OpenShared => art::OPEN_SHARED,
        Cmd::LiveShare => art::LIVE_SHARE,
        Cmd::MarkdownShortcuts => art::MARKDOWN,
        Cmd::CustomizeTags => art::CUSTOMIZE_TAGS,
        Cmd::RemoveTags => art::REMOVE_TAG,
        Cmd::ToDoList => art::TO_DO,
        Cmd::BulletedList => art::BULLETS,
        Cmd::Help => art::HELP,
        Cmd::CheckForUpdates => art::UPDATE,
        Cmd::Tag(_) => return None,
    })
}

/// The artwork the toolbar shows for `tag`: its symbol, or for a tag that only colours
/// text, a chip of lines in its font colour on its highlight.
fn tag_art(tag: &canvas::editor::NoteTag) -> &'static [&'static str] {
    use std::collections::BTreeMap;
    use std::sync::Mutex;
    // Artwork is borrowed for the program's life, so each colour pair is made once.
    type Chips = BTreeMap<(Option<u32>, Option<u32>), &'static [&'static str]>;
    static CHIPS: Mutex<Chips> = Mutex::new(BTreeMap::new());
    if let Some(art) = tags::artwork(tag) {
        return art;
    }
    let hex = |colorref: u32| {
        let [red, green, blue, _] = colorref.to_le_bytes();
        format!("#{red:02x}{green:02x}{blue:02x}")
    };
    let mut chips = CHIPS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    chips.entry((tag.color, tag.highlight)).or_insert_with(|| {
        let ink = tag.color.map_or_else(|| "#000000".into(), hex);
        let fill = tag.highlight.map_or_else(|| "#ffffff".into(), hex);
        let svg = format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16">
  <path d="M2 3H14A1 1 0 0 1 15 4V12A1 1 0 0 1 14 13H2A1 1 0 0 1 1 12V4A1 1 0 0 1 2 3Z" fill="#000000" fill-opacity="0.35"/>
  <path d="M2.25 4H13.75A0.25 0.25 0 0 1 14 4.25V11.75A0.25 0.25 0 0 1 13.75 12H2.25A0.25 0.25 0 0 1 2 11.75V4.25A0.25 0.25 0 0 1 2.25 4Z" fill="{fill}"/>
  <path d="M3.5 6H12.5V7.25H3.5ZM3.5 8.75H10V10H3.5Z" fill="{ink}"/>
</svg>"##
        );
        Box::leak(Box::new([&*svg.leak()]))
    })
}

/// The toolbar's popup `name`, which the menu bar opens too.
fn toolbar_popup(name: &str) -> Id {
    Id::ROOT.child(("popup", name))
}

/// A tooltip naming the tag at `place` and its chord on the box built last.
fn tag_tip(ui: &mut Ui, place: usize, tag: &canvas::editor::NoteTag) {
    let key = commands::shortcut(commands::Id::Tag(place));
    ui::popup::tooltip(ui, &tag.label, &key, None);
}

/// A tooltip naming command `id` and its chord on the box built last.
fn tip(ui: &mut Ui, id: commands::Id) {
    ui::popup::tooltip(
        ui,
        commands::command(id).title,
        &commands::shortcut(id),
        None,
    );
}

/// Whether each toolbar group stands on a face of its own.
fn grouped(theme: &Theme) -> bool {
    theme.tool[3] > 0.0 && !theme.tool_panel
}

/// The theme's face for tools where `shown`, padded beside its buttons and standing `rise`
/// past the box's top and bottom.
fn tool_face(theme: &Theme, shown: bool, rise: f32) -> Spec<'static> {
    if !shown {
        return Spec::default();
    }
    Spec {
        fill: Some(theme.tool),
        border: Some(theme.chip),
        radius: 4.0,
        inset: [0.0, -rise, 0.0, -rise],
        pad: [FACE_PAD, 0.0],
        ..Spec::default()
    }
}

/// The line leading a toolbar group, as far from its buttons as the groups are apart; groups
/// on faces of their own stand apart without one.
fn divider(ui: &mut Ui, theme: &Theme) {
    if grouped(theme) {
        return;
    }
    ui.leaf(
        "divider",
        Spec {
            size: [px(GAP), px(ui::shell::TOOL)],
            fill: Some(theme.chip),
            inset: [0.0, 0.0, GAP - 1.0, 0.0],
            ..Spec::default()
        },
    );
}

/// The kind of the file or network failure behind `error`, if one is.
pub fn io_kind(error: &(dyn Error + 'static)) -> Option<std::io::ErrorKind> {
    let mut next = Some(error);
    while let Some(error) = next {
        if let Some(error) = error.downcast_ref::<std::io::Error>() {
            return Some(error.kind());
        }
        // Transparent variants forward `source` past the I/O error they hold.
        if let Some(notebook::Error::Io(error) | notebook::Error::RemoteIo(error)) =
            error.downcast_ref()
        {
            return Some(error.kind());
        }
        next = error.source();
    }
    None
}

/// `error` as an alert tells it about `thing`: a file or network failure in plain words,
/// never the system's; any other error as it reads.
pub fn plain(error: &(dyn Error + 'static), thing: &str) -> String {
    use std::io::ErrorKind::*;
    match io_kind(error) {
        None | Some(Other) => error.to_string(),
        Some(NotFound) => format!("This {thing} was moved or deleted."),
        Some(PermissionDenied | ReadOnlyFilesystem) => {
            format!("You don't have permission to use this {thing}.")
        }
        Some(StorageFull | QuotaExceeded) => {
            "The disk is full. Free up space, then try again.".into()
        }
        Some(WouldBlock | ResourceBusy) => {
            format!("This {thing} is in use. Try again in a moment.")
        }
        Some(AlreadyExists) => {
            "Something with that name is already there. Choose another name.".into()
        }
        Some(
            TimedOut | ConnectionRefused | ConnectionReset | ConnectionAborted | NotConnected
            | HostUnreachable | NetworkUnreachable | NetworkDown | BrokenPipe,
        ) => "The server can't be reached. Check your connection, then try again.".into(),
        Some(_) => format!("Something went wrong with this {thing}. Try again."),
    }
}

/// The session for `section`, at catalog `path` in `library`, showing `space` or its first
/// page, and that page.
fn read_session(
    section: notebook::session::Section,
    library: Arc<Library>,
    path: String,
    space: Option<ExGuid>,
) -> Result<(Session, Page), Box<dyn Error>> {
    let folder = path.rsplit_once('/').map_or("", |(folder, _)| folder);
    let tabs = library.tabs(folder);
    let tab = tabs
        .iter()
        .position(|tab| tab.path == path)
        .ok_or("The section is not in its notebook")?;
    let pages = section.pages()?;
    let space = space
        .filter(|space| pages.iter().any(|(candidate, ..)| candidate == space))
        .or(pages.first().map(|(space, ..)| *space))
        .ok_or("The section has no pages")?;
    let page = section.page(space)?;
    let conflicts = section.conflicts()?;
    let history = section.versions()?;
    let sync = section.sync_status()?;
    Ok((
        Session {
            section,
            library,
            tabs,
            tab,
            pages,
            space,
            sync,
            conflicts,
            shown: None,
            change: None,
            history,
            shown_history: None,
            version: None,
        },
        page,
    ))
}

/// The text the editor shows, outline by outline, without hidden field codes.
fn page_text(editor: &CanvasEditor) -> String {
    editor
        .visible_outlines()
        .chain(editor.caret_outline())
        .map(|outline| {
            outline
                .document()
                .paragraphs()
                .map(|paragraph| {
                    let mut start = 0;
                    paragraph
                        .spans()
                        .iter()
                        .filter_map(|span| {
                            let run = &paragraph.text()[start..span.end];
                            start = span.end;
                            (span.format.hidden != Some(true)).then_some(run)
                        })
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .filter(|text| !text.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Runs `work` beside the frame: on a thread of its own, or in the browser, which gives the
/// page one thread, once the frame under way is done.
/// Work [`spawn`] started that has not ended.
static RUNNING: AtomicUsize = AtomicUsize::new(0);

fn spawn(work: impl FnOnce() + Send + 'static) {
    RUNNING.fetch_add(1, Ordering::Relaxed);
    let work = move || {
        work();
        // Whatever `work` sent is in the queue before the count shows it done.
        RUNNING.fetch_sub(1, Ordering::Release);
    };
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::spawn(work);
    #[cfg(target_arch = "wasm32")]
    platform::defer(work);
}

/// Runs asynchronous work beside the frame, creating the future on its executor.
#[cfg(feature = "live")]
fn spawn_async<F: Future<Output = ()> + 'static>(work: impl FnOnce() -> F + Send + 'static) {
    RUNNING.fetch_add(1, Ordering::Relaxed);
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::spawn(move || {
        pollster::block_on(work());
        RUNNING.fetch_sub(1, Ordering::Release);
    });
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::spawn_local(async move {
        work().await;
        RUNNING.fetch_sub(1, Ordering::Release);
    });
}

/// FILETIME now: when an edit happened, which its modification times record.
fn filetime() -> u64 {
    let unix = web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .unwrap_or_default();
    (unix.as_secs() + 11_644_473_600) * 10_000_000 + u64::from(unix.subsec_nanos() / 100)
}

impl State {
    /// Does what an event sent to the event loop asks of the open window.
    fn user_event(&mut self, event: UserEvent) {
        match event {
            // The event loop quits; a page stays open.
            UserEvent::Quit | UserEvent::Replay(Replay::Quit) => {}
            #[cfg(not(target_arch = "wasm32"))]
            UserEvent::Exit => {}
            #[cfg(target_os = "linux")]
            UserEvent::Clipboard => {
                if let Clipboard::System(clipboard) = &mut self.clipboard {
                    clipboard.serve();
                }
            }
            UserEvent::Picture(bytes) => {
                if let Err(error) = self.insert_picture(bytes, None) {
                    eprintln!("{error}");
                }
                self.window.request_redraw();
            }
            UserEvent::InsertText(text) => {
                if self.ui.focused() == Some(page()) {
                    match self.view.insert_text(text) {
                        Ok(response) => self.respond(response),
                        Err(error) => eprintln!("{error}"),
                    }
                    self.window.request_redraw();
                } else {
                    self.input(ui::Event::Ime(Ime::Commit(text)));
                }
            }
            UserEvent::Then(then) => {
                if let Err(error) = then(self) {
                    eprintln!("{error}");
                }
                self.window.request_redraw();
            }
            UserEvent::ICloudFolder => self.list_icloud(),
            UserEvent::ICloudAccount => {
                self.icloud_account_changed();
                self.window.request_redraw();
            }
            UserEvent::Open(paths) => {
                for path in paths {
                    self.open_path(&path);
                }
                self.window.set_minimized(false);
                self.window.focus_window();
                self.window.request_redraw();
            }
            #[cfg(target_os = "linux")]
            UserEvent::Activate(token) => desktop::activate(&self.window, &token),
            UserEvent::Update => {
                self.updated();
                self.window.request_redraw();
            }
            UserEvent::Sync => {
                if let Err(error) = self.synced() {
                    eprintln!("{error}");
                }
            }
            UserEvent::Choose(choice) => self.choose(choice),
            UserEvent::Appearance => {
                self.follow_color_scheme();
                self.window.request_redraw();
            }
            UserEvent::Redraw => self.window.request_redraw(),
            #[cfg(target_arch = "wasm32")]
            UserEvent::Renderer(chosen) => platform::switch_renderer(self, chosen),
            #[cfg(not(target_arch = "wasm32"))]
            UserEvent::Renderer(_) => unreachable!("The app switches renderers"),
            UserEvent::Replay(replay) => {
                let mark = match replay {
                    Replay::Mark(at) => Some(at),
                    _ => None,
                };
                match replay {
                    Replay::Input(event) => self.input(event),
                    Replay::Pinch(factor) => {
                        if let Err(error) = self.pinch(factor) {
                            eprintln!("{error}");
                        }
                    }
                    Replay::Snapshot(path) => self.snapshot = Some(path),
                    Replay::Settle(path, settled) => {
                        self.replay_settle = Some((path, settled, None));
                    }
                    Replay::Tick | Replay::Mark(_) | Replay::Quit => {}
                    Replay::Appearance(appearance) => {
                        self.window.set_theme(Some(appearance));
                        self.set_appearance(appearance);
                    }
                    Replay::Resize([width, height]) => {
                        let _ = self
                            .window
                            .request_inner_size(LogicalSize::new(width, height));
                    }
                    Replay::Renderer(_) => unreachable!("The app switches renderers"),
                }
                // A covered window gets no redraws, so each step draws its own frame.
                if let Err(error) = self.frame() {
                    eprintln!("{error}");
                }
                // Quiet still when the mark sent on finding it quiet arrives, what a thread sent
                // as it ended has been taken, however many frames wait in the queue before it.
                let quiet = self.settled()
                    && self.opening.is_none()
                    && RUNNING.load(Ordering::Acquire) == 0
                    && self.search.pending.load(Ordering::Relaxed) == 0;
                if let Some((path, settled, sent)) = self.replay_settle.take() {
                    match (quiet, sent) {
                        (true, Some(sent)) if mark == Some(sent) => {
                            if let Some(path) = path
                                && let Err(error) = self.write_accessibility(&path)
                            {
                                eprintln!("{error}");
                            }
                            let _ = settled.send(());
                        }
                        (true, None) => {
                            let now = Instant::now();
                            let _ = self.proxy.send_event(UserEvent::Replay(Replay::Mark(now)));
                            self.replay_settle = Some((path, settled, Some(now)));
                        }
                        (true, sent) => self.replay_settle = Some((path, settled, sent)),
                        (false, _) => self.replay_settle = Some((path, settled, None)),
                    }
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            UserEvent::Accessibility(event) => self.access_event(event),
        }
    }

    /// Does what assistive technology asked of the window.
    #[cfg(not(target_arch = "wasm32"))]
    fn access_event(&mut self, event: accesskit_winit::Event) {
        if event.window_id != self.window.id() {
            return;
        }
        let was_marked = self.view.editor.marked_range().is_some();
        let result = match event.window_event {
            accesskit_winit::WindowEvent::InitialTreeRequested => {
                self.deactivate_accessibility();
                self.update_accessibility(true)
            }
            accesskit_winit::WindowEvent::ActionRequested(request) => self.access_action(request),
            accesskit_winit::WindowEvent::AccessibilityDeactivated => {
                self.deactivate_accessibility();
                Ok(())
            }
        };
        if was_marked && self.view.editor.marked_range().is_none() {
            platform::clear_marked_text(&self.window);
        }
        if let Err(error) = result {
            eprintln!("{error}");
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl App {
    fn close(&self, event_loop: &ActiveEventLoop) {
        let Some(state) = self.state.as_ref().filter(|state| {
            let editor = &state.view.editor;
            let unchanged = editor.caret_outline().is_none_or(TextOutline::is_empty)
                && state.initial_date == editor.date().map(|date| date.timestamp())
                && state
                    .initial_layouts
                    .iter()
                    .map(|(id, layout)| (*id, layout))
                    .eq(editor.object_layouts())
                && state
                    .initial
                    .iter()
                    .map(|(id, document)| (id, document))
                    .eq(editor
                        .outlines()
                        .iter()
                        .map(|outline| (&outline.id, outline.document())));
            state.session.is_none() && !unchanged
        }) else {
            return self.exit(event_loop);
        };
        crate::confirm(
            "Discard this page?",
            "This temporary page has no saved copy. Closing it will discard your edits.",
            "Keep Editing",
            "Discard Changes",
            state.reply(|state, ()| {
                let _ = state.proxy.send_event(UserEvent::Exit);
                Ok(())
            }),
        );
    }

    fn exit(&self, event_loop: &ActiveEventLoop) {
        if let Some(state) = &self.state {
            state.publish_now(QUIT_PUBLISH);
        }
        event_loop.exit();
    }
}

/// How long quitting waits for the open section's edits to reach its file; they stay in the
/// replica and publish at the next launch otherwise.
#[cfg(not(target_arch = "wasm32"))]
const QUIT_PUBLISH: std::time::Duration = std::time::Duration::from_secs(3);

#[cfg(not(target_arch = "wasm32"))]
impl ApplicationHandler<UserEvent> for App {
    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        let Some(state) = &mut self.state else {
            match event {
                UserEvent::Quit => self.close(event_loop),
                // A cold launch's documents arrive before the window opens.
                UserEvent::Open(paths) => self.opening.extend(paths),
                _ => {}
            }
            return;
        };
        match event {
            UserEvent::Quit | UserEvent::Replay(Replay::Quit) => self.close(event_loop),
            UserEvent::Exit => self.exit(event_loop),
            UserEvent::Renderer(chosen) | UserEvent::Replay(Replay::Renderer(chosen)) => {
                let state = self.state.take().expect("The window is open");
                match state.switch_renderer(chosen) {
                    Ok(state) => self.state = Some(state),
                    Err(error) => {
                        self.startup_error = Some(error);
                        event_loop.exit();
                    }
                }
            }
            event => state.user_event(event),
        }
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        let mut input = self.input.take().unwrap();
        // The window starts at the notebook asked for rather than switching to it.
        for path in &self.opening {
            if let library::Located::Notebook { root, .. } = library::locate(path)
                && let Err(error) = input.show(&root)
            {
                eprintln!("Cannot open {}: {error}", root.display());
            }
        }
        match pollster::block_on(State::new(
            event_loop,
            self.proxy.clone(),
            input,
            &self.substitutes,
            self.screenshot.is_none(),
            self.launch.take().expect("The window opens once"),
        )) {
            Ok(mut state) => match &self.screenshot {
                // A replay drives the hidden window instead, its snapshots capturing it.
                Some(prefix) if std::env::var_os("SNOWBOUND_REPLAY").is_none() => {
                    self.startup_error = state.screenshot(prefix).err();
                    event_loop.exit();
                }
                _ => {
                    for path in self.opening.drain(..) {
                        state.open_path(&path);
                    }
                    state.offer_crash_report();
                    if cfg!(debug_assertions) && std::env::var_os("SNOWBOUND_CRASH").is_some() {
                        let open = state.session.as_ref().map(|session| &session.library);
                        panic!(
                            "SNOWBOUND_CRASH asked to crash with {:?} open",
                            open.map(|library| &library.location)
                        );
                    }
                    self.state = Some(state);
                }
            },
            Err(error) => {
                self.startup_error = Some(error);
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        if matches!(
            event,
            WindowEvent::KeyboardInput { .. }
                | WindowEvent::Ime(_)
                | WindowEvent::MouseInput { .. }
                | WindowEvent::CursorMoved { .. }
                | WindowEvent::ModifiersChanged(_)
                | WindowEvent::Focused(_)
        ) {
            trace_input(&event);
        }
        if let Some(state) = &mut self.state {
            state.access_adapter.process_event(&state.window, &event);
        }
        if matches!(event, WindowEvent::CloseRequested) {
            if let Some(state) = &mut self.state
                && let Err(error) = state.stop_recording(true).and_then(|()| state.persist())
            {
                eprintln!("{error}");
            }
            self.close(event_loop);
            return;
        }
        let Some(state) = &mut self.state else {
            return;
        };
        let scale = state.window.scale_factor() as f32;
        let result = (|| -> Result<(), Box<dyn Error>> {
            match event {
                WindowEvent::RedrawRequested => return state.frame(),
                WindowEvent::Resized(size) => {
                    if size.width > 0 && size.height > 0 {
                        let ratio = state.scale() / scale;
                        state.surface.size =
                            [size.width, size.height].map(|side| (side as f32 * ratio) as u32);
                        state.surface.configure(&state.renderer);
                    }
                    // Present inside AppKit's resize transaction; a redraw on the next turn
                    // lets the window show the previous frame at the new size.
                    return state.frame();
                }
                WindowEvent::ThemeChanged(appearance) => {
                    state.set_appearance(state.color_scheme.theme().unwrap_or(appearance));
                    state.window.request_redraw();
                }
                WindowEvent::ScaleFactorChanged { .. } => {
                    state.renderer.clear_glyph_cache();
                    state.window.request_redraw();
                }
                WindowEvent::Focused(focused) => {
                    state.ui.window_focused = focused;
                    if !focused {
                        // Someone switching to another device finds their edits there.
                        state.publish_now(std::time::Duration::ZERO);
                    } else {
                        // Notebooks another device added or removed meanwhile.
                        state.list_icloud();
                        state.follow_renderer_setting();
                    }
                    state.window.request_redraw();
                }
                WindowEvent::Occluded(occluded) => {
                    trace_input(&("Window occluded", occluded));
                    state.occluded = occluded;
                    if !occluded {
                        state.moved = true;
                        state.window.request_redraw();
                    }
                }
                WindowEvent::ModifiersChanged(modifiers) => {
                    state.input(ui::Event::Modifiers(modifiers.state()))
                }
                WindowEvent::CursorLeft { .. } => state.input(ui::Event::PointerLeft),
                WindowEvent::CursorMoved { position, .. } => {
                    state.input(ui::Event::Pressure(platform::pen_pressure()));
                    state.input(ui::Event::PointerMoved([
                        position.x as f32 / scale,
                        position.y as f32 / scale,
                    ]))
                }
                WindowEvent::MouseInput {
                    state: pressed,
                    button,
                    ..
                } => {
                    state.input(ui::Event::Pressure(platform::pen_pressure()));
                    state.input(ui::Event::Button {
                        button,
                        pressed: pressed == ElementState::Pressed,
                        at: Instant::now(),
                    })
                }
                WindowEvent::MouseWheel { delta, phase, .. } => {
                    if let MouseScrollDelta::PixelDelta(p) = delta {
                        state.swipe_scroll(phase, [p.x as f32 / scale, p.y as f32 / scale]);
                    }
                    state.input(ui::Event::Wheel(match delta {
                        MouseScrollDelta::LineDelta(x, y) => [x * 32.0, y * 32.0],
                        MouseScrollDelta::PixelDelta(p) => [p.x as f32 / scale, p.y as f32 / scale],
                    }))
                }
                WindowEvent::KeyboardInput { event, .. }
                    if event.state == ElementState::Pressed =>
                {
                    state.input(ui::Event::Key {
                        key: event.logical_key,
                        text: event.text.map(|text| text.to_string()),
                    })
                }
                WindowEvent::Ime(ime) => state.input(ui::Event::Ime(ime)),
                WindowEvent::DroppedFile(path) => {
                    let at = platform::drop_point(&state.window);
                    state.import_file(&path, at, true);
                    state.window.request_redraw();
                }
                WindowEvent::PinchGesture { delta, .. } => state.pinch(1.0 + delta as f32)?,
                // Smart zoom: to 200% about the fingers, or back to 100%.
                WindowEvent::DoubleTapGesture { .. } => {
                    let zoom = state.view.zoom();
                    let target = if (zoom - 1.0).abs() < 0.01 { 2.0 } else { 1.0 };
                    state.pinch(target / zoom)?
                }
                _ => {}
            }
            Ok(())
        })();
        if let Err(error) = result {
            eprintln!("{error}");
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let Some(state) = &mut self.state else {
            return;
        };
        if state.occluded {
            event_loop.set_control_flow(ControlFlow::Wait);
            return;
        }
        let now = Instant::now();
        let (repaint, blink) = state.view.blink(now);
        // A due interface change waits on the frame that shows it, which sets the next.
        let wake = state.ui.wake_at();
        if repaint || wake.is_some_and(|wake| wake <= now) {
            state.window.request_redraw();
        }
        let next = [blink, wake.filter(|wake| *wake > now)]
            .into_iter()
            .flatten()
            .min();
        event_loop.set_control_flow(next.map_or(ControlFlow::Wait, ControlFlow::WaitUntil));
    }

    /// Drops the window's state while the event loop still holds the Wayland connection the
    /// clipboard and the surface use; `run_app` closes it on returning.
    fn exiting(&mut self, _: &ActiveEventLoop) {
        self.restart = self
            .state
            .take()
            .and_then(|state| state.updates.restarting());
    }
}

fn write_png(path: &Path, size: [u32; 2], pixels: &[u8]) -> Result<(), Box<dyn Error>> {
    let partial = path.with_extension("partial");
    notebook::fs::write(&partial, paste::png(size, pixels)?)?;
    notebook::fs::rename(partial, path)?;
    Ok(())
}

/// Feeds a development script to the window from another thread, one command per line
/// in logical pixels: `move X Y`, `press [right]`, `release [right]`, `wheel DX DY`,
/// `pressure LEVEL|none`, `pinch FACTOR`, `key NAME`, `type TEXT`, `modifiers [shift]
/// [control] [command]`, `wait MILLISECONDS`, `snapshot PNG_PATH`, `settle` and
/// `accessibility TEXT_PATH` (which wait for what is on its way), `appearance light|dark`,
/// `resize WIDTH HEIGHT` and `quit`.
#[cfg(not(target_arch = "wasm32"))]
fn replay(script: String, proxy: EventLoopProxy<UserEvent>) -> Result<(), Box<dyn Error>> {
    let (settle, settled) = std::sync::mpsc::channel();
    let mut steps = Vec::new();
    for line in script.lines().filter(|line| !line.trim().is_empty()) {
        let (command, rest) = line.split_once(' ').unwrap_or((line, ""));
        let numbers = || -> Result<Vec<f32>, std::num::ParseFloatError> {
            rest.split_whitespace().map(str::parse).collect()
        };
        let button = |pressed| ui::Event::Button {
            button: if rest == "right" {
                MouseButton::Right
            } else {
                MouseButton::Left
            },
            pressed,
            at: Instant::now(),
        };
        steps.push(match command {
            "move" => Ok(Replay::Input(ui::Event::PointerMoved(
                numbers()?.try_into().map_err(|_| "move takes X Y")?,
            ))),
            "wheel" => Ok(Replay::Input(ui::Event::Wheel(
                numbers()?.try_into().map_err(|_| "wheel takes DX DY")?,
            ))),
            "pressure" => Ok(Replay::Input(ui::Event::Pressure(match rest {
                "none" => None,
                level => Some(level.parse()?),
            }))),
            "pinch" => Ok(Replay::Pinch(rest.parse()?)),
            "press" => Ok(Replay::Input(button(true))),
            "release" => Ok(Replay::Input(button(false))),
            "key" => Ok(Replay::Input(ui::Event::Key {
                key: match rest {
                    "Escape" => Key::Named(NamedKey::Escape),
                    "Enter" => Key::Named(NamedKey::Enter),
                    "Backspace" => Key::Named(NamedKey::Backspace),
                    "Tab" => Key::Named(NamedKey::Tab),
                    "Space" => Key::Named(NamedKey::Space),
                    "F5" => Key::Named(NamedKey::F5),
                    "F6" => Key::Named(NamedKey::F6),
                    "F7" => Key::Named(NamedKey::F7),
                    "Left" => Key::Named(NamedKey::ArrowLeft),
                    "Right" => Key::Named(NamedKey::ArrowRight),
                    "Up" => Key::Named(NamedKey::ArrowUp),
                    "Down" => Key::Named(NamedKey::ArrowDown),
                    character => Key::Character(character.into()),
                },
                text: None,
            })),
            "type" => Ok(Replay::Input(ui::Event::Ime(Ime::Commit(rest.into())))),
            "modifiers" => Ok(Replay::Input(ui::Event::Modifiers(
                rest.split_whitespace()
                    .map(|name| match name {
                        "shift" => Ok(ModifiersState::SHIFT),
                        "control" => Ok(ModifiersState::CONTROL),
                        // The shortcut modifier, as ui::edit_modifiers reads it.
                        "command" if cfg!(target_os = "macos") => Ok(ModifiersState::SUPER),
                        "command" => Ok(ModifiersState::CONTROL),
                        _ => Err(format!("Unknown modifier {name}")),
                    })
                    .try_fold(ModifiersState::empty(), |all, one| one.map(|one| all | one))?,
            ))),
            "wait" => Err(std::time::Duration::from_millis(rest.parse()?)),
            "quit" => Ok(Replay::Quit),
            "resize" => Ok(Replay::Resize(
                numbers()?
                    .try_into()
                    .map_err(|_| "resize takes WIDTH HEIGHT")?,
            )),
            "snapshot" => Ok(Replay::Snapshot(rest.into())),
            "settle" => Ok(Replay::Settle(None, settle.clone())),
            "accessibility" => Ok(Replay::Settle(Some(rest.into()), settle.clone())),
            "renderer" => Ok(Replay::Renderer(settings::Backend::named(rest)?)),
            "appearance" => Ok(Replay::Appearance(match rest {
                "light" => winit::window::Theme::Light,
                "dark" => winit::window::Theme::Dark,
                _ => return Err(format!("Unknown appearance {rest}").into()),
            })),
            _ => return Err(format!("Unknown replay command: {line}").into()),
        });
    }
    std::thread::spawn(move || {
        for step in steps {
            match step {
                Ok(mut replay) => {
                    if let Replay::Input(ui::Event::Button { at, .. }) = &mut replay {
                        *at = Instant::now();
                    }
                    let settling = matches!(replay, Replay::Settle(..));
                    let _ = proxy.send_event(UserEvent::Replay(replay));
                    // Frames tick until settled; a minute bounds a page that never lands.
                    let start = Instant::now();
                    while settling
                        && start.elapsed().as_secs() < 60
                        && settled
                            .recv_timeout(std::time::Duration::from_micros(16_667))
                            .is_err()
                    {
                        let _ = proxy.send_event(UserEvent::Replay(Replay::Tick));
                    }
                }
                Err(duration) => {
                    // Ticks keep a 60 Hz display's pace, dropping those a slow frame missed.
                    let start = Instant::now();
                    let period = std::time::Duration::from_micros(16_667);
                    let mut tick = start;
                    while start.elapsed() < duration {
                        while tick <= Instant::now() {
                            tick += period;
                        }
                        std::thread::sleep(tick - Instant::now());
                        let _ = proxy.send_event(UserEvent::Replay(Replay::Tick));
                    }
                }
            }
        }
    });
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn main() -> Result<(), Box<dyn Error>> {
    #[cfg(target_os = "linux")]
    if std::env::args_os()
        .nth(1)
        .is_some_and(|arg| arg.as_encoded_bytes() == platform::SYMBOLIZE.to_bytes())
    {
        return platform::symbolize(std::env::args_os().skip(2));
    }
    #[cfg(target_os = "linux")]
    loader::preload();
    platform::with_pool(launch)
}

/// The page starts the window through `web::start` once it has fetched the fonts.
#[cfg(target_arch = "wasm32")]
fn main() {}

#[cfg(not(target_arch = "wasm32"))]
fn launch() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args_os().skip(1);
    if args.next().is_some_and(|arg| arg == update::FINISH) {
        return update::finish(args);
    }
    let mut args = std::env::args_os().skip(1);
    let mut positional = Vec::new();
    let mut substitutes = Vec::new();
    let mut reference = None;
    let mut editable = false;
    let mut section = None;
    let mut notebook = None;
    let mut cache = None;
    let mut settings_file = None;
    let mut screenshot = None;
    let mut renderer = None;
    let mut opening = Vec::new();
    while let Some(arg) = args.next() {
        if arg == "--substitute-font" {
            substitutes.push(PathBuf::from(
                args.next()
                    .ok_or("Provide a font file after --substitute-font.")?,
            ));
        } else if arg == "--notebook" {
            notebook = Some(PathBuf::from(
                args.next()
                    .ok_or("Provide a notebook folder after --notebook.")?,
            ));
        } else if arg == "--section" {
            if reference.is_some() || section.is_some() {
                return Err("Only one page can be opened.".into());
            }
            let file = PathBuf::from(
                args.next()
                    .ok_or("Provide a section file and page title after --section.")?,
            );
            let title = args
                .next()
                .ok_or("Provide a page title after the section file.")?
                .to_str()
                .ok_or("The page title must be valid Unicode.")?
                .to_owned();
            section = Some((file, title));
        } else if arg == "--cache" {
            cache = Some(PathBuf::from(
                args.next().ok_or("Provide a directory after --cache.")?,
            ));
        } else if arg == "--settings" {
            settings_file = Some(PathBuf::from(
                args.next()
                    .ok_or("Provide a settings file after --settings.")?,
            ));
        } else if arg == "--renderer" {
            let name = args.next().ok_or("Provide a renderer after --renderer.")?;
            renderer = Some(settings::Backend::named(&name.to_string_lossy())?);
        } else if arg == "--screenshot" {
            screenshot = Some(PathBuf::from(
                args.next()
                    .ok_or("Provide a PNG path prefix after --screenshot.")?,
            ));
        } else if arg == "--reference" || arg == "--page" {
            if reference.is_some() {
                return Err("Only one page can be opened.".into());
            }
            editable = arg == "--page";
            let path = args
                .next()
                .ok_or("Provide a section file and page title after the page option.")?;
            let title = args
                .next()
                .ok_or("Provide a page title after the section file.")?;
            let bytes = notebook::fs::read(path)?;
            let store = onestore::Store::parse(&bytes)?;
            let index = onestore::RevisionIndex::parse(&store)?;
            reference = Some(Page::from_document(
                &onestore::document::Document::parse(&index)?,
                title
                    .to_str()
                    .ok_or("The page title must be valid Unicode.")?,
            )?);
        } else if arg.to_string_lossy().starts_with("snowbound:") {
            // A Live Share link the desktop opens with Snowbound.
            opening.push(PathBuf::from(arg));
        } else if let Some(file) = arg.to_str().and_then(|arg| arg.strip_prefix("file://")) {
            // A file as a desktop entry's %U passes it.
            opening.push(PathBuf::from(library::decode(file)));
        } else if library::locate(Path::new(&arg)) != library::Located::Nothing {
            // A file the desktop opens with Snowbound, as a double-clicked section.
            opening.push(std::path::absolute(arg)?);
        } else {
            positional.push(arg);
        }
    }
    let renderer = match (renderer, std::env::var("SNOWBOUND_RENDERER")) {
        (Some(renderer), _) => Some(renderer),
        (None, Ok(name)) => Some(settings::Backend::named(&name)?),
        (None, Err(_)) => None,
    };
    AUTOMATED.store(
        screenshot.is_some() || std::env::var_os("SNOWBOUND_REPLAY").is_some(),
        Ordering::Relaxed,
    );
    let stored = notebook.is_some() || section.is_some();
    if (editable || stored) && !positional.is_empty()
        || notebook.is_some() && (section.is_some() || reference.is_some())
    {
        return Err(
            "Use one of --notebook, --page or --section, without a text file or width.".into(),
        );
    }
    if positional.len() > 2 {
        return Err(
            "Usage: snowbound [NOTEBOOK_FOLDER | SECTION.one | TOC.onetoc2]... [TEXT_FILE] [WIDTH_POINTS] [--reference SECTION PAGE_TITLE | --page SECTION PAGE_TITLE | --section SECTION PAGE_TITLE | --notebook FOLDER] [--cache DIR] [--settings FILE] [--renderer NAME] [--screenshot PNG_PREFIX] [--substitute-font FONT_FILE]..."
                .into(),
        );
    }
    let text = positional
        .first()
        .map(notebook::fs::read_to_string)
        .transpose()?
        .unwrap_or_default();
    let width = positional
        .get(1)
        .map(|value| value.to_string_lossy().parse::<f32>())
        .transpose()?
        .unwrap_or(if reference.is_some() {
            DEFAULT_OUTLINE_WIDTH
        } else {
            480.0
        });
    let cache = match cache {
        Some(cache) => cache,
        None => platform::cache_dir().ok_or("HOME is not set.")?,
    };
    let settings_file = settings_file.or_else(|| settings::default_path().filter(|_| !automated()));
    let saved = settings_file
        .as_deref()
        .map(settings::Settings::load)
        .unwrap_or_default();
    let input = if let Some((file, title)) = section {
        Input::Section { file, title }
    } else if editable {
        Input::Page(reference.unwrap())
    } else if positional.is_empty() && reference.is_none() {
        let mut input = Input::Notebooks {
            locations: saved.notebooks.clone(),
            current: saved.current.clone(),
        };
        if let Some(root) = notebook {
            input.show(&root)?;
        }
        input
    } else {
        Input::Notes {
            document: TextDocument::new(
                text.split('\n')
                    .map(|line| Paragraph::new(line.to_owned(), Format::default()))
                    .collect(),
            )?,
            width,
            reference,
        }
    };
    #[cfg(not(target_os = "macos"))]
    let instance = match (&input, &screenshot) {
        (Input::Notebooks { .. }, None) => match instance::claim(&cache, &opening) {
            Some(instance) => Some(instance),
            None => return Ok(()),
        },
        _ => None,
    };
    if screenshot.is_some() {
        screenshot::prepare();
    }
    let event_loop = platform::event_loop(screenshot.is_some())?;
    #[cfg(not(target_os = "macos"))]
    if let Some(instance) = instance {
        instance.serve(event_loop.create_proxy());
    }
    if let Some(script) = std::env::var_os("SNOWBOUND_REPLAY") {
        replay(
            notebook::fs::read_to_string(script)?,
            event_loop.create_proxy(),
        )?;
    }
    // A screenshot leaves the settings as it found them.
    let settings_file = settings_file.filter(|_| screenshot.is_none());
    if let Some(file) = &settings_file
        && let Err(error) = crash::set_path(file.with_file_name("crash.txt"))
    {
        eprintln!("Cannot keep crash reports: {error}");
    }
    let mut app = App {
        proxy: event_loop.create_proxy(),
        input: Some(input),
        launch: Some(settings::Launch {
            file: settings_file,
            saved,
            renderer,
            cache,
        }),
        opening,
        substitutes,
        screenshot,
        state: None,
        restart: None,
        startup_error: None,
    };
    event_loop.run_app(&mut app)?;
    if let Some(staged) = &app.restart {
        update::relaunch(staged)?;
    }
    app.startup_error.map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use canvas::document::TextPosition;

    /// Alerts tell a file or network failure in plain words, however deep it is wrapped,
    /// and pass the app's own messages on.
    #[test]
    fn alerts_never_show_the_systems_words() {
        use std::io::{Error as Io, ErrorKind};
        let missing: Box<dyn Error> = Box::new(Io::from(ErrorKind::NotFound));
        assert_eq!(plain(&*missing, "page"), "This page was moved or deleted.");
        let wrapped: Box<dyn Error> = Box::new(notebook::Error::Io(Io::from_raw_os_error(2)));
        assert_eq!(plain(&*wrapped, "page"), "This page was moved or deleted.");
        let discovered = notebook::discover::Error::Io {
            path: "/a".into(),
            error: Io::from(ErrorKind::TimedOut),
        };
        assert!(plain(&discovered, "notebook").starts_with("The server can't be reached."));
        let refused: Box<dyn Error> = "The section is password protected".into();
        assert_eq!(
            plain(&*refused, "page"),
            "The section is password protected"
        );
    }

    /// A notebook opened from the desktop is listed once and shown first, whether it was
    /// listed already or not.
    #[test]
    fn an_opened_notebook_is_listed_once_and_shown() {
        let root = std::env::temp_dir();
        let [listed, opened] = ["Listed", "Opened"].map(|name| root.join(name));
        let location = |path: &Path| path.to_string_lossy().into_owned();
        let mut input = Input::Notebooks {
            locations: vec![location(&listed)],
            current: None,
        };
        input.show(&opened).unwrap();
        input.show(&listed).unwrap();
        let Input::Notebooks { locations, current } = input else {
            unreachable!()
        };
        assert_eq!(locations, [location(&listed), location(&opened)]);
        assert_eq!(current, Some(location(&listed)));
    }

    /// The ops an editing session records reach the section through `apply` and read back
    /// as the editor's page.
    #[test]
    fn recorded_ops_reach_the_section() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let directory = std::env::temp_dir().join(format!("snowbound-ops-{}", std::process::id()));
        let _ = notebook::fs::remove_dir_all(&directory);
        notebook::fs::create_dir_all(directory.join("cache")).unwrap();
        let file = directory.join("section.one");
        notebook::fs::copy(
            root.join("corpus/paragraph-edit/before/notebook/synthetic.one"),
            &file,
        )
        .unwrap();
        let section =
            notebook::session::Section::open(&file, directory.join("cache"), || {}).unwrap();
        let mut engine = TextEngine::default();
        for (space, ..) in section.pages().unwrap().into_iter().take(3) {
            let mut editor =
                CanvasEditor::from_page(section.page(space).unwrap(), &mut engine).unwrap();
            let outline = editor.outlines().iter().find(|o| !o.title).unwrap().id;
            editor.focus_outline(outline).unwrap();
            let at = |paragraph, offset| [TextPosition { paragraph, offset }; 2].into();
            editor.select(at(0, 0)).unwrap();
            editor.insert(&mut engine, "Typed ").unwrap();
            editor.enter(&mut engine, false).unwrap();
            editor.delete(&mut engine, true).unwrap();
            editor.undo(&mut engine).unwrap();
            let ops = editor.take_ops().unwrap();
            section
                .apply(
                    "Test",
                    onestore::op::Edit {
                        at: filetime(),
                        ops: ops
                            .into_iter()
                            .map(|op| onestore::op::Op::Page { space, op })
                            .collect(),
                    },
                )
                .unwrap();
            let (stored, model) = (section.page(space).unwrap(), editor.page().unwrap());
            assert_eq!(
                Page {
                    title: model.title.clone(),
                    ..stored
                },
                model
            );
        }
        assert!(
            !section
                .events()
                .iter()
                .any(|event| matches!(event, notebook::session::Event::Rejected { .. }))
        );
        section.close().unwrap();
        notebook::fs::remove_dir_all(&directory).unwrap();
    }

    use notebook::session::{Event, Section};

    /// Edits `editor` made, as one edit of `space`.
    fn edit(editor: &mut CanvasEditor, space: ExGuid) -> onestore::op::Edit {
        onestore::op::Edit {
            at: filetime(),
            ops: editor
                .take_ops()
                .unwrap()
                .into_iter()
                .map(|op| onestore::op::Op::Page { space, op })
                .collect(),
        }
    }

    /// Polls `section`, keeping what it reports in `seen`, until `done` holds of an event.
    fn wait(section: &Section, seen: &mut Vec<Event>, done: impl Fn(&Event) -> bool) {
        let deadline = Instant::now() + std::time::Duration::from_secs(120);
        loop {
            let events = section.events();
            let finished = events.iter().any(&done);
            seen.extend(events);
            if finished {
                return;
            }
            assert!(Instant::now() < deadline, "no such event: {seen:?}");
            section.wake();
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    fn published(event: &Event) -> bool {
        matches!(
            event,
            Event::Attempt {
                status: notebook::EditStatus::Published { .. },
                ..
            }
        )
    }

    /// Publishing this machine's edits never reports the page it edits as changed, which
    /// would reload the editor under the caret; another writer's edit does, and the editor
    /// takes it in place with the caret where it was.
    #[test]
    fn only_another_writer_changes_the_open_page() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let directory =
            std::env::temp_dir().join(format!("snowbound-changed-{}", std::process::id()));
        let _ = notebook::fs::remove_dir_all(&directory);
        notebook::fs::create_dir_all(&directory).unwrap();
        let file = directory.join("section.one");
        notebook::fs::copy(
            root.join("corpus/paragraph-edit/before/notebook/synthetic.one"),
            &file,
        )
        .unwrap();
        let ours = Section::open(&file, directory.join("ours"), || {}).unwrap();
        let space = ours.pages().unwrap()[0].0;
        let mut engine = TextEngine::default();
        let mut editor = CanvasEditor::from_page(ours.page(space).unwrap(), &mut engine).unwrap();
        let outline = editor.outlines().iter().find(|o| !o.title).unwrap().id;
        editor.focus_outline(outline).unwrap();
        let at = |paragraph, offset| [TextPosition { paragraph, offset }; 2].into();
        editor.select(at(0, 0)).unwrap();
        let mut seen = Vec::new();
        for text in ["One ", "two ", "three "] {
            editor.insert(&mut engine, text).unwrap();
            ours.apply("Ours", edit(&mut editor, space)).unwrap();
            wait(&ours, &mut seen, published);
        }
        editor.enter(&mut engine, false).unwrap();
        editor.delete(&mut engine, true).unwrap();
        ours.apply("Ours", edit(&mut editor, space)).unwrap();
        wait(&ours, &mut seen, published);
        for _ in 0..3 {
            ours.wake();
            std::thread::sleep(std::time::Duration::from_millis(500));
            seen.extend(ours.events());
        }
        assert!(
            !seen
                .iter()
                .any(|event| matches!(event, Event::Changed(spaces) if spaces.contains(&space))),
            "{seen:?}"
        );

        let theirs = Section::open(&file, directory.join("theirs"), || {}).unwrap();
        let mut other = CanvasEditor::from_page(theirs.page(space).unwrap(), &mut engine).unwrap();
        other.focus_outline(outline).unwrap();
        let last = other.active_outline().document().paragraphs().count() - 1;
        let end = other
            .active_outline()
            .document()
            .paragraphs()
            .last()
            .unwrap()
            .text()
            .encode_utf16()
            .count() as u32;
        other.select(at(last, end)).unwrap();
        other.insert(&mut engine, " and theirs").unwrap();
        theirs.apply("Theirs", edit(&mut other, space)).unwrap();
        wait(&theirs, &mut Vec::new(), published);
        wait(
            &ours,
            &mut seen,
            |event| matches!(event, Event::Changed(spaces) if spaces.contains(&space)),
        );

        let (shown, selection) = (editor.active_outline().id, editor.selection());
        editor
            .refresh(ours.page(space).unwrap(), &mut engine)
            .unwrap();
        assert_eq!(
            (editor.active_outline().id, editor.selection()),
            (shown, selection)
        );
        assert!(
            editor
                .active_outline()
                .document()
                .paragraphs()
                .any(|paragraph| paragraph.text().ends_with(" and theirs"))
        );
        ours.close().unwrap();
        theirs.close().unwrap();
        notebook::fs::remove_dir_all(&directory).unwrap();
    }

    /// Times opening each section of a notebook and each of its pages, phase by phase, as a
    /// switch does: `SNOWBOUND_SWITCH_NOTEBOOK=FOLDER`, or a share through `ONESTORE_SMB_LAB`
    /// (`HOST:PORT`, share `agent`) at `ONESTORE_SMB_LAB_ROOT` with `ONESTORE_SMB_LAB_USER`
    /// and `ONESTORE_SMB_LAB_PASSWORD`. The first pass starts without replicas.
    #[test]
    #[ignore = "measures a notebook named by the environment"]
    fn switch_timings() {
        let cache = std::env::temp_dir().join(format!("snowbound-switch-{}", std::process::id()));
        let _ = notebook::fs::remove_dir_all(&cache);
        let open = || match std::env::var("SNOWBOUND_SWITCH_NOTEBOOK") {
            Ok(folder) => Library::notebook(&folder, &cache),
            Err(_) => {
                let user = std::env::var("ONESTORE_SMB_LAB_USER").ok();
                let mount = library::Mount {
                    server: std::env::var("ONESTORE_SMB_LAB").unwrap(),
                    share: "agent".into(),
                    user: user.clone(),
                    domain: String::new(),
                    root: std::env::var("ONESTORE_SMB_LAB_ROOT").unwrap(),
                };
                let login = library::Login {
                    user: user.unwrap_or_default(),
                    password: std::env::var("ONESTORE_SMB_LAB_PASSWORD").unwrap_or_default(),
                    domain: String::new(),
                };
                Library::on_share(&mount.url(), mount, login, &cache).unwrap()
            }
        };
        let ms = |start: Instant| start.elapsed().as_secs_f64() * 1e3;
        let paper = canvas::gpu::Paper {
            color: [1.0; 4],
            ink: [0.0, 0.0, 0.0, 1.0],
        };
        let mut engine = TextEngine::default();
        for pass in ["cold", "warm"] {
            let start = Instant::now();
            let library = Arc::new(open());
            eprintln!("{pass}\tnotebook\t{:.1}", ms(start));
            for tab in library.tabs("") {
                let start = Instant::now();
                let section = library.open(&tab.path, || {}).unwrap();
                let opened = ms(start);
                let start = Instant::now();
                let (session, page) =
                    read_session(section, Arc::clone(&library), tab.path.clone(), None).unwrap();
                let read = ms(start);
                let start = Instant::now();
                let (mut scene, editor) = PageScene::from_page(page, &mut engine).unwrap();
                let laid = ms(start);
                let start = Instant::now();
                scene.settle(Some(&editor), 2.0, paper);
                eprintln!(
                    "{pass}\tsection {}\topen {opened:.1}\tread {read:.1}\tlayout {laid:.1}\tpictures {:.1}",
                    tab.name,
                    ms(start)
                );
                for (space, title, _) in session.pages.iter().skip(1) {
                    let start = Instant::now();
                    let page = session.section.page(*space).unwrap();
                    let read = ms(start);
                    let start = Instant::now();
                    let (mut scene, editor) = PageScene::from_page(page, &mut engine).unwrap();
                    let laid = ms(start);
                    let start = Instant::now();
                    scene.settle(Some(&editor), 2.0, paper);
                    eprintln!(
                        "{pass}\tpage {title}\tread {read:.1}\tlayout {laid:.1}\tpictures {:.1}",
                        ms(start)
                    );
                }
                session.section.close().unwrap();
            }
        }
        let _ = notebook::fs::remove_dir_all(&cache);
    }
}
