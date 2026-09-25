mod macos;

use canvas::gpu::page::PageScene;
use canvas::interaction::{
    Cursor, Key as PageKey, Modifiers, NamedKey as PageNamedKey, PageView, Request, Response,
    TextColors, accessibility,
};
use canvas::{
    date::DateField,
    document::TextDocument,
    editor::{CanvasEditor, DEFAULT_OUTLINE_WIDTH, TextOutline},
    layout::TextEngine,
};
use draw::Renderer;
use onestore::ExGuid;
use onestore::document::Format;
use onestore::page::Page;
use onestore::page::text::Paragraph;
use std::{
    error::Error,
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};
use ui::{Axis, Flags, Id, Spec, Theme, Ui, fill, fit, px};
use winit::{
    application::ApplicationHandler,
    dpi::{LogicalSize, PhysicalPosition, PhysicalSize},
    event::{ElementState, Ime, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoopProxy},
    keyboard::{Key, ModifiersState, NamedKey},
    platform::macos::WindowAttributesExtMacOS,
    window::{CursorIcon, Window, WindowId},
};

/// Height of the window's top strip, which holds the traffic lights and section tabs.
const STRIP: f32 = 38.0;
/// Room the traffic lights take at the strip's leading edge.
const LIGHTS: f32 = 78.0;
const PAGE_LIST: f32 = 240.0;

#[derive(Debug)]
enum UserEvent {
    Quit,
    InsertText(String),
    Accessibility(accesskit_winit::Event),
    /// The section's synchronization thread reported an event.
    Sync,
    /// Scripted input from `SNOWBOUND_REPLAY`.
    Replay(Replay),
}

#[derive(Debug)]
enum Replay {
    Input(ui::Event),
    /// Paints the next frame into a PNG as well as the window.
    Snapshot(PathBuf),
}

impl From<accesskit_winit::Event> for UserEvent {
    fn from(event: accesskit_winit::Event) -> Self {
        Self::Accessibility(event)
    }
}

fn trace_input(event: &impl std::fmt::Debug) {
    if std::env::var_os("SNOWBOUND_TRACE_INPUT").is_some() {
        eprintln!("Input {:?}: {event:?}", std::time::SystemTime::now());
    }
}

fn page_key(key: &Key) -> PageKey {
    match key {
        Key::Character(text) => PageKey::Character(text.to_string()),
        Key::Named(named) => PageKey::Named(match named {
            NamedKey::Escape => PageNamedKey::Escape,
            NamedKey::Tab => PageNamedKey::Tab,
            NamedKey::Space => PageNamedKey::Space,
            NamedKey::Enter => PageNamedKey::Enter,
            NamedKey::Backspace => PageNamedKey::Backspace,
            NamedKey::Delete => PageNamedKey::Delete,
            NamedKey::ArrowLeft => PageNamedKey::ArrowLeft,
            NamedKey::ArrowRight => PageNamedKey::ArrowRight,
            NamedKey::ArrowUp => PageNamedKey::ArrowUp,
            NamedKey::ArrowDown => PageNamedKey::ArrowDown,
            NamedKey::Home => PageNamedKey::Home,
            NamedKey::End => PageNamedKey::End,
            NamedKey::Alt
            | NamedKey::AltGraph
            | NamedKey::Control
            | NamedKey::Shift
            | NamedKey::Super
            | NamedKey::Meta => PageNamedKey::Modifier,
            _ => PageNamedKey::Other,
        }),
        _ => PageKey::Named(PageNamedKey::Other),
    }
}

fn page_modifiers(modifiers: ModifiersState) -> Modifiers {
    Modifiers {
        shift: modifiers.shift_key(),
        control: modifiers.control_key(),
        option: modifiers.alt_key(),
        command: modifiers.super_key(),
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

/// The open section and the stored model the editor's page was loaded from.
struct Session {
    section: notebook::session::Section,
    tab: usize,
    /// Spaces, titles and outline levels in section order.
    pages: Vec<(ExGuid, String, u32)>,
    space: ExGuid,
    before: Page,
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
            .conflicts()?
            .iter()
            .any(|(edit, _)| edit.space == self.space);
        Ok(())
    }
}

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
    instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    renderer: Renderer,
    ui: Ui,
    view: PageView,
    library: Option<Library>,
    session: Option<Session>,
    /// Text filtering the page list.
    filter: String,
    commands: Vec<Command>,
    /// The page changed during this frame, to be saved and announced after it.
    changed: bool,
    /// Whether the page last heard it had keyboard focus.
    page_focused: bool,
    window_focused: bool,
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
                library = Some(opened);
                let (scene, editor) =
                    open_session(section, 0, Some(space), &mut engine, &mut session)?;
                (editor, Some((scene, [0.0; 2])))
            }
            Input::Notebook { root, cache } => {
                let notebook = notebook::session::Notebook::open(&root, &cache)?;
                let tabs = tabs(&notebook);
                if tabs.is_empty() {
                    return Err(format!("No readable sections in {}", root.display()).into());
                }
                let opened = Library {
                    notebook: Some(notebook),
                    tabs,
                    cache,
                };
                let section = opened.open(0, proxy.clone())?;
                library = Some(opened);
                let (scene, editor) = open_session(section, 0, None, &mut engine, &mut session)?;
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
        let mut ui = Ui::new(Theme::dark());
        ui.set_focus(Some(page()));
        let state = Self {
            window,
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
            commands: Vec::new(),
            changed: false,
            page_focused: true,
            window_focused: true,
            pointer: [0.0; 2],
            strip_press: None,
            snapshot: None,
            initial,
            initial_date,
            initial_layouts,
            occluded: false,
            ime_allowed: true,
            clipboard: arboard::Clipboard::new()?,
            access_adapter,
            accessibility: accessibility::Accessibility::default(),
        };
        state.title();
        Ok(state)
    }

    /// Builds, lays out and paints one frame, then does what it asked for and asks for the
    /// frame that shows the result.
    fn frame(&mut self) -> Result<(), Box<dyn Error>> {
        let size = self.window.inner_size();
        let scale = self.window.scale_factor() as f32;
        self.ui.begin(
            [size.width as f32 / scale, size.height as f32 / scale],
            scale,
            Instant::now(),
        );
        self.build()?;
        self.ui.end();
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
        let commands = std::mem::take(&mut self.commands);
        let follow = !commands.is_empty() || self.ui.wants_frame();
        for command in commands {
            self.apply(command)?;
        }
        if std::mem::take(&mut self.changed) {
            self.after_edit()?;
        }
        if follow {
            self.window.request_redraw();
        }
        Ok(())
    }

    fn build(&mut self) -> Result<(), Box<dyn Error>> {
        let theme = self.ui.theme.clone();
        self.ui.open_as(
            strip(),
            Spec {
                flags: Flags::CLICKABLE,
                size: [fill(), px(STRIP)],
                fill: Some(theme.strip),
                pad: [0.0, 8.0],
                gap: 2.0,
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
        match (&self.library, &self.session) {
            (Some(library), Some(session)) => {
                for (index, tab) in library.tabs.iter().enumerate() {
                    let active = index == session.tab;
                    let id = self.ui.open(
                        ("tab", index),
                        Spec {
                            flags: Flags::CLICKABLE,
                            size: [fit(), px(STRIP - 8.0)],
                            text: Some(&tab.name),
                            color: Some(if active { theme.text } else { theme.text_dim }),
                            fill: Some(if active { theme.base } else { theme.strip }),
                            hover_fill: (!active).then_some(theme.chip),
                            radius: 6.0,
                            pad: [14.0 + 14.0, 0.0],
                            center: true,
                            ..Spec::default()
                        },
                    );
                    let swatch = tab.color.map_or(theme.text_dim, canvas::gpu::colorref);
                    let middle = (STRIP - 8.0) / 2.0;
                    self.ui
                        .mark([12.0, middle - 4.0, 20.0, middle + 4.0], swatch);
                    self.ui.close();
                    if self.ui.signal(id).clicked && !active {
                        self.commands.push(Command::OpenSection(index));
                    }
                }
            }
            _ => {
                self.ui.leaf(
                    "tab",
                    Spec {
                        size: [fit(), px(STRIP - 8.0)],
                        text: Some("Temporary page"),
                        fill: Some(theme.base),
                        radius: 6.0,
                        pad: [16.0, 0.0],
                        ..Spec::default()
                    },
                );
            }
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
                    size: [fit(), px(STRIP - 8.0)],
                    text: Some(session.status),
                    color: Some(theme.text_dim),
                    pad: [14.0, 0.0],
                    ..Spec::default()
                },
            );
        }
        self.ui.close();

        self.ui.open(
            "body",
            Spec {
                size: [fill(), fill()],
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
                fill: Some([1.0; 4]),
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
        let focused = self.window_focused && self.ui.focused() == Some(page());
        if focused != self.page_focused {
            self.page_focused = focused;
            let response = self.view.focus_changed(focused)?;
            self.respond(response);
        }
        self.page_events(signal.events)?;
        self.ui.close();
        if self.session.is_some() {
            self.ui.leaf(
                "separator",
                Spec {
                    size: [px(1.0), fill()],
                    fill: Some(theme.separator),
                    ..Spec::default()
                },
            );
            self.page_list(&theme);
        }
        self.ui.close();
        Ok(())
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

    fn page_list(&mut self, theme: &Theme) {
        let Some(session) = &self.session else {
            return;
        };
        self.ui.open(
            "panel",
            Spec {
                axis: Axis::Y,
                size: [px(PAGE_LIST), fill()],
                fill: Some(theme.panel),
                ..Spec::default()
            },
        );
        self.ui.open(
            "pages",
            Spec {
                flags: Flags::SCROLL | Flags::CLIP,
                axis: Axis::Y,
                size: [fill(), fill()],
                pad: [6.0, 6.0],
                gap: 1.0,
                ..Spec::default()
            },
        );
        let query = self.filter.to_lowercase();
        let mut first = None;
        for (space, title, level) in &session.pages {
            if !title.to_lowercase().contains(&query) {
                continue;
            }
            first.get_or_insert(*space);
            let selected = *space == session.space;
            let signal = self.ui.leaf(
                space,
                Spec {
                    flags: Flags::CLICKABLE,
                    size: [fill(), px(26.0)],
                    text: Some(if title.is_empty() {
                        "Untitled page"
                    } else {
                        title
                    }),
                    color: Some(if selected || !title.is_empty() {
                        theme.text
                    } else {
                        theme.text_dim
                    }),
                    fill: selected.then_some(theme.accent),
                    hover_fill: (!selected).then_some(theme.hover),
                    hover_border: (!selected).then_some(theme.accent),
                    radius: 4.0,
                    pad: [10.0 + 16.0 * level.saturating_sub(1) as f32, 0.0],
                    ..Spec::default()
                },
            );
            if signal.clicked && !selected {
                self.commands.push(Command::OpenPage(*space));
            }
        }
        self.ui.close();
        let placeholder = format!("Filter {} pages…", session.pages.len());
        self.ui.open(
            "footer",
            Spec {
                size: [fill(), px(38.0)],
                pad: [6.0, 6.0],
                ..Spec::default()
            },
        );
        let before = self.filter.clone();
        let signal = ui::text_field(
            &mut self.ui,
            filter(),
            &mut self.filter,
            &placeholder,
            Spec {
                size: [fill(), px(26.0)],
                fill: Some(theme.base),
                border: Some(theme.separator),
                radius: 4.0,
                pad: [8.0, 0.0],
                ..Spec::default()
            },
        );
        // The list above was built with the filter as it stood.
        if self.filter != before {
            self.window.request_redraw();
        }
        for event in &signal.events {
            if let ui::Event::Key {
                key: Key::Named(key @ (NamedKey::Escape | NamedKey::Enter)),
                ..
            } = event
            {
                if *key == NamedKey::Enter
                    && let Some(space) = first.filter(|space| *space != session.space)
                {
                    self.commands.push(Command::OpenPage(space));
                }
                self.filter.clear();
                self.ui.set_focus(Some(page()));
            }
        }
        self.ui.close();
        self.ui.close();
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
                    self.view.key(&page_key(&key), text.as_deref())?
                }
                ui::Event::Ime(Ime::Preedit(text, cursor)) => self.view.compose(text, cursor)?,
                ui::Event::Ime(Ime::Commit(text)) => self.view.commit_text(text)?,
                ui::Event::Ime(Ime::Disabled) => self.view.cancel_composition()?,
                ui::Event::Modifiers(modifiers) => {
                    self.view.modifiers_changed(page_modifiers(modifiers))?
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
        if let Some(request) = response.request {
            self.commands.push(Command::Page(request));
        }
    }

    fn apply(&mut self, command: Command) -> Result<(), Box<dyn Error>> {
        match command {
            Command::OpenSection(tab) => {
                let library = self.library.as_ref().ok_or("No sections to open")?;
                let section = library.open(tab, self.proxy.clone())?;
                let (scene, editor) =
                    open_session(section, tab, None, &mut self.view.engine, &mut self.session)?;
                self.view.open(editor, Some((scene, [0.0; 2])));
                self.filter.clear();
                self.opened()?;
            }
            Command::OpenPage(space) => {
                let session = self.session.as_mut().ok_or("No section is open")?;
                let page = session.section.page(space)?;
                let (scene, editor) = PageScene::from_page(page.clone(), &mut self.view.engine)?;
                session.space = space;
                session.before = page;
                session.refresh_conflict()?;
                self.view.open(editor, Some((scene, [0.0; 2])));
                self.opened()?;
            }
            Command::Resolve { keep_mine } => self.resolve_conflict(keep_mine)?,
            Command::Page(Request::EditDate(field)) => self.edit_date(field)?,
            Command::Page(Request::Copy(text)) => self.clipboard.set_text(text)?,
            Command::Page(Request::Paste) => {
                let text = self.clipboard.get_text()?;
                let response = self.view.commit_text(text)?;
                self.respond(response);
            }
            Command::Page(Request::CharacterPalette) => macos::show_character_palette(),
        }
        Ok(())
    }

    /// Follows a page shown in place of another.
    fn opened(&mut self) -> Result<(), Box<dyn Error>> {
        self.ui.set_focus(Some(page()));
        self.title();
        self.update_accessibility()?;
        self.window.request_redraw();
        Ok(())
    }

    /// Follows an edit: the input method's position, accessibility, saving and the title.
    fn after_edit(&mut self) -> Result<(), Box<dyn Error>> {
        let scale = self.ui.scale();
        let corner = self.ui.rect(page()).unwrap_or_default();
        let [x0, y0, x1, y1] = self.view.caret_area()?;
        self.window.set_ime_cursor_area(
            PhysicalPosition::new(x0 + corner[0] * scale, y0 + corner[1] * scale),
            PhysicalSize::new(x1 - x0, y1 - y0),
        );
        self.update_accessibility()?;
        self.persist()?;
        self.title();
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

    /// Saves the edited page to the section's replica; a page changed underneath the
    /// editor is reloaded in place of the edit.
    fn persist(&mut self) -> Result<(), Box<dyn Error>> {
        let Some(session) = &mut self.session else {
            return Ok(());
        };
        let after = self.view.editor.page()?;
        match session
            .section
            .save(session.space, &session.before, &after, "snowbound")?
        {
            notebook::session::Save::Unchanged => {}
            notebook::session::Save::Queued(_) => {
                session.before = session.section.page(session.space)?;
                session.status = "Saving";
                if let Some(entry) = session
                    .pages
                    .iter_mut()
                    .find(|(space, ..)| *space == session.space)
                {
                    entry.1 = after.title;
                }
            }
            notebook::session::Save::Stale => self.reload()?,
        }
        Ok(())
    }

    /// Replaces the editor with the page currently stored in the section.
    fn reload(&mut self) -> Result<(), Box<dyn Error>> {
        let Some(session) = &mut self.session else {
            return Ok(());
        };
        let page = session.section.page(session.space)?;
        let (scene, editor) = PageScene::from_page(page.clone(), &mut self.view.engine)?;
        session.before = page;
        self.view.replace(editor, Some((scene, [0.0; 2])));
        self.title();
        self.update_accessibility()?;
        self.window.request_redraw();
        Ok(())
    }

    /// Reviews the oldest conflict on this page: `keep_mine` publishes the editor's page
    /// over the remote change, otherwise the remote page replaces the editor's.
    fn resolve_conflict(&mut self, keep_mine: bool) -> Result<(), Box<dyn Error>> {
        let Some(session) = &mut self.session else {
            return Ok(());
        };
        let Some((edit, _)) = session
            .section
            .conflicts()?
            .into_iter()
            .find(|(edit, _)| edit.space == session.space)
        else {
            return Ok(());
        };
        let reviewed = if keep_mine {
            self.view.editor.page()?
        } else {
            session.section.remote_page(session.space)?
        };
        session.section.review(edit.id, &reviewed)?;
        session.status = "Saving";
        session.refresh_conflict()?;
        if keep_mine {
            session.before = session.section.page(session.space)?;
            self.window.request_redraw();
            Ok(())
        } else {
            self.reload()
        }
    }

    /// Applies what the synchronization thread reported since the last poll.
    fn synced(&mut self) -> Result<(), Box<dyn Error>> {
        let Some(session) = &mut self.session else {
            return Ok(());
        };
        let shown = (session.status, session.conflict);
        let mut refreshed = false;
        for event in session.section.events() {
            use notebook::session::Event;
            session.status = match event {
                Event::Refreshed => {
                    refreshed = true;
                    continue;
                }
                Event::Attempt {
                    status: notebook::EditStatus::Published { .. },
                    ..
                } => "Saved",
                Event::Attempt {
                    status: notebook::EditStatus::Conflict(_),
                    ..
                } => "Conflict",
                Event::Attempt { .. } => "Saving",
                Event::Unreachable(_) => "Offline",
                Event::Failed(error) => {
                    eprintln!("Synchronization stopped: {error}");
                    "Not saving"
                }
            };
        }
        session.refresh_conflict()?;
        if refreshed || shown != (session.status, session.conflict) {
            self.window.request_redraw();
        }
        if refreshed {
            session.pages = session.section.pages()?;
            if !session
                .pages
                .iter()
                .any(|(space, ..)| *space == session.space)
            {
                let first = session.pages.first().ok_or("The section has no pages")?.0;
                self.commands.push(Command::OpenPage(first));
            } else if session.section.page(session.space)? != session.before {
                self.reload()?;
            }
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
                viewport,
                &self.window.title(),
                view.outline_preview(),
            ) {
                Ok(mut update) => {
                    self.accessibility.append_page_fields(
                        &mut update,
                        view.scene.as_ref(),
                        &view.editor,
                        viewport,
                        view.object_focus().and_then(|focus| focus.read_only()),
                    );
                    update
                }
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
        self.paint(&frame.texture.create_view(&Default::default()))?;
        self.window.pre_present_notify();
        self.renderer.queue.present(frame);
        trace_input(&"Present submitted");
        if reconfigure {
            self.surface.configure(&self.renderer.device, &self.config);
        }
        Ok(())
    }

    /// Paints the interface with the page in its box.
    fn paint(&mut self, target: &wgpu::TextureView) -> Result<(), Box<dyn Error>> {
        let [caret, selection] = macos::text_colors();
        let page_primitives = self.view.primitives(TextColors { caret, selection })?;
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
                    primitives,
                },
                ui::Layer::Custom { rect, .. } => draw::Layer {
                    scale: viewport.scale,
                    origin: [
                        viewport.origin[0] + corner[0] * scale,
                        viewport.origin[1] + corner[1] * scale,
                    ],
                    clip: Some(rect.map(|value| value * scale)),
                    primitives: &page_primitives,
                },
            })
            .collect();
        trace_input(&("Draw", viewport.origin, viewport.scale));
        self.renderer
            .draw(
                target,
                [self.config.width, self.config.height],
                self.ui.theme.base,
                &layers,
            )
            .map_err(|error| format!("Canvas drawing failed: {error:?}").into())
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
                self.window.set_maximized(!self.window.is_maximized());
            } else {
                let _ = self.window.drag_window();
            }
            return;
        }
        self.ui.event(event);
        self.window.request_redraw();
    }
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

/// Makes `section` the open one showing `space`, or its first page, and returns the
/// page's scene and editor.
fn open_session(
    section: notebook::session::Section,
    tab: usize,
    space: Option<ExGuid>,
    engine: &mut TextEngine,
    session: &mut Option<Session>,
) -> Result<(PageScene, CanvasEditor), Box<dyn Error>> {
    let pages = section.pages()?;
    let space = match space {
        Some(space) => space,
        None => pages.first().ok_or("The section has no pages")?.0,
    };
    let before = section.page(space)?;
    let (scene, editor) = PageScene::from_page(before.clone(), engine)?;
    let mut opened = Session {
        section,
        tab,
        pages,
        space,
        before,
        status: "",
        conflict: false,
    };
    opened.refresh_conflict()?;
    *session = Some(opened);
    Ok((scene, editor))
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
            UserEvent::Replay(replay) => {
                if let Some(state) = &mut self.state {
                    match replay {
                        Replay::Input(event) => state.input(event),
                        Replay::Snapshot(path) => state.snapshot = Some(path),
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
            accesskit_winit::WindowEvent::InitialTreeRequested => state.update_accessibility(),
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
                WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                    state.renderer.clear_glyph_cache();
                    let response = state.view.scale_factor_changed(scale_factor as f32)?;
                    state.respond(response);
                    state.window.request_redraw();
                }
                WindowEvent::Focused(focused) => {
                    state.window_focused = focused;
                    state.window.request_redraw();
                }
                WindowEvent::Occluded(occluded) => {
                    trace_input(&("Window occluded", occluded));
                    state.occluded = occluded;
                    if !occluded {
                        state.changed = true;
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
        let (repaint, next) = if state.occluded {
            (false, None)
        } else {
            state.view.blink(Instant::now())
        };
        if repaint {
            state.window.request_redraw();
        }
        event_loop.set_control_flow(next.map_or(ControlFlow::Wait, ControlFlow::WaitUntil));
    }
}

/// Feeds a development script to the window from another thread, one command per line
/// in logical pixels: `move X Y`, `press`, `release`, `wheel DX DY`, `key NAME`, `type
/// TEXT`, `modifiers [shift] [command]`, `wait MILLISECONDS` and `snapshot PNG_PATH`.
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
                Err(duration) => std::thread::sleep(duration),
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
            "Usage: snowbound [TEXT_FILE] [WIDTH_POINTS] [--reference SECTION PAGE_TITLE | --page SECTION PAGE_TITLE | --section SECTION PAGE_TITLE | --notebook FOLDER] [--cache DIR] [--substitute-font FONT_FILE]..."
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
        state: None,
        startup_error: None,
    };
    event_loop.run_app(&mut app)?;
    app.startup_error.map_or(Ok(()), Err)
}
