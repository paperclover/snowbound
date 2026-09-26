mod art;
mod macos;

use canvas::gpu::page::PageScene;
use canvas::interaction::{Cursor, PageView, Request, Response, TextColors, accessibility};
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
use onestore::ExGuid;
use onestore::document::Format;
use onestore::page::Page;
use onestore::page::text::Paragraph;
use std::{
    error::Error,
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
    time::Instant,
};
use ui::{Axis, Flags, Id, Spec, Theme, Ui, children, fill, fit, px};
use winit::{
    application::ApplicationHandler,
    dpi::{LogicalSize, PhysicalPosition, PhysicalSize},
    event::{ElementState, Ime, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoopProxy},
    keyboard::{Key, ModifiersState, NamedKey},
    platform::macos::WindowAttributesExtMacOS,
    window::{CursorIcon, Window, WindowId},
};

/// Height of the macOS title bar, which holds the traffic lights.
const TITLE: f32 = 28.0;
/// Room the traffic lights take at the title bar's leading edge.
const LIGHTS: f32 = 78.0;
const TAB_ROW: f32 = 28.0;
/// Width of the section colour around the page.
const FRAME: f32 = 6.0;
/// macOS rounds windows' corners by 10 pt; the page's corners share their centres.
const ROUNDING: f32 = 10.0 - FRAME;
const PAGE_LIST: f32 = 240.0;
/// Font sizes the size box offers, OneNote's list in points.
const SIZES: [f32; 17] = [
    8.0, 9.0, 10.0, 10.5, 11.0, 12.0, 14.0, 16.0, 18.0, 20.0, 22.0, 24.0, 26.0, 28.0, 36.0, 48.0,
    72.0,
];
/// OneNote's highlight colours, COLORREF.
const HIGHLIGHTS: &[u32] = &[
    0x00ffff, 0x00ff00, 0xffff00, 0xff00ff, 0xff0000, 0x0000ff, 0x800000, 0x808000, 0x008000,
    0x800080, 0x000080, 0x008080, 0x808080, 0xc0c0c0, 0x000000,
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
/// Rounding where the open page tab meets the page, concentric with its neighbours'.
const JOIN: f32 = PILL_MARGIN + ROUNDING;

#[derive(Debug)]
enum UserEvent {
    Quit,
    InsertText(String),
    Accessibility(accesskit_winit::Event),
    /// The section's synchronization thread reported an event.
    Sync,
    /// Scripted input from `SNOWBOUND_REPLAY`.
    Replay(Replay),
    /// A page background finished rasterizing on its worker thread.
    Redraw,
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
    /// Font files for the interface in place of the system's.
    interface_font: Vec<Vec<u8>>,
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
        cache: PathBuf,
    },
    Notebook {
        root: PathBuf,
        cache: PathBuf,
    },
}

/// A section tab: where the section opens from and how it is labelled.
struct Tab {
    /// The notebook catalog path, or the file of a section opened on its own.
    path: String,
    name: String,
    /// COLORREF.
    color: Option<u32>,
}

/// The sections the tabs offer.
struct Library {
    /// The notebook's folder name, or the section file's.
    name: String,
    notebook: Option<notebook::session::Notebook>,
    tabs: Vec<Tab>,
    cache: PathBuf,
}

impl Library {
    fn open(
        &self,
        tab: usize,
        proxy: EventLoopProxy<UserEvent>,
    ) -> Result<notebook::session::Section, Box<dyn Error>> {
        let notify = move || {
            let _ = proxy.send_event(UserEvent::Sync);
        };
        let path = &self.tabs[tab].path;
        Ok(match &self.notebook {
            Some(notebook) => notebook.section(path, notify)?,
            None => notebook::session::Section::open(path, &self.cache, notify)?,
        })
    }
}

/// The open section and the page the editor shows.
struct Session {
    section: notebook::session::Section,
    tab: usize,
    /// Spaces, titles and outline levels in section order.
    pages: Vec<(ExGuid, String, u32)>,
    space: ExGuid,
    status: &'static str,
    /// The editor's page has an unreviewed conflict with another machine's change.
    conflict: bool,
}

impl Session {
    fn title(&self) -> &str {
        self.pages
            .iter()
            .find(|(space, ..)| *space == self.space)
            .map_or("", |(_, title, _)| title)
    }

    fn refresh_conflict(&mut self) -> Result<(), Box<dyn Error>> {
        self.conflict = self
            .section
            .conflict()?
            .is_some_and(|conflict| conflict.space == self.space);
        Ok(())
    }
}

/// What a loader thread opened: a section, or another page of the open one.
enum Loaded {
    Section(Box<Session>),
    Page(ExGuid),
}

/// A loader thread's request number, what it opened and the page it read.
type Read = (u64, Result<(Loaded, Page), String>);

/// The newest page read, laid out and waiting for the pictures it shows first, so it
/// never appears without them.
struct Opening {
    loaded: Loaded,
    scene: (PageScene, [f32; 2]),
    editor: CanvasEditor,
    since: Instant,
}

/// How long an opening page waits for its pictures before showing without them.
const HOLD: std::time::Duration = std::time::Duration::from_millis(200);

/// Work the interface asked for, done after the frame is built.
enum Command {
    OpenSection(usize),
    OpenPage(ExGuid),
    Resolve { keep_mine: bool },
    Page(Request),
}

struct State {
    window: Arc<Window>,
    proxy: EventLoopProxy<UserEvent>,
    /// Results of reads done off the frame thread, by request number.
    loads: (mpsc::Sender<Read>, mpsc::Receiver<Read>),
    /// The newest read requested; older ones are dropped when they finish.
    loading: u64,
    opening: Option<Opening>,
    /// Asks for a frame when a worker thread finishes something the page shows.
    redraw: std::task::Waker,
    instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    renderer: Renderer,
    ui: Ui,
    view: PageView,
    library: Option<Arc<Library>>,
    session: Option<Session>,
    /// Text filtering the page list.
    filter: String,
    /// Whether the page list is shown beside the page.
    pages_open: bool,
    /// Installed font families, for the font box; listing them is slow.
    fonts: Vec<String>,
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
    /// Where a replay asked the next frame to be written.
    snapshot: Option<PathBuf>,
    initial: Vec<(onestore::ExGuid, TextDocument)>,
    initial_layouts: Vec<(onestore::ExGuid, onestore::document::Layout)>,
    initial_date: Option<u64>,
    occluded: bool,
    ime_allowed: bool,
    clipboard: arboard::Clipboard,
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

/// The field filtering the page list.
fn filter() -> Id {
    Id::ROOT.child("filter")
}

impl State {
    async fn new(
        event_loop: &ActiveEventLoop,
        proxy: EventLoopProxy<UserEvent>,
        input: Input,
        substitutes: &[PathBuf],
        interface_font: Vec<Vec<u8>>,
    ) -> Result<Self, Box<dyn Error>> {
        let window = Arc::new(
            event_loop.create_window(
                Window::default_attributes()
                    .with_visible(false)
                    .with_titlebar_transparent(true)
                    .with_title_hidden(true)
                    .with_fullsize_content_view(true)
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
                        Input::Notebook { root, .. } => root.display().to_string(),
                    })
                    .with_inner_size(LogicalSize::new(1180.0, 760.0)),
            )?,
        );
        macos::install_text_input(&window);
        let access_adapter =
            accesskit_winit::Adapter::with_event_loop_proxy(event_loop, &window, proxy.clone());
        window.set_visible(true);
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle(
            Box::new(window.clone()),
        ));
        let surface = instance.create_surface(window.clone())?;
        macos::configure_presentation(&surface);
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                compatible_surface: Some(&surface),
                ..Default::default()
            })
            .await?;
        let (device, queue) = adapter.request_device(&Default::default()).await?;
        let size = window.inner_size();
        let config = surface
            .get_default_config(&adapter, size.width, size.height)
            .ok_or("No supported canvas surface")?;
        surface.configure(&device, &config);
        let renderer = Renderer::new(device, queue, config.format);
        let mut engine = TextEngine::default();
        for path in substitutes {
            let target = engine
                .register_substitute(parley::fontique::Blob::new(Arc::new(std::fs::read(path)?)))?;
            eprintln!("Using {} for {target}", path.display());
        }
        let mut library = None;
        let mut session = None;
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
            Input::Section { file, title, cache } => {
                let opened = Library {
                    name: file_name(file.parent().unwrap_or(&file)),
                    notebook: None,
                    tabs: vec![Tab {
                        name: file
                            .file_stem()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                        path: file.to_string_lossy().into_owned(),
                        color: None,
                    }],
                    cache,
                };
                let section = opened.open(0, proxy.clone())?;
                let space = section
                    .pages()?
                    .into_iter()
                    .find(|(_, candidate, _)| *candidate == title)
                    .ok_or_else(|| format!("No page titled {title:?} in {}", file.display()))?
                    .0;
                library = Some(Arc::new(opened));
                let (opened, page) = read_session(section, 0, Some(space))?;
                let (scene, editor) = PageScene::from_page(page, &mut engine)?;
                session = Some(opened);
                (editor, Some((scene, [0.0; 2])))
            }
            Input::Notebook { root, cache } => {
                let notebook = notebook::session::Notebook::open(&root, &cache)?;
                let tabs = tabs(&notebook);
                if tabs.is_empty() {
                    return Err(format!("No readable sections in {}", root.display()).into());
                }
                let opened = Library {
                    name: file_name(&root),
                    notebook: Some(notebook),
                    tabs,
                    cache,
                };
                let section = opened.open(0, proxy.clone())?;
                library = Some(Arc::new(opened));
                let (opened, page) = read_session(section, 0, None)?;
                let (scene, editor) = PageScene::from_page(page, &mut engine)?;
                session = Some(opened);
                (editor, Some((scene, [0.0; 2])))
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
        let mut ui = Ui::new(
            theme(window.theme().unwrap_or(winit::window::Theme::Dark)),
            macos::double_click_interval(),
        );
        if !interface_font.is_empty() {
            let family = ui
                .use_fonts(interface_font)
                .ok_or("The interface font files hold no font")?;
            eprintln!("Using {family} for the interface");
        }
        ui.set_focus(Some(page()));
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
        let state = Self {
            window,
            redraw: Arc::new(Redraw(proxy.clone())).into(),
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
                macos::double_click_interval(),
            ),
            library,
            session,
            filter: String::new(),
            pages_open: true,
            fonts,
            app_icon: macos::app_icon((16.0 * dpr).round() as u32),
            commands: Vec::new(),
            changed: false,
            moved: false,
            stale: false,
            page_focused: true,
            pointer: [0.0; 2],
            strip_press: None,
            snapshot: None,
            initial,
            initial_date,
            initial_layouts,
            occluded: false,
            ime_allowed: true,
            clipboard: arboard::Clipboard::new()?,
            loads: mpsc::channel(),
            loading: 0,
            opening: None,
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
        let scale = self.window.scale_factor() as f32;
        self.ui.begin(
            [size.width as f32 / scale, size.height as f32 / scale],
            scale,
            Instant::now(),
        );
        let (section, open_tab, open_page) = self.build()?;
        self.ui.end();
        self.edges(section, open_tab, open_page);
        lap("build", start);
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
        self.window.set_cursor(
            self.ui
                .cursor()
                .unwrap_or_else(|| cursor_icon(self.view.cursor())),
        );
        let ime = if self.ui.focused() == Some(page()) {
            self.view.accepts_text()
        } else {
            self.ui.focused() == Some(filter())
        };
        if self.ime_allowed != ime {
            self.ime_allowed = ime;
            self.window.set_ime_allowed(ime);
        }
        self.draw()?;
        lap("drawn", start);
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

    /// Builds the frame's boxes and returns the section's colours as they ease, the open
    /// section tab and the open page's tab, for the edges drawn once they are laid out.
    fn build(&mut self) -> Result<(ui::Section, Id, Option<Id>), Box<dyn Error>> {
        let target = self.ui.theme.section(match (&self.library, &self.session) {
            (Some(library), Some(session)) => section_color(library.tabs[session.tab].color),
            _ => section_color(None),
        });
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
        self.ui.theme.accent = section.accent;
        [
            self.ui.theme.caret,
            self.ui.theme.selection,
            self.ui.theme.inactive_selection,
        ] = macos::text_colors(&self.window);
        let theme = self.ui.theme.clone();
        self.title_bar(&theme);
        self.toolbar(&theme)?;
        self.ui.open(
            "tabs",
            Spec {
                size: [fill(), px(TAB_ROW)],
                fill: Some(theme.strip),
                pad: [FRAME, 0.0],
                ..Spec::default()
            },
        );
        self.ui.leaf(
            "corner",
            Spec {
                size: [px(ROUNDING), px(1.0)],
                ..Spec::default()
            },
        );
        let (clicked, open_tab) = match (&self.library, &self.session) {
            (Some(library), Some(session)) => {
                let tabs: Vec<_> = library
                    .tabs
                    .iter()
                    .map(|tab| (tab.name.as_str(), section_color(tab.color)))
                    .collect();
                ui::shell::section_tabs(
                    &mut self.ui,
                    "sections",
                    &tabs,
                    session.tab,
                    &section,
                    TAB_ROW,
                )
            }
            _ => ui::shell::section_tabs(
                &mut self.ui,
                "sections",
                &[("Temporary page", section_color(None))],
                0,
                &section,
                TAB_ROW,
            ),
        };
        if let Some(index) = clicked {
            self.commands.push(Command::OpenSection(index));
        }
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
        if self
            .session
            .as_ref()
            .is_some_and(|session| session.conflict)
        {
            self.conflict_bar(&theme);
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
        let open_page = self.page_list(&theme, &section);
        self.ui.close();
        Ok((section, open_tab, open_page))
    }

    /// Borders the open section tab and the frame's top, and rounds and borders the page
    /// together with the open page's tab, joined where they meet.
    fn edges(&mut self, section: ui::Section, open_tab: Id, open_page: Option<Id>) {
        let panel = self.ui.rect(frame().child("panel"));
        let (Some(frame), Some(page)) = (self.ui.rect(frame()), self.ui.rect(page())) else {
            return;
        };
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
                (reach - right > 2.0 * JOIN && end - start > 2.0 * ROUNDING).then(|| {
                    // A tab too near the page's corner joins it along the edge.
                    let start = if start < top + ROUNDING + JOIN {
                        top
                    } else {
                        start
                    };
                    let end = if end > bottom - ROUNDING - JOIN {
                        bottom
                    } else {
                        end
                    };
                    [start, end, reach]
                })
            });
        let mut outline = vec![([left, top], ROUNDING)];
        match tab {
            Some([start, end, reach]) => {
                if start > top {
                    outline.extend([([right, top], ROUNDING), ([right, start], JOIN)]);
                }
                outline.extend([([reach, start], ROUNDING), ([reach, end], ROUNDING)]);
                if end < bottom {
                    outline.extend([([right, end], JOIN), ([right, bottom], ROUNDING)]);
                }
            }
            None => outline.extend([([right, top], ROUNDING), ([right, bottom], ROUNDING)]),
        }
        outline.push(([left, bottom], ROUNDING));
        let height = (frame[3] - frame[1]).max(1.0);
        let paper = self.ui.theme.paper;
        self.ui.round_corners(&outline, paper, |y| {
            ui::mix(section.frame[0], section.frame[1], (y - frame[1]) / height)
        });
        self.ui.border(&outline, true, section.edge);
        // The frame's top corners share the page's centres; its top edge breaks where the
        // open tab stands on it.
        let [start, end] = [frame[0], frame[2]];
        let outer = FRAME + ROUNDING;
        let strip = self.ui.theme.strip;
        self.ui.round_corners(
            &[
                ([start, frame[1]], outer),
                ([end, frame[1]], outer),
                ([end, frame[3]], 0.0),
                ([start, frame[3]], 0.0),
            ],
            paper,
            |_| strip,
        );
        let [foot, toe] = self
            .ui
            .rect(open_tab)
            .map_or([end; 2], |tab| ui::shell::tab_base(tab, TAB_ROW));
        for edge in [
            [
                ([start, frame[1] + outer], 0.0),
                ([start, frame[1]], outer),
                ([foot, frame[1]], 0.0),
            ],
            [
                ([toe, frame[1]], 0.0),
                ([end, frame[1]], outer),
                ([end, frame[1] + outer], 0.0),
            ],
        ] {
            self.ui.border(&edge, false, section.edge);
        }
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
                size: [px(LIGHTS), px(1.0)],
                ..Spec::default()
            },
        );
        self.ui.leaf(
            "notebook",
            Spec {
                size: [fit(), px(TITLE)],
                image: self.app_icon.as_ref(),
                text: Some(
                    self.library
                        .as_ref()
                        .map_or("Temporary page", |library| &library.name),
                ),
                ..Spec::default()
            },
        );
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
                    text: Some(session.status),
                    color: Some(theme.text_dim),
                    pad: [12.0, 0.0],
                    ..Spec::default()
                },
            );
        }
        self.ui.close();
    }

    /// Two rows of formatting, tag, insert and zoom buttons, grouped as OneNote's Home
    /// ribbon groups them.
    fn toolbar(&mut self, theme: &Theme) -> Result<(), Box<dyn Error>> {
        use canvas::editor::{Alignment, FormatState, Formatting, NoteTag, Toggle};
        // A focused picture has no text to show a format for.
        let state = self
            .view
            .editor
            .format_state()
            .unwrap_or_else(|_| FormatState {
                toggles: Vec::new(),
                font: None,
                font_size: None,
                alignment: None,
                bullets: false,
                numbering: false,
                tags: Vec::new(),
            });
        let fonts = &self.fonts;
        let ui = &mut self.ui;
        let text = theme.text;
        let on = |toggle| state.toggles.contains(&toggle);
        let mut command = None;
        let mut history = None;
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
                history = Some(true);
            }
            if ui::shell::tool_button(ui, "redo", art::REDO, text, false).clicked {
                history = Some(false);
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
                let items: Vec<_> = fonts
                    .iter()
                    .map(|name| ui::popup::Item {
                        text: name,
                        font: Some(name),
                        checked: *name == font,
                        ..Default::default()
                    })
                    .collect();
                let anchor = ui::Anchor::Over(ui.rect(combo).unwrap_or_default());
                if let Some(index) = ui::popup::menu(ui, popup("font"), anchor, &items, Some("Font"))
                {
                    command = Some(Formatting::Font(fonts[index].clone()));
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
                    command = Some(Formatting::FontSize(sizes[index]));
                }
                let bullets =
                    ui::shell::split_button(ui, "bullets", art::BULLETS, None, state.bullets);
                let numbering =
                    ui::shell::split_button(ui, "numbering", art::NUMBERING, None, state.numbering);
                if bullets.iter().any(|signal| signal.clicked) {
                    command = Some(Formatting::Bullets);
                }
                if numbering.iter().any(|signal| signal.clicked) {
                    command = Some(Formatting::Numbering);
                }
                if ui::shell::tool_button(ui, "clear", art::CLEAR_FORMATTING, text, false).clicked {
                    command = Some(Formatting::Clear);
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
                        command = Some(Formatting::Toggle(toggle));
                    }
                }
                let script = ui.id("script");
                let superscript = on(Toggle::Superscript);
                let [button, menu] = ui::shell::split_button(
                    ui,
                    "script",
                    if superscript {
                        art::SUPERSCRIPT
                    } else {
                        art::SUBSCRIPT
                    },
                    None,
                    on(Toggle::Subscript) || superscript,
                );
                if button.clicked {
                    command = Some(Formatting::Toggle(if superscript {
                        Toggle::Superscript
                    } else {
                        Toggle::Subscript
                    }));
                }
                if menu.pressed {
                    ui.open_popup(popup("script"));
                }
                let items = [
                    ui::popup::Item {
                        text: "Subscript",
                        icon: Some(art::SUBSCRIPT),
                        shortcut: "⌘=",
                        checked: on(Toggle::Subscript),
                        ..Default::default()
                    },
                    ui::popup::Item {
                        text: "Superscript",
                        icon: Some(art::SUPERSCRIPT),
                        shortcut: "⇧⌘=",
                        checked: superscript,
                        ..Default::default()
                    },
                ];
                let anchor = ui::Anchor::Below(ui.rect(script).unwrap_or_default());
                if let Some(index) = ui::popup::menu(ui, popup("script"), anchor, &items, None) {
                    command = Some(Formatting::Toggle(
                        [Toggle::Subscript, Toggle::Superscript][index],
                    ));
                }
                for (part, icon, swatches, columns, none, default) in [
                    (
                        "highlight",
                        art::HIGHLIGHTER,
                        HIGHLIGHTS,
                        5,
                        "No colour",
                        0x00ffff,
                    ),
                    (
                        "color",
                        art::FONT_COLOR,
                        &FONT_COLORS[..],
                        10,
                        "Automatic",
                        0x3a3ae8,
                    ),
                ] {
                    let split = ui.id(part);
                    let [button, menu] =
                        ui::shell::split_button(ui, part, icon, Some(colorref(default)), false);
                    let paint = |color| {
                        if part == "highlight" {
                            Formatting::Highlight(color)
                        } else {
                            Formatting::Color(color)
                        }
                    };
                    if button.clicked {
                        command = Some(paint(Some(default)));
                    }
                    if menu.pressed {
                        ui.open_popup(popup(part));
                    }
                    let colors: Vec<_> = swatches.iter().map(|color| colorref(*color)).collect();
                    let anchor = ui::Anchor::Below(ui.rect(split).unwrap_or_default());
                    if let Some(chosen) =
                        ui::popup::colors(ui, popup(part), anchor, none, &colors, columns)
                    {
                        command = Some(paint(chosen.and_then(|chosen| {
                            swatches
                                .iter()
                                .zip(&colors)
                                .find(|(_, color)| **color == chosen)
                                .map(|(stored, _)| *stored)
                        })));
                    }
                }
                if ui::shell::tool_button(ui, "outdent", art::OUTDENT, text, false).clicked {
                    command = Some(Formatting::Outdent);
                }
                if ui::shell::tool_button(ui, "indent", art::INDENT, text, false).clicked {
                    command = Some(Formatting::Indent);
                }
                let align = ui.id("align");
                let alignments = [
                    (Alignment::Left, "Align left", art::ALIGN_LEFT),
                    (Alignment::Center, "Centre", art::ALIGN_CENTER),
                    (Alignment::Right, "Align right", art::ALIGN_RIGHT),
                ];
                let current = alignments
                    .iter()
                    .find(|(alignment, ..)| Some(*alignment) == state.alignment)
                    .unwrap_or(&alignments[0]);
                let [button, menu] = ui::shell::split_button(ui, "align", current.2, None, false);
                if button.pressed || menu.pressed {
                    ui.open_popup(popup("align"));
                }
                let items = alignments.map(|(alignment, name, icon)| ui::popup::Item {
                    text: name,
                    icon: Some(icon),
                    checked: Some(alignment) == state.alignment,
                    ..Default::default()
                });
                let anchor = ui::Anchor::Below(ui.rect(align).unwrap_or_default());
                if let Some(index) = ui::popup::menu(ui, popup("align"), anchor, &items, None) {
                    command = Some(Formatting::Align(alignments[index].0));
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
        group(ui, "tags", |ui| {
            for (index, tags) in tags.chunks(5).enumerate() {
                row(ui, index, |ui| {
                    for (column, (icon, tag)) in tags.iter().enumerate() {
                        let lit = state.tags.contains(tag);
                        if ui::shell::tool_button(ui, column, icon, [1.0; 4], lit).clicked {
                            command = Some(Formatting::Tag(*tag));
                        }
                    }
                    if index == 1 {
                        ui::shell::tool_button(ui, "more", ui::shell::CHEVRON, text, false);
                    }
                });
            }
        });
        divider(ui, "tags", theme);
        group(ui, "insert", |ui| {
            row(ui, 0, |ui| {
                for (part, icon) in [
                    ("table", art::TABLE),
                    ("picture", art::PICTURE),
                    ("file", art::ATTACHMENT),
                    ("link", art::LINK),
                ] {
                    ui::shell::tool_button(ui, part, icon, text, false);
                }
            });
            row(ui, 1, |ui| {
                for (part, icon) in [
                    ("date", art::CALENDAR),
                    ("time", art::CLOCK),
                    ("equation", art::EQUATION),
                ] {
                    ui::shell::tool_button(ui, part, icon, text, false);
                }
            });
        });
        ui.leaf(
            "space",
            Spec {
                size: [fill(), px(1.0)],
                ..Spec::default()
            },
        );
        let zoom = self.view.zoom();
        let label = format!("{:.0}%", zoom * 100.0);
        let mut chosen = None;
        group(ui, "zoom", |ui| {
            row(ui, 0, |ui| {
                if ui::shell::tool_button(ui, "out", art::ZOOM_OUT, text, false).clicked {
                    chosen = Some(zoom / 1.1);
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
                    chosen = Some(1.0);
                }
                if ui::shell::tool_button(ui, "in", art::ZOOM_IN, text, false).clicked {
                    chosen = Some(zoom * 1.1);
                }
            });
        });
        ui.close();
        if let Some(zoom) = chosen {
            let response = self.view.set_zoom(zoom)?;
            self.respond(response);
        }
        if let Some(command) = command {
            let response = self.view.format(command)?;
            self.respond(response);
        }
        if let Some(undo) = history {
            let view = &mut self.view;
            let done = if undo {
                view.editor.undo(&mut view.engine)?
            } else {
                view.editor.redo(&mut view.engine)?
            };
            self.changed |= done;
        }
        Ok(())
    }

    /// The search box and the page list's buttons, above the list.
    fn page_tools(&mut self, theme: &Theme) {
        let Some(session) = &self.session else {
            return;
        };
        self.ui.open(
            "tools",
            Spec {
                size: [px(PAGE_LIST), px(TAB_ROW)],
                pad: [0.0, (TAB_ROW - ui::shell::TOOL) / 2.0],
                gap: 3.0,
                ..Spec::default()
            },
        );
        let focused = self.ui.focused() == Some(filter());
        self.ui.open(
            "search",
            Spec {
                size: [fill(), px(ui::shell::TOOL)],
                fill: Some(theme.base),
                border: Some(if focused { theme.accent } else { theme.chip }),
                radius: 4.0,
                pad: [6.0, 0.0],
                ..Spec::default()
            },
        );
        self.ui.leaf(
            "icon",
            Spec {
                size: [fit(), px(ui::shell::TOOL)],
                icon: Some(art::SEARCH),
                color: Some(theme.text_dim),
                ..Spec::default()
            },
        );
        let signal = ui::text_field(
            &mut self.ui,
            filter(),
            &mut self.filter,
            "Search pages",
            Spec {
                size: [fill(), px(ui::shell::TOOL)],
                pad: [6.0, 0.0],
                ..Spec::default()
            },
        );
        if !self.filter.is_empty()
            && ui::shell::tool_button(&mut self.ui, "clear", art::CLOSE, theme.text_dim, false)
                .clicked
        {
            self.filter.clear();
        }
        self.ui.close();
        for event in &signal.events {
            if let ui::Event::Key {
                key: Key::Named(key @ (NamedKey::Escape | NamedKey::Enter)),
                ..
            } = event
            {
                let first = matching(&session.pages, &self.filter).next();
                if *key == NamedKey::Enter
                    && let Some((space, ..)) = first.filter(|(space, ..)| *space != session.space)
                {
                    self.commands.push(Command::OpenPage(*space));
                }
                self.filter.clear();
                self.ui.set_focus(Some(page()));
            }
        }
        ui::shell::tool_button(&mut self.ui, "new", art::NEW_PAGE, theme.text, false);
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

    fn conflict_bar(&mut self, theme: &Theme) {
        self.ui.open(
            "conflict",
            Spec {
                size: [fill(), px(40.0)],
                fill: Some(theme.panel),
                pad: [12.0, 7.0],
                gap: 8.0,
                ..Spec::default()
            },
        );
        self.ui.leaf(
            "message",
            Spec {
                size: [fit(), px(26.0)],
                text: Some("This page also changed on another computer."),
                ..Spec::default()
            },
        );
        self.ui.leaf(
            "space",
            Spec {
                size: [fill(), px(1.0)],
                ..Spec::default()
            },
        );
        if ui::button(&mut self.ui, "mine", "Keep mine").clicked {
            self.commands.push(Command::Resolve { keep_mine: true });
        }
        if ui::button(&mut self.ui, "theirs", "Keep theirs").clicked {
            self.commands.push(Command::Resolve { keep_mine: false });
        }
        self.ui.close();
    }

    /// The section's pages as tabs down the frame's right side, returning the open page's
    /// tab, which is the page's colour and joins it.
    fn page_list(&mut self, theme: &Theme, section: &ui::Section) -> Option<Id> {
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
        let mut open = None;
        for (space, title, level) in matching(&session.pages, &self.filter) {
            let selected = *space == session.space;
            let spec = Spec {
                flags: Flags::CLICKABLE,
                size: [px(PAGE_LIST), px(ROW)],
                text: Some(if title.is_empty() {
                    "Untitled page"
                } else {
                    title
                }),
                pad: [10.0 + 16.0 * level.saturating_sub(1) as f32, 0.0],
                ..Spec::default()
            };
            let spec = if selected {
                open = Some(self.ui.id(space));
                Spec {
                    color: Some(theme.paper_ink),
                    fill: Some(theme.paper),
                    inset: [0.0, 0.0, 0.0, ROW_GAP],
                    ..spec
                }
            } else {
                // Tabs not open float free of the page as pills; only the open one joins it.
                Spec {
                    color: Some(if title.is_empty() {
                        ui::mix(theme.ink, section.tab, 0.5)
                    } else {
                        theme.ink
                    }),
                    fill: Some(section.tab),
                    hover_fill: Some(ui::mix(section.tab, section.accent, 0.3)),
                    radius: ROUNDING,
                    inset: [PILL_MARGIN, 0.0, 6.0, ROW_GAP],
                    // The label keeps its place as the tab opens and closes.
                    pad: [spec.pad[0] - PILL_MARGIN, 0.0],
                    ..spec
                }
            };
            if self.ui.leaf(space, spec).clicked && !selected {
                self.commands.push(Command::OpenPage(*space));
            }
        }
        self.ui.close();
        open
    }

    /// Hands the page the events routed to its box, in its device pixels.
    fn page_events(&mut self, events: Vec<ui::Event>) -> Result<(), Box<dyn Error>> {
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
                ui::Event::Button { .. } | ui::Event::Ime(Ime::Enabled) => continue,
                ui::Event::Wheel(delta) => self.view.wheel([delta[0] * scale, delta[1] * scale])?,
                ui::Event::Key { key, text } => {
                    let modifiers = self.view.modifiers();
                    let find = matches!(&key, Key::Character(character)
                        if character.eq_ignore_ascii_case("f"));
                    if modifiers.command && !modifiers.shift && find && self.session.is_some() {
                        self.ui.set_focus(Some(filter()));
                        continue;
                    }
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
            macos::clear_marked_text(&self.window);
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
            Command::OpenSection(tab) => {
                let library = Arc::clone(self.library.as_ref().ok_or("No sections to open")?);
                let proxy = self.proxy.clone();
                self.load(move || {
                    let section = library.open(tab, proxy)?;
                    let (session, page) = read_session(section, tab, None)?;
                    Ok((Loaded::Section(Box::new(session)), page))
                });
            }
            Command::OpenPage(space) => {
                let session = self.session.as_ref().ok_or("No section is open")?;
                let replica = Arc::clone(session.section.replica());
                self.load(move || Ok((Loaded::Page(space), replica.page(space)?)));
            }
            Command::Resolve { keep_mine } => self.resolve_conflict(keep_mine)?,
            Command::Page(Request::EditDate(field)) => self.edit_date(field)?,
            Command::Page(Request::Copy(text)) => self.clipboard.set_text(text)?,
            Command::Page(Request::Paste) => {
                let text = self.clipboard.get_text()?;
                let language = canvas::language::lcid(&macos::input_language());
                let response = self.view.paste(&text, language)?;
                self.respond(response);
            }
            Command::Page(Request::CharacterPalette) => macos::show_character_palette(),
        }
        Ok(())
    }

    /// Runs `read` on a thread of its own; `open_loaded` shows what it read unless a newer
    /// read was asked for meanwhile.
    fn load(
        &mut self,
        read: impl FnOnce() -> Result<(Loaded, Page), Box<dyn Error>> + Send + 'static,
    ) {
        self.loading += 1;
        let (id, sender, redraw) = (self.loading, self.loads.0.clone(), self.redraw.clone());
        std::thread::spawn(move || {
            let _ = sender.send((id, read().map_err(|error| error.to_string())));
            redraw.wake();
        });
    }

    /// Lays out the newest page read, and shows it once the pictures it shows first are
    /// drawn, or after `HOLD`.
    fn open_loaded(&mut self) -> Result<(), Box<dyn Error>> {
        for (id, loaded) in self.loads.1.try_iter() {
            if id != self.loading {
                continue;
            }
            let (loaded, page) = loaded?;
            let (scene, editor) = PageScene::from_page(page, &mut self.view.engine)?;
            self.opening = Some(Opening {
                loaded,
                scene: (scene, [0.0; 2]),
                editor,
                since: Instant::now(),
            });
        }
        let Some(mut opening) = self.opening.take() else {
            return Ok(());
        };
        let paper = canvas::gpu::Paper {
            color: self.ui.theme.paper,
            ink: self.ui.theme.paper_ink,
        };
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
        match opening.loaded {
            Loaded::Section(session) => {
                self.session = Some(*session);
                self.filter.clear();
            }
            Loaded::Page(space) => {
                let session = self.session.as_mut().ok_or("No section is open")?;
                session.space = space;
                session.refresh_conflict()?;
            }
        }
        self.view.open(opening.editor, Some(opening.scene));
        self.opened()
    }

    /// Follows a page shown in place of another.
    fn opened(&mut self) -> Result<(), Box<dyn Error>> {
        self.ui.set_focus(Some(page()));
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
            PhysicalPosition::new(x0 + corner[0] * scale, y0 + corner[1] * scale),
            PhysicalSize::new(x1 - x0, y1 - y0),
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
        }
    }

    fn edit_date(&mut self, field: DateField) -> Result<(), Box<dyn Error>> {
        let Some(date) = self.view.editor.date() else {
            return Ok(());
        };
        let timestamp = date.timestamp();
        self.view.editor.finish_composition();
        macos::clear_marked_text(&self.window);
        if let Some((timestamp, text)) = macos::edit_date(timestamp, field)? {
            let response = self.view.change_date(timestamp, text)?;
            self.respond(response);
            self.window.request_redraw();
        }
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
        let space = session.space;
        session.section.apply(
            "snowbound",
            onestore::op::Edit {
                at: filetime(),
                ops: ops
                    .into_iter()
                    .map(|op| onestore::op::Op::Page { space, op })
                    .collect(),
            },
        )?;
        session.status = "Saving";
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
        let page = session.section.page(session.space)?;
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
        macos::clear_marked_text(&self.window);
        if let Some(session) = &mut self.session {
            session.status = "Not saving";
        }
        self.refresh()?;
        macos::change_not_saved();
        Ok(())
    }

    /// Ends the conflict on this page: `keep_mine` publishes the local page over the remote
    /// change, otherwise the remote page replaces it.
    fn resolve_conflict(&mut self, keep_mine: bool) -> Result<(), Box<dyn Error>> {
        self.persist()?;
        let Some(session) = &mut self.session else {
            return Ok(());
        };
        let Some(conflict) = session
            .section
            .conflict()?
            .filter(|conflict| conflict.space == session.space)
        else {
            return Ok(());
        };
        let resolution = if keep_mine {
            notebook::Resolution::Mine
        } else {
            notebook::Resolution::Theirs
        };
        session.section.resolve(conflict.id, resolution)?;
        session.status = "Saving";
        session.refresh_conflict()?;
        if keep_mine {
            self.window.request_redraw();
            Ok(())
        } else {
            self.refresh()
        }
    }

    /// Applies what the section reported since the last poll.
    fn synced(&mut self) -> Result<(), Box<dyn Error>> {
        let Some(session) = &mut self.session else {
            return Ok(());
        };
        let shown = (session.status, session.conflict);
        let mut listed = false;
        let mut changed = false;
        let mut rejected = None;
        let mut conflicted = session.conflict;
        for event in session.section.events() {
            use notebook::session::Event;
            session.status = match event {
                Event::Refreshed => {
                    listed = true;
                    continue;
                }
                Event::Changed(spaces) => {
                    listed = true;
                    changed |= spaces.contains(&session.space);
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
                Event::Attempt {
                    status: notebook::EditStatus::Conflict(_),
                    ..
                } => {
                    conflicted = true;
                    "Conflict"
                }
                Event::Attempt { .. } => "Saving",
                Event::Unreachable(_) => "Offline",
                Event::Failed(error) => {
                    eprintln!("Synchronization stopped: {error}");
                    "Not saving"
                }
            };
        }
        // Reading the queue waits on the section thread, so only a conflict shown or
        // reported sends the frame thread there.
        if conflicted {
            session.refresh_conflict()?;
        }
        if listed || changed || shown != (session.status, session.conflict) {
            self.window.request_redraw();
        }
        if listed {
            session.pages = session.section.pages()?;
            if !session
                .pages
                .iter()
                .any(|(space, ..)| *space == session.space)
            {
                let first = session.pages.first().ok_or("The section has no pages")?.0;
                self.commands.push(Command::OpenPage(first));
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
        let mut viewport = view.viewport;
        viewport.origin[0] += corner[0] * self.ui.scale();
        viewport.origin[1] += corner[1] * self.ui.scale();
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
                macos::configure_presentation(&self.surface);
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
        self.paint(&frame.texture.create_view(&Default::default()))?;
        self.window.pre_present_notify();
        let start = Instant::now();
        self.renderer.queue.present(frame);
        macos::commit_presentation(&self.window);
        lap("present", start);
        trace_input(&"Present submitted");
        if reconfigure {
            self.surface.configure(&self.renderer.device, &self.config);
        }
        Ok(())
    }

    /// Paints the interface with the page in its box.
    fn paint(&mut self, target: &wgpu::TextureView) -> Result<(), Box<dyn Error>> {
        let start = Instant::now();
        let paper = canvas::gpu::Paper {
            color: self.ui.theme.paper,
            ink: self.ui.theme.paper_ink,
        };
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
                ui::Layer::Primitives { clip, primitives } => draw::Layer {
                    scale,
                    origin: [0.0; 2],
                    clip: clip.map(|clip| clip.map(|value| value * scale)),
                    backdrop: None,
                    primitives,
                },
                ui::Layer::Custom { rect, .. } => draw::Layer {
                    scale: viewport.scale,
                    origin: [
                        viewport.origin[0] + corner[0] * scale,
                        viewport.origin[1] + corner[1] * scale,
                    ],
                    clip: Some(rect.map(|value| value * scale)),
                    backdrop: Some(self.ui.theme.paper),
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
                self.ui.theme.base,
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

    /// Queues input for the next frame. A press on the strip moves the window at once,
    /// while the platform still holds the press; a double press zooms it.
    fn input(&mut self, event: ui::Event) {
        if let ui::Event::PointerMoved(point) = event {
            self.pointer = point;
        }
        if let ui::Event::Button {
            button: MouseButton::Left,
            pressed: true,
            at,
        } = event
            && self.ui.box_at(self.pointer) == Some(strip())
        {
            let double = self.strip_press.is_some_and(|last| {
                at.saturating_duration_since(last) <= macos::double_click_interval()
            });
            self.strip_press = (!double).then_some(at);
            if double {
                macos::zoom(&self.window);
            } else {
                let _ = self.window.drag_window();
            }
            return;
        }
        self.ui.event(event);
        self.window.request_redraw();
    }
}

fn theme(appearance: winit::window::Theme) -> Theme {
    match appearance {
        winit::window::Theme::Dark => Theme::dark(),
        winit::window::Theme::Light => Theme::light(),
    }
}

fn file_name(path: &Path) -> String {
    path.canonicalize()
        .ok()
        .as_deref()
        .and_then(Path::file_name)
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// A section's colour as linear RGBA; sections without one take OneNote's default blue.
fn section_color(color: Option<u32>) -> [f32; 4] {
    color.map_or(draw::srgb(0x8a, 0xa8, 0xe4), canvas::gpu::colorref)
}

/// The pages whose titles contain `filter`, ignoring case.
fn matching<'a>(
    pages: &'a [(ExGuid, String, u32)],
    filter: &str,
) -> impl Iterator<Item = &'a (ExGuid, String, u32)> {
    let filter = filter.to_lowercase();
    pages
        .iter()
        .filter(move |(_, title, _)| title.to_lowercase().contains(&filter))
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

/// Section tabs for a notebook's readable top-level sections, in its order.
fn tabs(notebook: &notebook::session::Notebook) -> Vec<Tab> {
    notebook
        .catalog()
        .sections
        .iter()
        .filter_map(|section| match &section.state {
            notebook::discover::SectionState::Readable { name, color } => Some(Tab {
                name: name.clone().unwrap_or_else(|| {
                    Path::new(&section.path)
                        .file_stem()
                        .map(|stem| stem.to_string_lossy().into_owned())
                        .unwrap_or_default()
                }),
                path: section.path.clone(),
                color: *color,
            }),
            _ => None,
        })
        .collect()
}

/// The session for `section` showing `space`, or its first page, and that page.
fn read_session(
    section: notebook::session::Section,
    tab: usize,
    space: Option<ExGuid>,
) -> Result<(Session, Page), Box<dyn Error>> {
    let pages = section.pages()?;
    let space = match space {
        Some(space) => space,
        None => pages.first().ok_or("The section has no pages")?.0,
    };
    let page = section.page(space)?;
    let mut session = Session {
        section,
        tab,
        pages,
        space,
        status: "",
        conflict: false,
    };
    session.refresh_conflict()?;
    Ok((session, page))
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
        }) || macos::discard_changes()
        {
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
                            state.ui.theme = theme(appearance);
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
            macos::clear_marked_text(&state.window);
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
            std::mem::take(&mut self.interface_font),
        )) {
            Ok(state) => self.state = Some(state),
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
                        state.config.width = size.width;
                        state.config.height = size.height;
                        state
                            .surface
                            .configure(&state.renderer.device, &state.config);
                    }
                    // Present inside AppKit's resize transaction; a redraw on the next turn
                    // lets the window show the previous frame at the new size.
                    return state.frame();
                }
                WindowEvent::ThemeChanged(appearance) => {
                    state.ui.theme = theme(appearance);
                    state.window.request_redraw();
                }
                WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                    state.renderer.clear_glyph_cache();
                    state.app_icon = macos::app_icon((16.0 * scale_factor).round() as u32);
                    let response = state.view.scale_factor_changed(scale_factor as f32)?;
                    state.respond(response);
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
/// in logical pixels: `move X Y`, `press`, `release`, `wheel DX DY`, `key NAME`, `type
/// TEXT`, `modifiers [shift] [command]`, `wait MILLISECONDS`, `snapshot PNG_PATH` and
/// `appearance light|dark`.
fn replay(script: String, proxy: EventLoopProxy<UserEvent>) -> Result<(), Box<dyn Error>> {
    let mut steps = Vec::new();
    for line in script.lines().filter(|line| !line.trim().is_empty()) {
        let (command, rest) = line.split_once(' ').unwrap_or((line, ""));
        let numbers = || -> Result<Vec<f32>, std::num::ParseFloatError> {
            rest.split_whitespace().map(str::parse).collect()
        };
        let button = |pressed| ui::Event::Button {
            button: MouseButton::Left,
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
                        "command" => Ok(ModifiersState::SUPER),
                        _ => Err(format!("Unknown modifier {name}")),
                    })
                    .try_fold(ModifiersState::empty(), |all, one| one.map(|one| all | one))?,
            ))),
            "wait" => Err(std::time::Duration::from_millis(rest.parse()?)),
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
    let mut interface_font = Vec::new();
    let mut reference = None;
    let mut editable = false;
    let mut section = None;
    let mut notebook = None;
    let mut cache = None;
    while let Some(arg) = args.next() {
        if arg == "--substitute-font" {
            substitutes.push(PathBuf::from(
                args.next()
                    .ok_or("Provide a font file after --substitute-font.")?,
            ));
        } else if arg == "--ui-font" {
            let path = PathBuf::from(
                args.next()
                    .ok_or("Provide a font file or folder after --ui-font.")?,
            );
            let mut files = if path.is_dir() {
                std::fs::read_dir(&path)?
                    .map(|entry| entry.map(|entry| entry.path()))
                    .collect::<Result<Vec<_>, _>>()?
            } else {
                vec![path]
            };
            files.retain(|file| {
                file.extension().is_some_and(|extension| {
                    matches!(extension.to_str(), Some("otf" | "ttf" | "ttc"))
                })
            });
            files.sort();
            for file in files {
                interface_font.push(std::fs::read(file)?);
            }
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
            "Usage: snowbound [TEXT_FILE] [WIDTH_POINTS] [--reference SECTION PAGE_TITLE | --page SECTION PAGE_TITLE | --section SECTION PAGE_TITLE | --notebook FOLDER] [--cache DIR] [--ui-font FONT_FILE_OR_FOLDER] [--substitute-font FONT_FILE]..."
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
        None => PathBuf::from(std::env::var_os("HOME").ok_or("HOME is not set.")?)
            .join("Library/Caches/snowbound"),
    };
    let input = if let Some(root) = notebook {
        Input::Notebook { root, cache }
    } else if let Some((file, title)) = section {
        Input::Section { file, title, cache }
    } else if editable {
        Input::Page(reference.unwrap())
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
    let event_loop = macos::event_loop()?;
    if let Some(script) = std::env::var_os("SNOWBOUND_REPLAY") {
        replay(std::fs::read_to_string(script)?, event_loop.create_proxy())?;
    }
    let mut app = App {
        proxy: event_loop.create_proxy(),
        input: Some(input),
        substitutes,
        interface_font,
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
