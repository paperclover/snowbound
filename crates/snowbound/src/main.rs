mod art;
mod commands;
#[cfg(test)]
mod conflict_render;
mod history;
mod library;
mod link;
mod manage;
mod meeting;
#[cfg(target_os = "macos")]
mod menubar;
mod menus;
mod navigation;
mod options;
#[cfg_attr(target_os = "linux", path = "linux.rs")]
#[cfg_attr(target_os = "macos", path = "macos.rs")]
mod platform;
mod rename;
mod screenshot;
mod search;
mod settings;
mod sidebar;
mod templates;

use canvas::gpu::page::PageScene;
use canvas::interaction::{Cursor, PageView, Place, Request, Response, TextColors, accessibility};
use canvas::{
    date::DateField,
    document::TextDocument,
    editor::{CanvasEditor, DEFAULT_OUTLINE_WIDTH, TextOutline},
    layout::TextEngine,
};
use canvas::{
    gpu::{colorref, tag_sources},
    outline::TagIcon,
};
use draw::Renderer;
use library::Library;
use onestore::ExGuid;
use onestore::document::Format;
use onestore::page::Page;
use onestore::page::text::Paragraph;
use std::{
    collections::{HashMap, HashSet},
    error::Error,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    time::Instant,
};
use ui::{Axis, Flags, Id, Spec, Theme, Ui, children, fill, fit, px};
use winit::{
    application::ApplicationHandler,
    dpi::{LogicalPosition, LogicalSize},
    event::{ElementState, Ime, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoopProxy},
    keyboard::{Key, ModifiersState, NamedKey},
    window::{CursorIcon, Window, WindowId},
};

/// Height of the title bar, which holds the window's controls.
const TITLE: f32 = 28.0;
const TAB_ROW: f32 = 28.0;
/// Width of the section colour around the page.
const FRAME: f32 = 6.0;
const PAGE_LIST: f32 = 240.0;
/// Font sizes the size box offers, OneNote's list in points.
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
/// Office's theme and standard font colours, COLORREF.
const FONT_COLORS: [u32; 20] = [
    0xffffff, 0x000000, 0xe1ecee, 0x7d491f, 0xbd814f, 0x4d50c0, 0x59bb9b, 0xa26480, 0xc6ac4b,
    0x4696f7, 0x0000c0, 0x0000ff, 0x00c0ff, 0x00ffff, 0x50d092, 0x50b000, 0xf0b000, 0xc07000,
    0x602000, 0xa03070,
];
/// Height of a page tab's row, whose tab leaves `ROW_GAP` below it so tabs stand apart
/// while the gaps still take the pointer.
const ROW: f32 = 29.0;
const ROW_GAP: f32 = 3.0;
/// Space between the page and a page tab that isn't open.
const PILL_MARGIN: f32 = 3.0;
/// Why the platform's date dialog could not change the page's date.
const DATE_UNCHOSEN: &str = "Choose another date or time.";
const DATE_OUT_OF_RANGE: &str = "This date is outside the notebook's supported range.";

#[derive(Debug)]
enum UserEvent {
    Quit,
    /// Text AppKit inserts outside key events, such as the character palette's.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    InsertText(String),
    Accessibility(accesskit_winit::Event),
    /// The section's synchronization thread reported an event.
    Sync,
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
}

/// Asks the event loop for a frame from any thread.
struct Redraw(EventLoopProxy<UserEvent>);

impl std::task::Wake for Redraw {
    fn wake(self: Arc<Self>) {
        let _ = self.0.send_event(UserEvent::Redraw);
    }
}

#[derive(Debug)]
enum Replay {
    Input(ui::Event),
    /// Paints the next frame into a PNG as well as the window.
    Snapshot(PathBuf),
    /// A frame during a wait, as a visible window's display would ask for.
    Tick,
    Appearance(winit::window::Theme),
    Quit,
}

impl From<accesskit_winit::Event> for UserEvent {
    fn from(event: accesskit_winit::Event) -> Self {
        Self::Accessibility(event)
    }
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
        eprintln!("Input {:?}: {event:?}", std::time::SystemTime::now());
    }
}

fn cursor_icon(cursor: Cursor) -> CursorIcon {
    match cursor {
        Cursor::Default => CursorIcon::Default,
        Cursor::Text => CursorIcon::Text,
        Cursor::Pointer => CursorIcon::Pointer,
        Cursor::Move => CursorIcon::Move,
        Cursor::EwResize => CursorIcon::EwResize,
        Cursor::NsResize => CursorIcon::NsResize,
        Cursor::NwseResize => CursorIcon::NwseResize,
        Cursor::NeswResize => CursorIcon::NeswResize,
    }
}

struct App {
    proxy: EventLoopProxy<UserEvent>,
    input: Option<Input>,
    substitutes: Vec<PathBuf>,
    /// Where `--screenshot` writes its PNGs, drawn from a window never shown.
    screenshot: Option<PathBuf>,
    /// What the window starts from besides its input, until it opens.
    launch: Option<settings::Launch>,
    state: Option<State>,
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

/// Asks the event loop to poll sections when their synchronization reports.
fn notify(proxy: EventLoopProxy<UserEvent>) -> impl Fn() + Send + 'static {
    move || {
        let _ = proxy.send_event(UserEvent::Sync);
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
    status: &'static str,
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
    /// Saving's state, and that the notebook opened through the system's mount of its
    /// share where Snowbound's own SMB client could not sign in.
    fn status(&self) -> String {
        match (&self.library.notice, self.status) {
            (None, status) => status.to_owned(),
            (Some(_), "") => "Via mounted share".to_owned(),
            (Some(_), status) => format!("{status} · Via mounted share"),
        }
    }

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
}

/// How long an opening page waits for its pictures before showing without them.
const HOLD: std::time::Duration = std::time::Duration::from_millis(200);

/// Work the interface asked for, done after the frame is built.
enum Command {
    /// Opens a notebook's section by catalog path, at the page it showed last.
    OpenSection(Arc<Library>, String),
    /// Asks for a notebook folder and opens it.
    OpenNotebook,
    /// Asks where to keep a new notebook and creates it.
    NewNotebook,
    /// Closes a notebook, keeping its files.
    CloseNotebook(Arc<Library>),
    /// Changes a notebook's sections and groups.
    Structure(Arc<Library>, manage::Structure),
    /// Adds a page at the end of the open section, or a subpage of a page, and edits its
    /// title.
    NewPage {
        under: Option<ExGuid>,
    },
    /// Deletes pages of the open section to the notebook's recycle bin.
    DeletePages(Vec<ExGuid>),
    /// Moves or indents pages of the open section.
    Pages(Vec<onestore::PageEdit>),
    /// Moves a page of the open section to the end of another section of its folder.
    MovePage {
        space: ExGuid,
        path: String,
    },
    /// Gives the open page a template's background or colour.
    Template(templates::Choice),
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
    /// Where each page was left this run, by `Library::key` and page space, as OneNote
    /// returns to it until the notebook closes.
    places: HashMap<(String, ExGuid), Place>,
    /// The page each section showed last this run, by `Library::key`.
    last_pages: HashMap<String, ExGuid>,
    /// Pages visited, for Back and Forward.
    trail: navigation::Trail,
    /// Asks for a frame when a worker thread finishes something the page shows.
    redraw: std::task::Waker,
    instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    renderer: Renderer,
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
    /// Notebooks and section groups whose rows the sidebar folds, by `sidebar::fold_key`.
    folded: HashSet<String>,
    /// The open context menu: what it was opened on, and where.
    menu: Option<(menus::Target, [f32; 2])>,
    /// The Options dialog's choices while it is open.
    options: Option<options::Options>,
    /// The Link dialog's fields while it is open.
    link: Option<link::LinkDialog>,
    /// The page's context menu while it is open: what it was opened on, and where.
    text_menu: Option<(canvas::interaction::Context, [f32; 2])>,
    color_scheme: settings::ColorScheme,
    light_pages: bool,
    /// Carries frames to the window where the system's backdrop shows through the strip.
    translucent: Option<draw::Translucent>,
    /// The strip's fill with the window focused and not, continuing the system's title bar.
    titlebar: [[f32; 4]; 2],
    /// A section or group being renamed in the sidebar.
    renaming: Option<rename::Renaming>,
    /// The notebook shown while it has no section open, as one whose last section was
    /// deleted.
    sectionless: Option<Arc<Library>>,
    /// What the pointer is dragging: a page's tab, a section tab or a sidebar row.
    drag: Option<menus::Drag>,
    /// Pages whose template strip was dismissed this run.
    dismissed: HashSet<ExGuid>,
    /// A page just created, whose title takes the caret once it opens.
    title_focus: Option<ExGuid>,
    /// What the template strip shows over a blank page.
    templates: templates::View,
    thumbnails: templates::Thumbnails,
    search: search::Search,
    /// Whether the page list is shown beside the page.
    pages_open: bool,
    /// Installed font families, for the font box; listing them is slow.
    fonts: Vec<String>,
    /// Fonts picked from the font box this session, latest first.
    recent_fonts: Vec<String>,
    /// What the toolbar's buttons apply from their menus' last picks.
    toolbar: settings::Toolbar,
    /// The application's icon at the display's scale, for the title bar.
    app_icon: Option<draw::RasterImage>,
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
    /// Where a replay asked the next frame to be written.
    snapshot: Option<PathBuf>,
    initial: Vec<(onestore::ExGuid, TextDocument)>,
    initial_layouts: Vec<(onestore::ExGuid, onestore::document::Layout)>,
    initial_date: Option<u64>,
    occluded: bool,
    ime_allowed: bool,
    clipboard: platform::Clipboard,
    access_adapter: accesskit_winit::Adapter,
    accessibility: accessibility::Accessibility,
}

fn strip() -> Id {
    Id::ROOT.child("strip")
}

fn page() -> Id {
    Id::ROOT.child("page")
}

/// The section's colour around the page and its tabs.
fn frame() -> Id {
    Id::ROOT.child("frame")
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
                    // The page list and a page beside it, under the toolbar's text group.
                    .with_min_inner_size(LogicalSize::new(600.0, 400.0)),
            )?,
        );
        platform::install_text_input(&window);
        // Beneath the surface's layer, which is added over it.
        let backdrop = visible && platform::install_backdrop(&window);
        platform::install_menu();
        let access_adapter =
            accesskit_winit::Adapter::with_event_loop_proxy(event_loop, &window, proxy.clone());
        window.set_visible(visible);
        let instance = wgpu::Instance::new(
            wgpu::InstanceDescriptor::new_with_display_handle_from_env(Box::new(window.clone())),
        );
        let surface = instance.create_surface(window.clone())?;
        platform::configure_presentation(&surface);
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                compatible_surface: Some(&surface),
                ..Default::default()
            })
            .await?;
        let (device, queue) = adapter.request_device(&Default::default()).await?;
        let size = window.inner_size();
        let mut config = surface
            .get_default_config(&adapter, size.width, size.height)
            .ok_or("No supported canvas surface")?;
        let translucent = backdrop.then(|| {
            // What wgpu calls post-multiplied is Core Animation's non-opaque layer, which
            // composites colour premultiplied in sRGB, as `Translucent` leaves it.
            config.alpha_mode = wgpu::CompositeAlphaMode::PostMultiplied;
            let window_format = config.format.remove_srgb_suffix();
            config.view_formats.push(window_format);
            draw::Translucent::new(&device, config.format, window_format)
        });
        surface.configure(&device, &config);
        let renderer = Renderer::new(device, queue, config.format);
        let mut engine = TextEngine::default();
        for path in substitutes {
            let target = engine
                .register_substitute(parley::fontique::Blob::new(Arc::new(std::fs::read(path)?)))?;
            eprintln!("Using {} for {target}", path.display());
        }
        let layouts = Arc::new(Mutex::new(engine.clone()));
        let temporary = matches!(input, Input::Notes { .. } | Input::Page(_));
        let mut notebooks = Vec::new();
        let mut session = None;
        let mut sectionless = None;
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
                }
                let shown = notebooks
                    .iter()
                    .filter(|library| Some(&library.location) == current.as_ref())
                    .chain(&notebooks)
                    .find_map(|library| Some((library, library.first_section()?)));
                match shown {
                    Some((library, path)) => {
                        let section = library.open(&path, notify(proxy.clone()))?;
                        let (opened, page) =
                            read_session(section, Arc::clone(library), path, None)?;
                        let (scene, editor) = PageScene::from_page(page, &mut engine)?;
                        session = Some(opened);
                        (editor, Some((scene, [0.0; 2])))
                    }
                    None => {
                        sectionless = notebooks
                            .iter()
                            .find(|library| Some(&library.location) == current.as_ref())
                            .or(notebooks.first())
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
        eprintln!("Canvas GPU: {:?}; scale factor {dpr}", adapter.get_info());
        window.set_theme(stored.color_scheme.theme());
        let appearance = stored
            .color_scheme
            .theme()
            .unwrap_or_else(|| platform::appearance(&window));
        let mut ui = Ui::new(
            theme(appearance, stored.light_pages, backdrop),
            platform::double_click_interval(),
        );
        let titlebar = platform::titlebar(appearance).unwrap_or([ui.theme.strip; 2]);
        // A window shown but never focused hears no focus event; a hidden one draws as focused.
        ui.window_focused = !visible || window.has_focus();
        ui.set_focus(Some(page()));
        for family in FONTS {
            for (face, _) in engine.substitute(family).map_or(&[][..], |s| &s.faces) {
                ui.preview_font(face.clone(), family);
            }
        }
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
        let clipboard = platform::Clipboard::new(&window)?;
        let redraw: std::task::Waker = Arc::new(Redraw(proxy.clone())).into();
        let search = search::Search::new(stored.search_scope, redraw.clone());
        let state = Self {
            author: stored.user_name.unwrap_or_else(platform::user_name),
            window,
            redraw,
            proxy,
            instance,
            surface,
            config,
            renderer,
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
            link: None,
            text_menu: None,
            options: None,
            color_scheme: stored.color_scheme,
            light_pages: stored.light_pages,
            translucent,
            titlebar,
            renaming: None,
            sectionless,
            drag: None,
            templates: templates::View::Strip,
            thumbnails: templates::Thumbnails::default(),
            search,
            pages_open: true,
            fonts,
            recent_fonts: stored.recent_fonts,
            toolbar: stored.toolbar,
            app_icon: platform::app_icon((16.0 * dpr).round() as u32),
            commands: Vec::new(),
            changed: false,
            moved: false,
            stale: false,
            page_focused: true,
            pointer: [0.0; 2],
            strip_press: None,
            strip_held: false,
            snapshot: None,
            initial,
            initial_date,
            initial_layouts,
            occluded: false,
            ime_allowed: true,
            clipboard,
            loads: mpsc::channel(),
            layouts,
            loading: 0,
            opening: None,
            places: HashMap::new(),
            last_pages: HashMap::new(),
            trail: navigation::Trail::default(),
            access_adapter,
            accessibility: accessibility::Accessibility::default(),
        };
        state.title();
        Ok(state)
    }

    /// Builds, lays out and paints one frame, then does what it asked for and asks for the
    /// frame that shows the result.
    fn frame(&mut self) -> Result<(), Box<dyn Error>> {
        let start = Instant::now();
        if let Err(error) = self.open_loaded() {
            eprintln!("{error}");
        }
        lap("open", start);
        let size = self.window.inner_size();
        let scale = self.scale();
        if scale != self.ui.scale() {
            self.renderer.clear_glyph_cache();
            let response = self.view.scale_factor_changed(scale)?;
            self.respond(response);
        }
        self.layout(
            [size.width, size.height].map(|side| side as f32 / self.window.scale_factor() as f32),
            scale,
        )?;
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
        self.draw()?;
        lap("drawn", start);
        self.sync_index(false);
        let commands = std::mem::take(&mut self.commands);
        let follow = !commands.is_empty() || self.ui.wants_frame() || self.opening.is_some();
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
        let (section, open_tab, open_page) = self.build()?;
        self.options_dialog();
        self.link_dialog()?;
        self.text_menu()?;
        self.ui.end();
        self.edges(section, open_tab, open_page);
        if let Some(rect) = self.ui.rect(page()) {
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
        self.ui.theme = theme(appearance, self.light_pages, self.translucent.is_some());
        self.titlebar = platform::titlebar(appearance).unwrap_or([self.ui.theme.strip; 2]);
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
                .and_then(|session| session.tabs[session.tab].color),
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
        if !platform::system_titlebar(&self.window) {
            self.title_bar(&theme);
        }
        platform::update_menu(|| self.statuses());
        if self.session.is_none() && !self.temporary && self.sectionless.is_none() {
            self.welcome(&theme);
            return Ok((section, Id::ROOT, None));
        }
        self.toolbar(&theme)?;
        self.ui.open(
            "body",
            Spec {
                size: [fill(), fill()],
                ..Spec::default()
            },
        );
        let sidebar = self.sidebar(&theme);
        self.ui.open(
            "main",
            Spec {
                axis: Axis::Y,
                size: [fill(), fill()],
                ..Spec::default()
            },
        );
        let tab_row = self.ui.open(
            "tabs",
            Spec {
                size: [fill(), px(TAB_ROW)],
                fill: Some(theme.strip),
                pad: [FRAME, 0.0],
                ..Spec::default()
            },
        );
        self.sidebar_button(&theme, sidebar);
        self.ui.leaf(
            "corner",
            Spec {
                size: [px(self.rounding()), px(1.0)],
                ..Spec::default()
            },
        );
        let row = self.ui.id("sections");
        let (clicked, open_tab) = match &self.session {
            Some(session) => {
                // A tab being renamed takes the name typed, which its field covers.
                let tabs: Vec<_> = session
                    .tabs
                    .iter()
                    .map(|tab| {
                        let name = match &self.renaming {
                            Some(renaming) if renaming.entry(&session.library, &tab.path, true) => {
                                renaming.name.as_str()
                            }
                            _ => tab.name.as_str(),
                        };
                        (name, section_color(tab.color))
                    })
                    .collect();
                let lit = self.page_drop(row);
                let dragged = self.dragged_tab(row);
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
                    "sections",
                    &tabs,
                    session.tab,
                    lit,
                    dragged,
                    &section,
                    TAB_ROW,
                );
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
                self.tab_rename_field(&theme, row, tab_row);
                (clicked, open_tab)
            }
            None => {
                let tabs = [("Temporary page", section_color(None))];
                let shown = if self.temporary { &tabs[..] } else { &[] };
                let open_tab = ui::shell::section_tabs(
                    &mut self.ui,
                    "sections",
                    shown,
                    0,
                    None,
                    None,
                    &section,
                    TAB_ROW,
                )
                .open;
                (None, open_tab)
            }
        };
        self.commands.extend(clicked);
        if self.session.is_some() {
            self.page_tools(&theme);
        }
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
        if let Some(library) = self.sectionless.clone().filter(|_| self.session.is_none()) {
            self.no_sections(&theme, library);
            self.ui.close();
            self.ui.close();
            self.ui.close();
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
                ..Spec::default()
            },
        );
        self.template_strip(&theme);
        let scroll = self.view.scroll();
        for (index, axis) in [Axis::X, Axis::Y].into_iter().enumerate() {
            if let Some(offset) = ui::scrollbar(
                &mut self.ui,
                index,
                axis,
                -self.view.viewport.origin[index],
                [scroll.min[index], scroll.max[index]],
                self.view.viewport.size[index] as f32,
                [0.18, 0.18, 0.18, 0.45],
            ) {
                let response = self.view.scroll_to(index, offset)?;
                self.respond(response);
            }
        }
        self.ui.close();
        let signal = self.ui.signal(page());
        let focused = self.ui.window_focused && self.ui.focused() == Some(page());
        if focused != self.page_focused {
            self.page_focused = focused;
            let response = self.view.focus_changed(focused)?;
            self.respond(response);
        }
        self.page_events(signal.events)?;
        self.ui.close();
        let open_page = self.page_list(&theme, &section, row);
        self.ui.close();
        self.ui.close();
        self.ui.close();
        self.context_menu();
        Ok((section, open_tab, open_page))
    }

    /// The page's corner radius, sharing its centres with the window's corners.
    fn rounding(&self) -> f32 {
        (platform::corner_radius(&self.window) - FRAME).max(0.0)
    }

    /// Borders the open section tab and the frame's top, and rounds and borders the page
    /// together with the open page's tab, joined where they meet.
    fn edges(&mut self, section: ui::Section, open_tab: Id, open_page: Option<Id>) {
        let panel = self.ui.rect(frame().child("panel"));
        let (Some(frame), Some(page)) = (self.ui.rect(frame()), self.ui.rect(page())) else {
            return;
        };
        let rounding = self.rounding();
        // Where the open page tab meets the page, concentric with its neighbours.
        let join = PILL_MARGIN + rounding;
        let [left, top, right, bottom] = page;
        let tab = open_page
            .and_then(|id| self.ui.rect(id))
            .map(|row| [row[0], row[1], row[2], row[3] - ROW_GAP])
            .zip(panel)
            .and_then(|(row, panel)| {
                let [start, end] = [
                    row[1].max(panel[1]).max(top),
                    row[3].min(panel[3]).min(bottom),
                ];
                let reach = row[2].min(panel[2]);
                (reach - right > 2.0 * join && end - start > 2.0 * rounding).then(|| {
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
        let mut outline = vec![([left, top], rounding)];
        match tab {
            Some([start, end, reach]) => {
                if start > top {
                    outline.extend([([right, top], rounding), ([right, start], join)]);
                }
                outline.extend([([reach, start], rounding), ([reach, end], rounding)]);
                if end < bottom {
                    outline.extend([([right, end], join), ([right, bottom], rounding)]);
                }
            }
            None => outline.extend([([right, top], rounding), ([right, bottom], rounding)]),
        }
        outline.push(([left, bottom], rounding));
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
        let beside = outer * start / sidebar::WIDTH;
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
                    ([right, frame[3]], 0.0),
                    ([left, frame[3]], (beside - cut).max(0.0)),
                ],
                paper,
                |_| strip,
            );
        }
        let [foot, toe] = self
            .ui
            .rect(open_tab)
            .map_or([end; 2], |tab| ui::shell::tab_base(tab, TAB_ROW));
        // The border runs down both sides to the window's bottom, round the bottom corner
        // beside the sidebar.
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
                ([east, frame[3]], 0.0),
            ],
            false,
            section.edge,
        );
    }

    /// The window's title bar beside the traffic lights: the application's icon, the
    /// notebook's name and the saving status.
    fn title_bar(&mut self, theme: &Theme) {
        self.ui.open_as(
            strip(),
            Spec {
                flags: Flags::CLICKABLE,
                size: [fill(), px(TITLE)],
                fill: Some(theme.strip),
                ..Spec::default()
            },
        );
        self.ui.leaf(
            "lights",
            Spec {
                size: [px(platform::LEADING), px(1.0)],
                ..Spec::default()
            },
        );
        // AppKit centres the window's title over the strip.
        if !cfg!(target_os = "macos") {
            self.ui.leaf(
                "notebook",
                Spec {
                    size: [fit(), px(TITLE)],
                    image: self.app_icon.as_ref(),
                    text: Some(match &self.session {
                        Some(session) => &session.library.name,
                        None if self.temporary => "Temporary page",
                        None => "Snowbound",
                    }),
                    ..Spec::default()
                },
            );
        }
        self.ui.leaf(
            "space",
            Spec {
                size: [fill(), px(1.0)],
                ..Spec::default()
            },
        );
        if let Some(session) = &self.session {
            self.ui.leaf(
                "status",
                Spec {
                    size: [fit(), px(TITLE)],
                    text: Some(&session.status()),
                    color: Some(theme.text_dim),
                    pad: [12.0, 0.0],
                    ..Spec::default()
                },
            );
        }
        platform::window_controls(&mut self.ui, &self.window);
        self.ui.close();
    }

    /// Two rows of formatting, tag, insert and zoom buttons, grouped as OneNote's Home
    /// ribbon groups them.
    fn toolbar(&mut self, theme: &Theme) -> Result<(), Box<dyn Error>> {
        use canvas::editor::{
            Alignment, BULLET_LIBRARY, ListStyle, NUMBER_LIBRARY, NoteTag, Toggle,
        };
        use commands::{Choice, Id as Cmd, shortcut};
        // A focused picture has no text to show a format for.
        let state = self.format_state();
        let fonts = &self.fonts;
        let recent_fonts = &self.recent_fonts;
        let pens = &self.toolbar;
        let engine = &self.view.engine;
        // Under the system's title bar the save status joins the toolbar.
        let status = platform::system_titlebar(&self.window)
            .then(|| self.session.as_ref().map(Session::status))
            .flatten();
        let ui = &mut self.ui;
        let text = theme.text;
        let on = |toggle| state.toggles.contains(&toggle);
        let mut choice = None;
        ui.open(
            "toolbar",
            Spec {
                size: [fill(), children()],
                fill: Some(theme.strip),
                pad: [8.0, 3.0],
                gap: 6.0,
                ..Spec::default()
            },
        );
        group(ui, "history", |ui| {
            if ui::shell::tool_button(ui, "undo", art::UNDO, text, false).clicked {
                choice = Some(Choice::Command(Cmd::Undo));
            }
            if ui::shell::tool_button(ui, "redo", art::REDO, text, false).clicked {
                choice = Some(Choice::Command(Cmd::Redo));
            }
        });
        divider(ui, "history", theme);
        let popup = |name: &str| Id::ROOT.child(("popup", name));
        let font = state.font.clone().unwrap_or_default();
        let size = state
            .font_size
            .map_or(String::new(), |size| format!("{size}"));
        group(ui, "text", |ui| {
            row(ui, 0, |ui| {
                let combo = ui.id("font");
                if ui::shell::combo(ui, "font", &font, 120.0).pressed {
                    ui.open_popup(popup("font"));
                }
                // Each family is named with the substitute it shows in, and picked by its
                // own name; a family of None heads a group. Typing searches the full list.
                let label = |name: &str| {
                    engine.substitute(name).map_or_else(
                        || name.to_owned(),
                        |substitute| format!("{} ({name})", substitute.name),
                    )
                };
                let mut choices: Vec<(String, Option<&str>, bool)> = Vec::new();
                if ui::popup::query(ui, popup("font")).is_none_or(str::is_empty) {
                    choices.extend(FONTS.map(|name| (label(name), Some(name), false)));
                    if !recent_fonts.is_empty() {
                        choices.push(("Recent".to_owned(), None, true));
                        choices.extend(
                            recent_fonts
                                .iter()
                                .map(|name| (label(name), Some(name.as_str()), false)),
                        );
                    }
                }
                let listed = choices.len();
                choices.extend(fonts.iter().enumerate().map(|(index, name)| {
                    (label(name), Some(name.as_str()), index == 0 && listed > 0)
                }));
                let items: Vec<_> = choices
                    .iter()
                    .map(|(label, name, separated)| ui::popup::Item {
                        text: label,
                        font: *name,
                        current: *name == Some(font.as_str()),
                        heading: name.is_none(),
                        separated: *separated,
                        ..Default::default()
                    })
                    .collect();
                let anchor = ui::Anchor::Over(ui.rect(combo).unwrap_or_default());
                if let Some(index) =
                    ui::popup::menu(ui, popup("font"), anchor, &items, Some("Font"))
                    && let Some(name) = choices[index].1
                {
                    choice = Some(Choice::Font(name.to_owned()));
                }
                let combo = ui.id("size");
                if ui::shell::combo(ui, "size", &size, 44.0).pressed {
                    ui.open_popup(popup("size"));
                }
                // A size typed in the field joins the list, in half points as stored.
                let mut sizes = SIZES.to_vec();
                if let Some(typed) = ui::popup::query(ui, popup("size"))
                    .and_then(|query| query.trim().parse::<f32>().ok())
                    .map(|typed| (typed * 2.0).round() / 2.0)
                    .filter(|typed| onestore::FONT_SIZES.contains(typed) && !sizes.contains(typed))
                {
                    let at = sizes.partition_point(|size| *size < typed);
                    sizes.insert(at, typed);
                }
                let labels: Vec<_> = sizes.iter().map(|size| format!("{size}")).collect();
                let items: Vec<_> = labels
                    .iter()
                    .map(|label| ui::popup::Item {
                        text: label,
                        checked: *label == size,
                        ..Default::default()
                    })
                    .collect();
                let anchor = ui::Anchor::Over(ui.rect(combo).unwrap_or_default());
                if let Some(index) = ui::popup::menu(ui, popup("size"), anchor, &items, Some(&size))
                {
                    choice = Some(Choice::Size(sizes[index]));
                }
                // The buttons apply OneNote's default bullet and number, as its own do.
                let [bullets, numbering] = [ui.id("bullets"), ui.id("numbering")];
                if ui::shell::split_button(
                    ui,
                    "bullets",
                    art::BULLETS,
                    None,
                    state.bullets,
                    popup("bullets"),
                )
                .clicked
                {
                    choice = Some(Choice::Command(Cmd::Bullets));
                }
                if ui::shell::split_button(
                    ui,
                    "numbering",
                    art::NUMBERING,
                    None,
                    state.numbering,
                    popup("numbering"),
                )
                .clicked
                {
                    choice = Some(Choice::Command(Cmd::Numbering));
                }
                let current = |bullet| match state.list {
                    Some(ListStyle::Bullet(place)) if bullet => Some(place),
                    Some(ListStyle::Number(place)) if !bullet => Some(place),
                    _ => None,
                };
                let anchor = ui::Anchor::Below(ui.rect(bullets).unwrap_or_default());
                if let Some(place) = list_gallery(
                    ui,
                    popup("bullets"),
                    anchor,
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
                let anchor = ui::Anchor::Below(ui.rect(numbering).unwrap_or_default());
                if let Some(place) = list_gallery(
                    ui,
                    popup("numbering"),
                    anchor,
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
                if ui::shell::tool_button(ui, "clear", art::CLEAR_FORMATTING, text, false).clicked {
                    choice = Some(Choice::Command(Cmd::ClearFormatting));
                }
            });
            row(ui, 1, |ui| {
                for (part, icon, toggle) in [
                    ("bold", art::BOLD, Toggle::Bold),
                    ("italic", art::ITALIC, Toggle::Italic),
                    ("underline", art::UNDERLINE, Toggle::Underline),
                    ("strikethrough", art::STRIKETHROUGH, Toggle::Strikethrough),
                ] {
                    if ui::shell::tool_button(ui, part, icon, text, on(toggle)).clicked {
                        choice = Some(Choice::Command(Cmd::Toggle(toggle)));
                    }
                }
                let script = ui.id("script");
                let superscript = on(Toggle::Superscript);
                if ui::shell::split_button(
                    ui,
                    "script",
                    if superscript {
                        art::SUPERSCRIPT
                    } else {
                        art::SUBSCRIPT
                    },
                    None,
                    on(Toggle::Subscript) || superscript,
                    popup("script"),
                )
                .clicked
                {
                    choice = Some(Choice::Command(Cmd::Toggle(if superscript {
                        Toggle::Superscript
                    } else {
                        Toggle::Subscript
                    })));
                }
                let scripts = [
                    (Toggle::Subscript, art::SUBSCRIPT),
                    (Toggle::Superscript, art::SUPERSCRIPT),
                ];
                let keys = scripts.map(|(toggle, _)| shortcut(Cmd::Toggle(toggle)));
                let items: Vec<_> = scripts
                    .iter()
                    .zip(&keys)
                    .map(|((toggle, icon), key)| ui::popup::Item {
                        text: commands::command(Cmd::Toggle(*toggle)).title,
                        icon: Some(icon),
                        shortcut: key,
                        checked: on(*toggle),
                        ..Default::default()
                    })
                    .collect();
                let anchor = ui::Anchor::Below(ui.rect(script).unwrap_or_default());
                if let Some(index) = ui::popup::menu(ui, popup("script"), anchor, &items, None) {
                    choice = Some(Choice::Command(Cmd::Toggle(scripts[index].0)));
                }
                // Each button applies its menu's last pick: the highlighter shows it in its
                // artwork, the font colour in a bar.
                for (part, icon, swatches, columns, none, bar) in [
                    (
                        "highlight",
                        art::HIGHLIGHTER,
                        &HIGHLIGHTS.map(|(color, _)| color)[..],
                        5,
                        "No Color",
                        None,
                    ),
                    (
                        "color",
                        art::FONT_COLOR,
                        &FONT_COLORS[..],
                        10,
                        "Automatic",
                        Some(pens.font_color.map_or(text, colorref)),
                    ),
                ] {
                    let split = ui.id(part);
                    let button = ui::shell::split_button(ui, part, icon, bar, false, popup(part));
                    let highlight = part == "highlight";
                    if button.clicked {
                        choice = Some(Choice::Command(if highlight {
                            Cmd::Highlight
                        } else {
                            Cmd::FontColor
                        }));
                    }
                    let colors: Vec<_> = swatches.iter().map(|color| colorref(*color)).collect();
                    let anchor = ui::Anchor::Below(ui.rect(split).unwrap_or_default());
                    if let Some(chosen) =
                        ui::popup::colors(ui, popup(part), anchor, none, &colors, columns)
                    {
                        let color = chosen.and_then(|chosen| {
                            swatches
                                .iter()
                                .zip(&colors)
                                .find(|(_, color)| **color == chosen)
                                .map(|(stored, _)| *stored)
                        });
                        choice = Some(if highlight {
                            Choice::Highlight(color)
                        } else {
                            Choice::Color(color)
                        });
                    }
                }
                if ui::shell::tool_button(ui, "outdent", art::OUTDENT, text, false).clicked {
                    choice = Some(Choice::Command(Cmd::Outdent));
                }
                if ui::shell::tool_button(ui, "indent", art::INDENT, text, false).clicked {
                    choice = Some(Choice::Command(Cmd::Indent));
                }
                let alignments = [
                    (Alignment::Left, art::ALIGN_LEFT),
                    (Alignment::Center, art::ALIGN_CENTER),
                    (Alignment::Right, art::ALIGN_RIGHT),
                ];
                let keys = alignments.map(|(alignment, _)| shortcut(Cmd::Align(alignment)));
                let current = alignments
                    .iter()
                    .find(|(alignment, _)| Some(*alignment) == state.alignment)
                    .unwrap_or(&alignments[0]);
                let anchor = ui::shell::menu_button(ui, "align", current.1, popup("align"));
                // The current alignment starts highlighted, keeping its icon under the button's.
                let items: Vec<_> = alignments
                    .iter()
                    .zip(&keys)
                    .map(|((alignment, icon), key)| ui::popup::Item {
                        text: commands::command(Cmd::Align(*alignment)).title,
                        icon: Some(icon),
                        shortcut: key,
                        current: Some(*alignment) == state.alignment,
                        ..Default::default()
                    })
                    .collect();
                if let Some(index) = ui::popup::menu(ui, popup("align"), anchor, &items, None) {
                    choice = Some(Choice::Command(Cmd::Align(alignments[index].0)));
                }
            });
        });
        divider(ui, "text", theme);
        let tags: [(&[&str], NoteTag); 9] = [
            (
                tag_sources(TagIcon::CheckBox { checked: false }),
                NoteTag::ToDo,
            ),
            (tag_sources(TagIcon::Star), NoteTag::Important),
            (tag_sources(TagIcon::Question), NoteTag::Question),
            (art::TAG_REMEMBER, NoteTag::RememberForLater),
            (art::TAG_DEFINITION, NoteTag::Definition),
            (tag_sources(TagIcon::Highlight), NoteTag::Highlight),
            (tag_sources(TagIcon::Contact), NoteTag::Contact),
            (tag_sources(TagIcon::Address), NoteTag::Address),
            (tag_sources(TagIcon::Phone), NoteTag::PhoneNumber),
        ];
        let all_tags = &tags;
        group(ui, "tags", |ui| {
            for (index, tags) in tags.chunks(5).enumerate() {
                row(ui, index, |ui| {
                    for (column, (icon, tag)) in tags.iter().enumerate() {
                        let lit = state.tags.contains(tag);
                        if ui::shell::tool_button(ui, column, icon, [1.0; 4], lit).clicked {
                            choice = Some(Choice::Command(Cmd::Tag(*tag)));
                        }
                    }
                    if index == 1 {
                        let more = ui.id("more");
                        let open = ui.popup_open(popup("tags"));
                        if ui::shell::tool_button(ui, "more", ui::shell::CHEVRON, text, open)
                            .pressed
                        {
                            ui.open_popup(popup("tags"));
                        }
                        let shortcuts: Vec<_> = all_tags
                            .iter()
                            .map(|(_, tag)| shortcut(Cmd::Tag(*tag)))
                            .collect();
                        let remove = shortcut(Cmd::RemoveTags);
                        let mut items: Vec<_> = all_tags
                            .iter()
                            .zip(&shortcuts)
                            .map(|((icon, tag), shortcut)| ui::popup::Item {
                                text: commands::command(Cmd::Tag(*tag)).title,
                                icon: Some(*icon),
                                colored: true,
                                shortcut,
                                ..Default::default()
                            })
                            .collect();
                        items.push(ui::popup::Item {
                            text: commands::command(Cmd::RemoveTags).title,
                            shortcut: &remove,
                            separated: true,
                            ..Default::default()
                        });
                        let anchor = ui::Anchor::Below(ui.rect(more).unwrap_or_default());
                        if let Some(chosen) =
                            ui::popup::menu(ui, popup("tags"), anchor, &items, None)
                        {
                            choice = Some(Choice::Command(
                                all_tags
                                    .get(chosen)
                                    .map_or(Cmd::RemoveTags, |(_, tag)| Cmd::Tag(*tag)),
                            ));
                        }
                    }
                });
            }
        });
        divider(ui, "tags", theme);
        group(ui, "insert", |ui| {
            for (index, buttons) in [
                &[
                    ("table", art::TABLE, Cmd::Table),
                    ("picture", art::PICTURE, Cmd::Picture),
                    ("file", art::ATTACHMENT, Cmd::Attachment),
                    ("link", art::LINK, Cmd::Link),
                ][..],
                &[
                    ("date", art::CALENDAR, Cmd::Date),
                    ("time", art::CLOCK, Cmd::Time),
                    ("equation", art::EQUATION, Cmd::Equation),
                ],
            ]
            .into_iter()
            .enumerate()
            {
                row(ui, index, |ui| {
                    for (part, icon, id) in buttons {
                        if ui::shell::tool_button(ui, part, icon, text, false).clicked {
                            choice = Some(Choice::Command(*id));
                        }
                    }
                });
            }
        });
        ui.leaf(
            "space",
            Spec {
                size: [fill(), px(1.0)],
                ..Spec::default()
            },
        );
        if let Some(status) = status {
            ui.leaf(
                "status",
                Spec {
                    size: [fit(), px(ui::shell::TOOL)],
                    text: Some(&status),
                    color: Some(theme.text_dim),
                    center: true,
                    ..Spec::default()
                },
            );
        }
        let zoom = self.view.zoom();
        let label = format!("{:.0}%", zoom * 100.0);
        group(ui, "zoom", |ui| {
            row(ui, 0, |ui| {
                if ui::shell::tool_button(ui, "out", art::ZOOM_OUT, text, false).clicked {
                    choice = Some(Choice::Command(Cmd::ZoomOut));
                }
                let level = ui.leaf(
                    "level",
                    Spec {
                        flags: Flags::CLICKABLE,
                        size: [px(44.0), px(ui::shell::TOOL)],
                        text: Some(&label),
                        hover_fill: Some(theme.hover()),
                        radius: 4.0,
                        center: true,
                        ..Spec::default()
                    },
                );
                if level.clicked {
                    choice = Some(Choice::Command(Cmd::ActualSize));
                }
                if ui::shell::tool_button(ui, "in", art::ZOOM_IN, text, false).clicked {
                    choice = Some(Choice::Command(Cmd::ZoomIn));
                }
            });
        });
        ui.close();
        if let Some(choice) = choice {
            self.choose(choice);
        }
        Ok(())
    }

    /// The search box and the page list's buttons, above the list.
    fn page_tools(&mut self, theme: &Theme) {
        self.ui.open(
            "tools",
            Spec {
                size: [px(PAGE_LIST), px(TAB_ROW)],
                pad: [0.0, (TAB_ROW - ui::shell::TOOL) / 2.0],
                gap: 3.0,
                ..Spec::default()
            },
        );
        if let Err(error) = self.search_box(theme) {
            eprintln!("{error}");
        }
        if ui::shell::tool_button(&mut self.ui, "new", art::PLUS, theme.text, false).clicked {
            self.commands.push(Command::NewPage { under: None });
        }
        let toggle = if self.pages_open {
            art::SIDEBAR_COLLAPSE
        } else {
            art::SIDEBAR_EXPAND
        };
        if ui::shell::tool_button(&mut self.ui, "toggle", toggle, theme.text, false).clicked {
            self.pages_open = !self.pages_open;
        }
        self.ui.close();
    }

    /// The section's pages as tabs down the frame's right side, returning the open page's
    /// tab, which is the page's colour and joins it.
    fn page_list(&mut self, theme: &Theme, section: &ui::Section, tabs: Id) -> Option<Id> {
        let session = self.session.as_ref()?;
        let panel = self.ui.id("panel");
        let width = self
            .ui
            .animate(panel, if self.pages_open { PAGE_LIST } else { 0.0 });
        self.ui.open(
            "panel",
            Spec {
                flags: Flags::SCROLL | Flags::CLIP,
                axis: Axis::Y,
                size: [px(width), fill()],
                ..Spec::default()
            },
        );
        let found = self
            .search
            .found_in(&session.library.key(&session.tabs[session.tab].path));
        let rounding = self.rounding();
        let dragged = self.dragged_page();
        let rows = page_rows(
            &mut self.ui,
            theme,
            section,
            session,
            &found,
            rounding,
            self.renaming.as_mut(),
            dragged,
        );
        self.commands.extend(
            rows.version
                .map(|(page, version)| Command::OpenVersion { page, version }),
        );
        if let Some((space, point)) = rows.context {
            self.menu = Some((menus::Target::Page(space), point));
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
        self.ui.close();
        rows.open
    }

    /// Hands the page the events routed to its box, in its device pixels.
    fn page_events(&mut self, events: Vec<ui::Event>) -> Result<(), Box<dyn Error>> {
        let read_only = self.session.as_ref().is_some_and(Session::read_only);
        let scale = self.ui.scale();
        let corner = self.ui.rect(page()).unwrap_or_default();
        let device = |point: [f32; 2]| {
            [
                (point[0] - corner[0]) * scale,
                (point[1] - corner[1]) * scale,
            ]
        };
        for event in events {
            let response = match event {
                ui::Event::PointerMoved(point) => self.view.pointer_moved(device(point))?,
                ui::Event::PointerLeft => self.view.pointer_left(),
                ui::Event::Button {
                    button: MouseButton::Left,
                    pressed,
                    at,
                } => {
                    if pressed {
                        self.view.pointer_pressed(at)?
                    } else {
                        self.view.pointer_released()?
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
                ui::Event::Button { .. } | ui::Event::Ime(Ime::Enabled) => continue,
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
                let notify = notify(self.proxy.clone());
                let last = self.last_pages.get(&library.key(&path)).copied();
                self.load(move || {
                    let section = library.open(&path, notify)?;
                    let (session, page) = read_session(section, library, path, last)?;
                    Ok(Loaded::Section(Box::new(session), page))
                });
            }
            Command::OpenPage(space) => {
                let session = self.session.as_ref().ok_or("No section is open")?;
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
                session.status = "Saving";
                session.refresh_conflicts()?;
                self.commands.push(Command::OpenPage(page));
            }
            Command::CopyVersion { version, tab } => {
                let session = self.session.as_ref().ok_or("No section is open")?;
                let page = match session.version {
                    Some(open) if open == version => {
                        session.section.version(session.space, open)?
                    }
                    _ => session.section.page(version)?,
                };
                if tab == session.tab {
                    session.section.import_page(&page, &self.author)?;
                } else {
                    let library = Arc::clone(&session.library);
                    let path = session.tabs[tab].path.clone();
                    let (notify, author) = (notify(self.proxy.clone()), self.author.clone());
                    std::thread::spawn(move || {
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
            Command::OpenNotebook => {
                if let Some(path) = platform::pick_notebook("Open Notebook") {
                    self.open_path(&path);
                }
            }
            Command::NewNotebook => self.new_notebook()?,
            Command::CloseNotebook(library) => self.close_notebook(&library),
            Command::Structure(library, change) => self.restructure(library, change),
            // The index follows the section's page list.
            Command::NewPage { under } => {
                self.new_page(under)?;
                self.edited(Vec::new());
            }
            Command::DeletePages(pages) => {
                self.delete_pages(pages)?;
                self.edited(Vec::new());
            }
            Command::Pages(edits) => self.edit_pages(edits)?,
            Command::MovePage { space, path } => {
                self.move_page(space, path)?;
                self.edited(Vec::new());
            }
            Command::Template(choice) => self.apply_template(choice)?,
            Command::Page(Request::EditDate(field)) => self.edit_date(field)?,
            Command::Page(Request::Copy(text)) => self.clipboard.set_text(text)?,
            Command::Page(Request::Paste) => {
                let text = self.clipboard.get_text()?;
                let language = canvas::language::lcid(&platform::input_language());
                let response = self.view.paste(&text, language)?;
                self.respond(response);
            }
            Command::Page(Request::OpenLink(address)) => self.open_link(&address)?,
            Command::Choose(choice) => self.run(choice)?,
        }
        Ok(())
    }

    /// Runs `read` on a thread of its own, which lays out the page it read; `open_loaded`
    /// shows it unless a newer read was asked for meanwhile.
    fn load(&mut self, read: impl FnOnce() -> Result<Loaded, Box<dyn Error>> + Send + 'static) {
        self.loading += 1;
        let (id, sender, redraw) = (self.loading, self.loads.0.clone(), self.redraw.clone());
        let layouts = Arc::clone(&self.layouts);
        std::thread::spawn(move || {
            let laid = read().and_then(|loaded| {
                let (shown, page) = match loaded {
                    Loaded::Section(session, page) => (Shown::Section(session), page),
                    Loaded::Created(session, page) => (Shown::Created(session), page),
                    Loaded::Page(space, page) => (Shown::Page(space), page),
                    Loaded::Version(space, version, page) => (Shown::Version(space, version), page),
                    Loaded::Library(library, path) => return Ok(Laid::Library(library, path)),
                };
                let mut engine = layouts.lock().map_err(|_| "Page layout failed")?;
                let (scene, editor) = PageScene::from_page(page, &mut engine)?;
                Ok(Laid::Page(Box::new(Opening {
                    loaded: shown,
                    scene: (scene, [0.0; 2]),
                    editor,
                    since: Instant::now(),
                })))
            });
            let _ = sender.send((id, laid.map_err(|error| error.to_string())));
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
                    eprintln!("{error}");
                    platform::alert("Couldn't open", &error);
                }
            }
        }
        let Some(mut opening) = self.opening.take() else {
            return Ok(());
        };
        let paper = canvas::gpu::Paper {
            color: self.ui.theme.paper,
            ink: self.ui.theme.paper_ink,
        }
        .colored(opening.editor.page_color());
        if !self
            .view
            .prepare(&mut opening.scene, &opening.editor, paper, &self.redraw)
            && opening.since.elapsed() < HOLD
        {
            self.opening = Some(opening);
            return Ok(());
        }
        // The page shown so far keeps what was typed while the next one loaded.
        self.persist()?;
        if let Some(session) = &self.session {
            let key = session.library.key(&session.tabs[session.tab].path);
            self.places
                .insert((key.clone(), session.space), self.view.place());
            self.last_pages.insert(key, session.space);
        }
        let created = matches!(opening.loaded, Shown::Created(_));
        match opening.loaded {
            Shown::Section(session) | Shown::Created(session) => {
                // The settings keep the notebook shown, not the section.
                let other_notebook = self
                    .session
                    .as_ref()
                    .is_none_or(|open| open.library.location != session.library.location);
                // A notebook read again after a change replaces the one it was.
                match self
                    .notebooks
                    .iter_mut()
                    .find(|library| library.location == session.library.location)
                {
                    Some(library) => *library = Arc::clone(&session.library),
                    None => self.notebooks.push(Arc::clone(&session.library)),
                }
                self.session = Some(*session);
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
            }
        }
        let place = self.session.as_ref().and_then(|session| {
            let key = session.library.key(&session.tabs[session.tab].path);
            self.places.get(&(key, session.space)).copied()
        });
        self.view.open(opening.editor, Some(opening.scene), place);
        self.visited();
        self.opened()?;
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
        match self
            .notebooks
            .iter_mut()
            .find(|open| open.location == library.location)
        {
            Some(open) => *open = Arc::clone(&library),
            None => self.notebooks.push(Arc::clone(&library)),
        }
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
                self.sectionless = Some(library);
                self.title();
            }
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

    /// Follows a page shown in place of another.
    fn opened(&mut self) -> Result<(), Box<dyn Error>> {
        // A page a search result shows leaves the keys with the search.
        if !search::takes_text(self.ui.focused()) {
            self.ui.set_focus(Some(page()));
        }
        self.title();
        self.update_accessibility()?;
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
        let corner = self.ui.rect(page()).unwrap_or_default();
        let [x0, y0, x1, y1] = self.view.caret_area()?;
        self.window.set_ime_cursor_area(
            LogicalPosition::new(x0 / scale + corner[0], y0 / scale + corner[1]),
            LogicalSize::new((x1 - x0) / scale, (y1 - y0) / scale),
        );
        let start = Instant::now();
        self.update_accessibility()?;
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
                format!("{} · {file}", session.title())
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

    fn edit_date(&mut self, field: DateField) -> Result<(), Box<dyn Error>> {
        let Some(date) = self.view.editor.date() else {
            return Ok(());
        };
        let timestamp = date.timestamp();
        self.view.editor.finish_composition();
        platform::clear_marked_text(&self.window);
        let title = match field {
            DateField::Date => "Change Page Date",
            DateField::Time => "Change Page Time",
        };
        if let Some((timestamp, text)) = platform::edit_date(timestamp, field, title)? {
            let response = self.view.change_date(timestamp, text)?;
            self.respond(response);
            self.window.request_redraw();
        }
        Ok(())
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
        session.status = "Saving";
        // The page list shows the title as it is typed.
        if self.view.editor.active_outline().title {
            session.pages = session.section.pages()?;
        }
        self.edited(vec![space]);
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
        if let Some(session) = &mut self.session {
            session.status = "Not saving";
        }
        self.refresh()?;
        platform::alert(
            "Change not saved",
            "The page now shows what was last saved. Your text is on the clipboard to paste back.",
        );
        Ok(())
    }

    /// Applies what the section reported since the last poll.
    fn synced(&mut self) -> Result<(), Box<dyn Error>> {
        let Some(session) = &mut self.session else {
            return Ok(());
        };
        let shown = session.status;
        let mut listed = false;
        let mut changed = false;
        let mut rejected = None;
        for event in session.section.events() {
            use notebook::session::Event;
            session.status = match event {
                Event::Changed(spaces) => {
                    listed = true;
                    changed |= spaces.contains(&session.space);
                    self.search.changed(
                        session.library.key(&session.tabs[session.tab].path),
                        session.section.replica(),
                        spaces,
                    );
                    continue;
                }
                Event::Rejected { spaces, error } => {
                    if spaces.contains(&session.space) {
                        rejected = Some(error);
                    } else {
                        eprintln!("Saving failed: {error}");
                    }
                    "Not saving"
                }
                Event::Attempt {
                    status: notebook::EditStatus::Published { .. },
                    ..
                } => "Saved",
                Event::Attempt { .. } => "Saving",
                Event::Unreachable(_) => "Offline",
                Event::Failed(error) => {
                    eprintln!("Synchronization stopped: {error}");
                    "Not saving"
                }
            };
        }
        if listed || changed || shown != session.status {
            self.window.request_redraw();
        }
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

    fn update_accessibility(&mut self) -> Result<(), Box<dyn Error>> {
        let mut error = None;
        let view = &self.view;
        let corner = self.ui.rect(page()).unwrap_or_default();
        let scale = self.ui.scale();
        // Bounds are in the window's pixels, which `scale` renders fewer of when capped.
        let ratio = self.window.scale_factor() as f32 / scale;
        let viewport = canvas::gpu::Viewport {
            size: view.viewport.size.map(|side| (side as f32 * ratio) as u32),
            scale: view.viewport.scale * ratio,
            origin: [0, 1].map(|axis| (view.viewport.origin[axis] + corner[axis] * scale) * ratio),
        };
        self.access_adapter.update_if_active(|| {
            match self.accessibility.update(
                &view.editor,
                view.scene.as_ref(),
                viewport,
                &self.window.title(),
                view.outline_preview(),
                view.object_focus().and_then(|focus| focus.read_only()),
            ) {
                Ok(update) => update,
                Err(failure) => {
                    error = Some(failure);
                    self.accessibility.deactivate();
                    let mut root = accesskit::Node::new(accesskit::Role::Window);
                    root.set_label(self.window.title());
                    accesskit::TreeUpdate {
                        nodes: vec![(accessibility::ROOT, root)],
                        tree: Some(accesskit::TreeInfo::new(accessibility::ROOT)),
                        tree_id: accesskit::TreeId::ROOT,
                        focus: accessibility::ROOT,
                    }
                }
            }
        });
        if let Some(error) = error {
            return Err(error.into());
        }
        Ok(())
    }

    fn access_action(&mut self, request: accesskit::ActionRequest) -> Result<(), Box<dyn Error>> {
        trace_input(&request);
        use accesskit::{Action, ActionData};
        if request.target_tree != accesskit::TreeId::ROOT {
            return Ok(());
        }
        if let Some(field) = self.accessibility.date_for_node(request.target_node) {
            if request.action == Action::Click {
                self.edit_date(field)?;
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
            self.snapshot(&path)?;
        }
        if self.occluded || [self.config.width, self.config.height].contains(&0) {
            return Ok(());
        }
        let mut reconfigure = false;
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) => frame,
            wgpu::CurrentSurfaceTexture::Suboptimal(frame) => {
                reconfigure = true;
                frame
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.configure(&self.renderer.device, &self.config);
                self.window.request_redraw();
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                self.surface = self.instance.create_surface(self.window.clone())?;
                platform::configure_presentation(&self.surface);
                self.surface.configure(&self.renderer.device, &self.config);
                self.window.request_redraw();
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Timeout => {
                trace_input(&"Surface timeout");
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Occluded => {
                trace_input(&"Surface occluded");
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                return Err("Canvas surface validation failed".into());
            }
        };
        lap("acquire", start);
        let size = [self.config.width, self.config.height];
        let target = self
            .translucent
            .as_mut()
            .map(|translucent| translucent.target(&self.renderer.device, size));
        self.paint(
            target
                .as_ref()
                .unwrap_or(&frame.texture.create_view(&Default::default())),
        )?;
        if let Some(translucent) = &self.translucent {
            let window = frame.texture.create_view(&wgpu::TextureViewDescriptor {
                format: Some(self.config.format.remove_srgb_suffix()),
                ..Default::default()
            });
            translucent.present(&self.renderer.device, &self.renderer.queue, &window);
        }
        self.window.pre_present_notify();
        let start = Instant::now();
        self.renderer.queue.present(frame);
        platform::commit_presentation(&self.window);
        lap("present", start);
        trace_input(&"Present submitted");
        if reconfigure {
            self.surface.configure(&self.renderer.device, &self.config);
        }
        Ok(())
    }

    /// The window's pixels per point, lowered where the window is larger than the GPU's
    /// largest texture, so the surface renders scaled rather than failing.
    fn scale(&self) -> f32 {
        let size = self.window.inner_size();
        let most = self.renderer.device.limits().max_texture_dimension_2d as f32;
        let longest = size.width.max(size.height) as f32;
        self.window.scale_factor() as f32 * (most / longest).min(1.0)
    }

    /// The paper the open page lies on: the theme's, in the page's colour.
    fn paper(&self) -> canvas::gpu::Paper {
        canvas::gpu::Paper {
            color: self.ui.theme.paper,
            ink: self.ui.theme.paper_ink,
        }
        .colored(self.view.editor.page_color())
    }

    /// Paints the interface with the page in its box.
    fn paint(&mut self, target: &wgpu::TextureView) -> Result<(), Box<dyn Error>> {
        let start = Instant::now();
        let paper = self.paper();
        self.view.update_pictures(paper, &self.redraw);
        let theme = &self.ui.theme;
        let page_primitives = self.view.primitives(TextColors {
            caret: theme.caret,
            selection: if self.page_focused {
                theme.selection
            } else {
                theme.inactive_selection
            },
            paper,
        })?;
        lap("page primitives", start);
        let scale = self.ui.scale();
        let corner = self.ui.rect(page()).unwrap_or_default();
        let viewport = self.view.viewport;
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
                    motion: None,
                    primitives: &page_primitives,
                },
            })
            .collect();
        trace_input(&("Draw", viewport.origin, viewport.scale));
        let start = Instant::now();
        self.renderer
            .draw(
                target,
                [self.config.width, self.config.height],
                self.ui.theme.strip,
                &layers,
            )
            .map_err(|error| format!("Canvas drawing failed: {error:?}"))?;
        lap("render", start);
        Ok(())
    }

    /// Writes the frame to a PNG, so a covered window can be reviewed.
    fn snapshot(&mut self, path: &Path) -> Result<(), Box<dyn Error>> {
        let size = [self.config.width, self.config.height];
        let device = &self.renderer.device;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Snapshot"),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.config.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let row = (size[0] * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Snapshot readback"),
            size: u64::from(row) * u64::from(size[1]),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        self.paint(&texture.create_view(&Default::default()))?;
        let mut encoder = self
            .renderer
            .device
            .create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(size[1]),
                },
            },
            texture.size(),
        );
        self.renderer.queue.submit([encoder.finish()]);
        buffer.map_async(wgpu::MapMode::Read, .., |_| {});
        self.renderer.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(std::time::Duration::from_secs(5)),
        })?;
        let bgra = matches!(
            self.config.format,
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
        );
        let pixels: Vec<u8> = buffer
            .get_mapped_range(..)?
            .chunks_exact(row as usize)
            .flat_map(|line| line[..size[0] as usize * 4].chunks_exact(4))
            .flat_map(|pixel| {
                if bgra {
                    [pixel[2], pixel[1], pixel[0], pixel[3]]
                } else {
                    [pixel[0], pixel[1], pixel[2], pixel[3]]
                }
            })
            .collect();
        let partial = path.with_extension("partial");
        let mut encoder = png::Encoder::new(std::fs::File::create(&partial)?, size[0], size[1]);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header()?.write_image_data(&pixels)?;
        std::fs::rename(partial, path)?;
        Ok(())
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
            && self.ui.box_at(self.pointer) == Some(strip())
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
        // On macOS the menu bar takes the chords of the items it enables first.
        if let ui::Event::Key { key, .. } = &event
            && let Some(id) =
                commands::find(&ui::edit_key(key), ui::edit_modifiers(self.ui.modifiers()))
        {
            self.choose(commands::Choice::Command(id));
        } else {
            self.ui.event(event);
        }
        self.window.request_redraw();
    }
}

/// The interface's colours in `appearance`, on white pages when `light_pages`, and over the
/// system's material where it shows through as a `backdrop`.
fn theme(appearance: winit::window::Theme, light_pages: bool, backdrop: bool) -> Theme {
    let mut theme = match appearance {
        winit::window::Theme::Dark => Theme::dark(),
        winit::window::Theme::Light => Theme::light(),
    };
    if light_pages {
        let light = Theme::light();
        [theme.paper, theme.paper_ink] = [light.paper, light.paper_ink];
    }
    if backdrop {
        theme.over_backdrop()
    } else {
        theme
    }
}

/// A section's colour as linear RGBA; sections without one take OneNote's default blue.
fn section_color(color: Option<u32>) -> [f32; 4] {
    color.map_or(draw::srgb(0x8a, 0xa8, 0xe4), canvas::gpu::colorref)
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
    for (part, text) in [("said", said), ("action", action)] {
        ui.leaf(
            part,
            Spec {
                size: [fill(), fit()],
                text: Some(text),
                overflow: ui::Overflow::Wrap,
                color: Some(draw::srgb(0x20, 0x20, 0x20)),
                ..Spec::default()
            },
        );
    }
    let action = ui.id("action");
    ui.close();
    ui.close();
    let clicked = ui.signal(row).clicked;
    let menu = Id::ROOT.child("conflict-menu");
    let copy = Id::ROOT.child("conflict-copy");
    // The menu opens under the line the click asks for, as wide as its items.
    let [left, top, _, bottom] = ui.rect(action).unwrap_or_default();
    let anchor = ui::Anchor::Below([left, top, left, bottom]);
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
                    ..Default::default()
                },
                ui::popup::Item {
                    text: "Copy Page To…",
                    disabled: sections.is_empty(),
                    ..Default::default()
                },
                ui::popup::Item {
                    text: "Select Previous Conflicting Change",
                    disabled: !steps[0],
                    separated: true,
                    ..Default::default()
                },
                ui::popup::Item {
                    text: "Select Next Conflicting Change",
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

/// The page list's rows, a page's conflict pages beneath it while shown and the pages a
/// search `found` marked. A `dragged` page's tab follows the pointer over the others, which
/// slide aside to open the gap it would land in; once let go it eases into its place.
#[allow(clippy::too_many_arguments)]
fn page_rows(
    ui: &mut Ui,
    theme: &Theme,
    section: &ui::Section,
    session: &Session,
    found: &HashSet<ExGuid>,
    rounding: f32,
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
            found,
            rounding,
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
            found,
            rounding,
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
    found: &HashSet<ExGuid>,
    rounding: f32,
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
    let (signal, kept) = page_tab(ui, theme, section, space, tab, selected, rounding);
    rows.kept = rows.kept.or(kept);
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
    if session.shown_history == Some(*space) {
        for version in session.page_versions(*space) {
            let label = history::label(version);
            let tab = PageTab {
                label: &label,
                dim: false,
                indent: level.saturating_sub(1),
                conflicted: false,
                found: false,
                renaming: None,
                shift,
                lifted: false,
            };
            let selected = session.version == Some(version.context);
            if selected {
                rows.open = Some(ui.id(version.context));
            }
            height += ROW;
            if page_tab(ui, theme, &muted, &version.context, tab, selected, rounding)
                .0
                .clicked
                && !selected
            {
                rows.version = Some((*space, version.context));
            }
        }
    }
    if session.shown != Some(*space) {
        return height;
    }
    for version in versions {
        let label = match version.created {
            Some(created) => format!("{} {}", platform::short_date(created), version.user),
            None => version.user.clone(),
        };
        let tab = PageTab {
            label: &label,
            dim: false,
            indent: level.saturating_sub(1),
            conflicted: false,
            found: false,
            renaming: None,
            shift,
            lifted: false,
        };
        let selected = version.space == session.space;
        if selected {
            rows.open = Some(ui.id(version.space));
        }
        height += ROW;
        if page_tab(ui, theme, &muted, &version.space, tab, selected, rounding)
            .0
            .clicked
            && !selected
        {
            rows.clicked = Some(version.space);
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
    /// The name typed in the tab's rename field, while it shows one.
    renaming: Option<&'a mut String>,
    /// How far down from its place in the list it is drawn.
    shift: f32,
    /// Dragged: lifted over the others with a shadow.
    lifted: bool,
}

/// A page's tab down the frame's right side: the open one is the page's colour and joins
/// it, the others float free of it as pills rounded like the page.
fn page_tab(
    ui: &mut Ui,
    theme: &Theme,
    section: &ui::Section,
    id: &ExGuid,
    tab: PageTab,
    selected: bool,
    rounding: f32,
) -> (ui::Signal, Option<bool>) {
    // A rename field's text stands where the label did.
    let pad = 10.0 + 16.0 * tab.indent as f32
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
        let spec = Spec {
            fill: Some(tint(section.tab)),
            hover_fill: Some(tint(section.hover())),
            radius: rounding,
            inset: [PILL_MARGIN, 0.0, 6.0, ROW_GAP],
            // The label keeps its place as the tab opens and closes.
            pad: [pad - PILL_MARGIN, 0.0],
            ..spec
        };
        let color = if tab.dim {
            ui::mix(theme.ink, section.tab, 0.5)
        } else {
            theme.ink
        };
        (spec, color)
    };
    let row = ui.open(id, spec);
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
    let groups = [(headings[0], recent.len()), (headings[1], count + 1)];
    let place = |index: usize| match index.checked_sub(recent.len()) {
        None => Some(recent[index]),
        Some(at) => at.checked_sub(1),
    };
    let chosen = ui::popup::gallery(
        ui,
        id,
        anchor,
        &groups[usize::from(recent.is_empty())..],
        5,
        size,
        current.map(|place| recent.len() + 1 + place),
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

/// A toolbar group's rows, stacked.
fn group(ui: &mut Ui, part: &str, rows: impl FnOnce(&mut Ui)) {
    ui.open(
        part,
        Spec {
            axis: Axis::Y,
            gap: 2.0,
            ..Spec::default()
        },
    );
    rows(ui);
    ui.close();
}

fn row(ui: &mut Ui, part: usize, buttons: impl FnOnce(&mut Ui)) {
    ui.open(
        part,
        Spec {
            gap: 1.0,
            ..Spec::default()
        },
    );
    buttons(ui);
    ui.close();
}

/// The line between two toolbar groups.
fn divider(ui: &mut Ui, part: &str, theme: &Theme) {
    ui.leaf(
        ("divider", part),
        Spec {
            size: [px(1.0), px(2.0 * ui::shell::TOOL + 2.0)],
            fill: Some(theme.chip),
            ..Spec::default()
        },
    );
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
    Ok((
        Session {
            section,
            library,
            tabs,
            tab,
            pages,
            space,
            status: "",
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

/// FILETIME now: when an edit happened, which its modification times record.
fn filetime() -> u64 {
    let unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    (unix.as_secs() + 11_644_473_600) * 10_000_000 + u64::from(unix.subsec_nanos() / 100)
}

impl App {
    fn close(&self, event_loop: &ActiveEventLoop) {
        if self.state.as_ref().is_none_or(|state| {
            let editor = &state.view.editor;
            state.session.is_some()
                || editor.caret_outline().is_none_or(TextOutline::is_empty)
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
                            .map(|outline| (&outline.id, outline.document())))
        }) || platform::confirm(
            "Discard this page?",
            "This temporary page has no saved copy. Closing it will discard your edits.",
            "Keep Editing",
            "Discard Changes",
        ) {
            event_loop.exit();
        }
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        let event = match event {
            UserEvent::InsertText(text) => {
                if let Some(state) = &mut self.state {
                    if state.ui.focused() == Some(page()) {
                        match state.view.insert_text(text) {
                            Ok(response) => state.respond(response),
                            Err(error) => eprintln!("{error}"),
                        }
                        state.window.request_redraw();
                    } else {
                        state.input(ui::Event::Ime(Ime::Commit(text)));
                    }
                }
                return;
            }
            UserEvent::Quit => {
                self.close(event_loop);
                return;
            }
            UserEvent::Sync => {
                if let Some(state) = &mut self.state
                    && let Err(error) = state.synced()
                {
                    eprintln!("{error}");
                }
                return;
            }
            UserEvent::Choose(choice) => {
                if let Some(state) = &mut self.state {
                    state.choose(choice);
                }
                return;
            }
            UserEvent::Appearance => {
                if let Some(state) = &mut self.state {
                    state.follow_color_scheme();
                    state.window.request_redraw();
                }
                return;
            }
            UserEvent::Redraw => {
                if let Some(state) = &self.state {
                    state.window.request_redraw();
                }
                return;
            }
            UserEvent::Replay(replay) => {
                if let Some(state) = &mut self.state {
                    match replay {
                        Replay::Input(event) => state.input(event),
                        Replay::Snapshot(path) => state.snapshot = Some(path),
                        Replay::Tick => {}
                        Replay::Appearance(appearance) => {
                            state.window.set_theme(Some(appearance));
                            state.set_appearance(appearance);
                        }
                        Replay::Quit => {
                            self.close(event_loop);
                            return;
                        }
                    }
                    // A covered window gets no redraws, so each step draws its own frame.
                    if let Err(error) = state.frame() {
                        eprintln!("{error}");
                    }
                }
                return;
            }
            UserEvent::Accessibility(event) => event,
        };
        let Some(state) = &mut self.state else {
            return;
        };
        if event.window_id != state.window.id() {
            return;
        }
        let was_marked = state.view.editor.marked_range().is_some();
        let result = match event.window_event {
            accesskit_winit::WindowEvent::InitialTreeRequested => {
                state.accessibility.deactivate();
                state.update_accessibility()
            }
            accesskit_winit::WindowEvent::ActionRequested(request) => state.access_action(request),
            accesskit_winit::WindowEvent::AccessibilityDeactivated => {
                state.accessibility.deactivate();
                Ok(())
            }
        };
        if was_marked && state.view.editor.marked_range().is_none() {
            platform::clear_marked_text(&state.window);
        }
        if let Err(error) = result {
            eprintln!("{error}");
        }
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        match pollster::block_on(State::new(
            event_loop,
            self.proxy.clone(),
            self.input.take().unwrap(),
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
                _ => self.state = Some(state),
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
                        state.config.width = (size.width as f32 * ratio) as u32;
                        state.config.height = (size.height as f32 * ratio) as u32;
                        state
                            .surface
                            .configure(&state.renderer.device, &state.config);
                    }
                    // Present inside AppKit's resize transaction; a redraw on the next turn
                    // lets the window show the previous frame at the new size.
                    return state.frame();
                }
                WindowEvent::ThemeChanged(appearance) => {
                    state.set_appearance(state.color_scheme.theme().unwrap_or(appearance));
                    state.window.request_redraw();
                }
                WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                    state.renderer.clear_glyph_cache();
                    state.app_icon = platform::app_icon((16.0 * scale_factor).round() as u32);
                    state.window.request_redraw();
                }
                WindowEvent::Focused(focused) => {
                    state.ui.window_focused = focused;
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
                    state.input(ui::Event::PointerMoved([
                        position.x as f32 / scale,
                        position.y as f32 / scale,
                    ]))
                }
                WindowEvent::MouseInput {
                    state: pressed,
                    button,
                    ..
                } => state.input(ui::Event::Button {
                    button,
                    pressed: pressed == ElementState::Pressed,
                    at: Instant::now(),
                }),
                WindowEvent::MouseWheel { delta, .. } => {
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
}

/// Feeds a development script to the window from another thread, one command per line
/// in logical pixels: `move X Y`, `press [right]`, `release [right]`, `wheel DX DY`, `key NAME`, `type
/// TEXT`, `modifiers [shift] [command]`, `wait MILLISECONDS`, `snapshot PNG_PATH`,
/// `appearance light|dark` and `quit`.
fn replay(script: String, proxy: EventLoopProxy<UserEvent>) -> Result<(), Box<dyn Error>> {
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
            "press" => Ok(Replay::Input(button(true))),
            "release" => Ok(Replay::Input(button(false))),
            "key" => Ok(Replay::Input(ui::Event::Key {
                key: match rest {
                    "Escape" => Key::Named(NamedKey::Escape),
                    "Enter" => Key::Named(NamedKey::Enter),
                    "Backspace" => Key::Named(NamedKey::Backspace),
                    "Tab" => Key::Named(NamedKey::Tab),
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
            "snapshot" => Ok(Replay::Snapshot(rest.into())),
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
                    let _ = proxy.send_event(UserEvent::Replay(replay));
                }
                Err(duration) => {
                    let start = Instant::now();
                    while start.elapsed() < duration {
                        std::thread::sleep(std::time::Duration::from_millis(16));
                        let _ = proxy.send_event(UserEvent::Replay(Replay::Tick));
                    }
                }
            }
        }
    });
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
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
            let bytes = std::fs::read(path)?;
            let store = onestore::Store::parse(&bytes)?;
            let index = onestore::RevisionIndex::parse(&store)?;
            reference = Some(Page::from_document(
                &onestore::document::Document::parse(&index)?,
                title
                    .to_str()
                    .ok_or("The page title must be valid Unicode.")?,
            )?);
        } else {
            positional.push(arg);
        }
    }
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
            "Usage: snowbound [TEXT_FILE] [WIDTH_POINTS] [--reference SECTION PAGE_TITLE | --page SECTION PAGE_TITLE | --section SECTION PAGE_TITLE | --notebook FOLDER] [--cache DIR] [--settings FILE] [--screenshot PNG_PREFIX] [--substitute-font FONT_FILE]..."
                .into(),
        );
    }
    let text = positional
        .first()
        .map(std::fs::read_to_string)
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
    let settings_file = settings_file.or_else(settings::default_path);
    let saved = settings_file
        .as_deref()
        .map(settings::Settings::load)
        .unwrap_or_default();
    let input = if let Some((file, title)) = section {
        Input::Section { file, title }
    } else if editable {
        Input::Page(reference.unwrap())
    } else if positional.is_empty() && reference.is_none() {
        let mut locations = saved.notebooks.clone();
        let mut current = saved.current.clone();
        if let Some(root) = notebook {
            let location = std::path::absolute(root)?.to_string_lossy().into_owned();
            if !locations.contains(&location) {
                locations.push(location.clone());
            }
            current = Some(location);
        }
        Input::Notebooks { locations, current }
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
    let event_loop = platform::event_loop(screenshot.is_some())?;
    if let Some(script) = std::env::var_os("SNOWBOUND_REPLAY") {
        replay(std::fs::read_to_string(script)?, event_loop.create_proxy())?;
    }
    let mut app = App {
        proxy: event_loop.create_proxy(),
        input: Some(input),
        // A screenshot leaves the settings as it found them.
        launch: Some(settings::Launch {
            file: settings_file.filter(|_| screenshot.is_none()),
            saved,
            cache,
        }),
        substitutes,
        screenshot,
        state: None,
        startup_error: None,
    };
    event_loop.run_app(&mut app)?;
    app.startup_error.map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use canvas::document::TextPosition;

    /// The ops an editing session records reach the section through `apply` and read back
    /// as the editor's page.
    #[test]
    fn recorded_ops_reach_the_section() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let directory = std::env::temp_dir().join(format!("snowbound-ops-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(directory.join("cache")).unwrap();
        let file = directory.join("section.one");
        std::fs::copy(
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
        std::fs::remove_dir_all(&directory).unwrap();
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
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        let file = directory.join("section.one");
        std::fs::copy(
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
        std::fs::remove_dir_all(&directory).unwrap();
    }
}
