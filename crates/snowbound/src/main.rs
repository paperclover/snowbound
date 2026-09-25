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
use onestore::document::Format;
use onestore::page::Page;
use onestore::page::text::Paragraph;
use std::{error::Error, path::PathBuf, sync::Arc, time::Instant};
use winit::{
    application::ApplicationHandler,
    dpi::{LogicalSize, PhysicalPosition, PhysicalSize},
    event::{ElementState, Ime, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoopProxy},
    keyboard::{Key, ModifiersState, NamedKey},
    window::{CursorIcon, Window, WindowId},
};

#[derive(Debug)]
enum UserEvent {
    Quit,
    InsertText(String),
    Accessibility(accesskit_winit::Event),
    /// The section's synchronization thread reported an event.
    Sync,
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
}

/// The opened section and the stored model the editor's page was loaded from.
struct Session {
    section: notebook::session::Section,
    space: onestore::ExGuid,
    before: Page,
    title: String,
    status: &'static str,
}

impl Session {
    fn window_title(&self) -> String {
        let file = self
            .section
            .file()
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        format!("{} · {file}{}", self.title, self.status)
    }
}

struct State {
    window: Arc<Window>,
    instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    renderer: Renderer,
    view: PageView,
    session: Option<Session>,
    initial: Vec<(onestore::ExGuid, TextDocument)>,
    initial_layouts: Vec<(onestore::ExGuid, onestore::document::Layout)>,
    initial_date: Option<u64>,
    occluded: bool,
    ime_allowed: bool,
    clipboard: arboard::Clipboard,
    access_adapter: accesskit_winit::Adapter,
    accessibility: accessibility::Accessibility,
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
                    })
                    .with_inner_size(LogicalSize::new(1000.0, 720.0)),
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
                let section = notebook::session::Section::open(&file, cache, move || {
                    let _ = proxy.send_event(UserEvent::Sync);
                })?;
                let (space, _) = section
                    .pages()?
                    .into_iter()
                    .find(|(_, candidate)| *candidate == title)
                    .ok_or_else(|| format!("No page titled {title:?} in {}", file.display()))?;
                let before = section.page(space)?;
                let (scene, editor) = PageScene::from_page(before.clone(), &mut engine)?;
                session = Some(Session {
                    section,
                    space,
                    before,
                    title,
                    status: "",
                });
                (editor, Some((scene, [0.0; 2])))
            }
        };
        if let Some(session) = &session {
            window.set_title(&session.window_title());
        }
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
        window.set_cursor(CursorIcon::Text);
        window.request_redraw();
        eprintln!("Canvas GPU: {:?}; scale factor {dpr}", adapter.get_info());
        Ok(Self {
            window,
            instance,
            surface,
            config,
            renderer,
            view: PageView::new(
                editor,
                engine,
                scene,
                [size.width, size.height],
                dpr,
                macos::double_click_interval(),
            ),
            session,
            initial,
            initial_date,
            initial_layouts,
            occluded: false,
            ime_allowed: true,
            clipboard: arboard::Clipboard::new()?,
            access_adapter,
            accessibility: accessibility::Accessibility::default(),
        })
    }

    /// Carries out what a page event asked of the platform and follows its changes: the
    /// input method's position, accessibility, saving, the window title and a repaint.
    fn respond(&mut self, response: Response) -> Result<(), Box<dyn Error>> {
        match response.request {
            Some(Request::EditDate(field)) => self.edit_date(field)?,
            Some(Request::Copy(text)) => self.clipboard.set_text(text)?,
            Some(Request::Paste) => {
                let text = self.clipboard.get_text()?;
                let response = self.view.commit_text(text)?;
                self.respond(response)?;
            }
            Some(Request::CharacterPalette) => macos::show_character_palette(),
            None => {}
        }
        if self.ime_allowed != self.view.accepts_text() {
            self.ime_allowed = self.view.accepts_text();
            self.window.set_ime_allowed(self.ime_allowed);
        }
        self.window.set_cursor(cursor_icon(self.view.cursor()));
        if response.changed {
            let [x0, y0, x1, y1] = self.view.caret_area()?;
            self.window.set_ime_cursor_area(
                PhysicalPosition::new(x0, y0),
                PhysicalSize::new(x1 - x0, y1 - y0),
            );
            self.update_accessibility()?;
            self.persist()?;
            self.title_from_page();
        }
        if response.redraw {
            self.window.request_redraw();
        }
        Ok(())
    }

    /// A temporary page's window takes its title's first line.
    fn title_from_page(&self) {
        let editor = &self.view.editor;
        if self.session.is_some() || !editor.active_outline().title {
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
            .unwrap();
        let title = if title.trim().is_empty() {
            "Untitled"
        } else {
            title
        };
        let title = format!("{title} · Temporary page");
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
            self.respond(response)?;
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
                session.status = " · saving";
                self.window.set_title(&session.window_title());
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
        self.window.set_title(&session.window_title());
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
        session.status = " · saving";
        if keep_mine {
            session.before = session.section.page(session.space)?;
            self.window.set_title(&session.window_title());
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
                } => " · saved",
                Event::Attempt {
                    status: notebook::EditStatus::Conflict(_),
                    ..
                } => " · conflict",
                Event::Attempt { .. } => " · saving",
                Event::Unreachable(_) => " · offline",
                Event::Failed(error) => {
                    eprintln!("Synchronization stopped: {error}");
                    " · not saving"
                }
            };
        }
        self.window.set_title(&session.window_title());
        if refreshed && session.section.page(session.space)? != session.before {
            self.reload()?;
        }
        Ok(())
    }

    fn update_accessibility(&mut self) -> Result<(), Box<dyn Error>> {
        let mut error = None;
        let view = &self.view;
        self.access_adapter.update_if_active(|| {
            match self.accessibility.update(
                &view.editor,
                view.viewport,
                &self.window.title(),
                view.outline_preview(),
            ) {
                Ok(mut update) => {
                    self.accessibility.append_page_fields(
                        &mut update,
                        view.scene.as_ref(),
                        &view.editor,
                        view.viewport,
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
                let response = self.view.focus_read_only(index)?;
                self.respond(response)?;
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
        let response = self.view.focus_text()?;
        self.respond(response)
    }

    fn draw(&mut self) -> Result<(), Box<dyn Error>> {
        let viewport = self.view.viewport;
        trace_input(&("Draw", viewport.origin, viewport.scale));
        if self.occluded || viewport.size.contains(&0) {
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
        let [caret, selection] = macos::text_colors();
        let primitives = self.view.primitives(TextColors { caret, selection })?;
        self.renderer
            .draw(
                &frame.texture.create_view(&Default::default()),
                viewport.size,
                [1.0; 4],
                &[viewport.layer(&primitives)],
            )
            .map_err(|error| format!("Canvas drawing failed: {error:?}"))?;
        self.window.pre_present_notify();
        self.renderer.queue.present(frame);
        trace_input(&"Present submitted");
        if reconfigure {
            self.surface.configure(&self.renderer.device, &self.config);
        }
        Ok(())
    }

    fn key(&mut self, key: &Key, text: Option<&str>) -> Result<(), Box<dyn Error>> {
        let modifiers = self.view.modifiers();
        if modifiers.command
            && modifiers.shift
            && self.session.is_some()
            && self.view.accepts_text()
            && let Key::Character(character) = key
        {
            if character.eq_ignore_ascii_case("k") {
                return self.resolve_conflict(true);
            }
            if character.eq_ignore_ascii_case("t") {
                return self.resolve_conflict(false);
            }
        }
        let response = self.view.key(&page_key(key), text)?;
        self.respond(response)
    }
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
                    let result = state
                        .view
                        .insert_text(text)
                        .and_then(|response| state.respond(response));
                    if let Err(error) = result {
                        eprintln!("{error}");
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
        let was_marked =
            !matches!(event, WindowEvent::Ime(_)) && state.view.editor.marked_range().is_some();
        let result = (|| -> Result<(), Box<dyn Error>> {
            let response = match event {
                WindowEvent::RedrawRequested => return state.draw(),
                WindowEvent::Resized(size) => {
                    if size.width > 0 && size.height > 0 {
                        state.config.width = size.width;
                        state.config.height = size.height;
                        state
                            .surface
                            .configure(&state.renderer.device, &state.config);
                    }
                    let response = state.view.resized([size.width, size.height])?;
                    let changed = response.changed;
                    state.respond(response)?;
                    // Present inside AppKit's resize transaction; a redraw on the next turn
                    // lets the window show the previous frame at the new size.
                    if changed {
                        state.draw()?;
                    }
                    return Ok(());
                }
                WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                    state.renderer.clear_glyph_cache();
                    state.view.scale_factor_changed(scale_factor as f32)?
                }
                WindowEvent::Focused(focused) => state.view.focus_changed(focused)?,
                WindowEvent::Occluded(occluded) => {
                    trace_input(&("Window occluded", occluded));
                    state.occluded = occluded;
                    if occluded {
                        return Ok(());
                    }
                    Response {
                        changed: true,
                        redraw: true,
                        request: None,
                    }
                }
                WindowEvent::ModifiersChanged(modifiers) => state
                    .view
                    .modifiers_changed(page_modifiers(modifiers.state()))?,
                WindowEvent::CursorLeft { .. } => state.view.pointer_left(),
                WindowEvent::CursorMoved { position, .. } => state
                    .view
                    .pointer_moved([position.x as f32, position.y as f32])?,
                WindowEvent::MouseInput {
                    state: ElementState::Pressed,
                    button: MouseButton::Left,
                    ..
                } => state.view.pointer_pressed(Instant::now())?,
                WindowEvent::MouseInput {
                    state: ElementState::Released,
                    button: MouseButton::Left,
                    ..
                } => state.view.pointer_released()?,
                WindowEvent::MouseWheel { delta, .. } => {
                    let dpr = state.window.scale_factor() as f32;
                    state.view.wheel(match delta {
                        MouseScrollDelta::LineDelta(x, y) => [x * 32.0 * dpr, y * 32.0 * dpr],
                        MouseScrollDelta::PixelDelta(p) => [p.x as f32, p.y as f32],
                    })?
                }
                WindowEvent::KeyboardInput { event, .. }
                    if event.state == ElementState::Pressed =>
                {
                    return state.key(&event.logical_key, event.text.as_deref());
                }
                WindowEvent::Ime(Ime::Preedit(text, cursor)) => state.view.compose(text, cursor)?,
                WindowEvent::Ime(Ime::Commit(text)) => state.view.commit_text(text)?,
                WindowEvent::Ime(Ime::Disabled) => state.view.cancel_composition()?,
                _ => return Ok(()),
            };
            state.respond(response)
        })();
        if was_marked && state.view.editor.marked_range().is_none() {
            macos::clear_marked_text(&state.window);
        }
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

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args_os().skip(1);
    let mut positional = Vec::new();
    let mut substitutes = Vec::new();
    let mut reference = None;
    let mut editable = false;
    let mut section = None;
    let mut cache = None;
    while let Some(arg) = args.next() {
        if arg == "--substitute-font" {
            substitutes.push(PathBuf::from(
                args.next()
                    .ok_or("Provide a font file after --substitute-font.")?,
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
    if (editable || section.is_some()) && !positional.is_empty() {
        return Err(
            "Use --page or --section with a section file and page title, without a text file or width.".into(),
        );
    }
    if positional.len() > 2 {
        return Err(
            "Usage: snowbound [TEXT_FILE] [WIDTH_POINTS] [--reference SECTION PAGE_TITLE | --page SECTION PAGE_TITLE | --section SECTION PAGE_TITLE [--cache DIR]] [--substitute-font FONT_FILE]..."
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
    let input = if let Some((file, title)) = section {
        let cache = match cache {
            Some(cache) => cache,
            None => PathBuf::from(std::env::var_os("HOME").ok_or("HOME is not set.")?)
                .join("Library/Caches/snowbound"),
        };
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
