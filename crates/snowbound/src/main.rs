mod accessibility;
mod macos;
#[cfg(test)]
mod profile;
mod scroll;

use canvas::gpu::{Primitive, Renderer, Stroke, Viewport, page::PageScene};
use canvas::{
    date::DateField,
    document::TextDocument,
    editor::{
        CanvasEditor, DEFAULT_OUTLINE_WIDTH, Movement, Selection, SelectionUnit, TextOutline,
    },
    layout::TextEngine,
};
use onestore::document::Format;
use onestore::page::Page;
use onestore::page::text::Paragraph;
use std::{
    error::Error,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use winit::{
    application::ApplicationHandler,
    dpi::{LogicalSize, PhysicalPosition, PhysicalSize},
    event::{ElementState, Ime, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoopProxy},
    keyboard::{Key, ModifiersState, NamedKey},
    window::{Window, WindowId},
};

const HANDLE_HEIGHT: f32 = 6.75;
const DATE_LABELS: [&str; 2] = ["Page date", "Page time"];

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

fn read_only_shortcut(key: &Key, modifiers: ModifiersState) -> bool {
    key == &Key::Named(NamedKey::Escape)
        || (modifiers.control_key() && key == &Key::Named(NamedKey::Tab))
        || (modifiers.super_key()
            && matches!(key, Key::Character(value) if matches!(value.as_str(), "+" | "=" | "-" | "0") || (modifiers.shift_key() && value.eq_ignore_ascii_case("n"))))
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

enum Drag {
    Text {
        anchor: Selection,
        unit: SelectionUnit,
    },
    Scrollbar {
        axis: usize,
        grab: f32,
    },
    Resize {
        outline: Option<Box<TextOutline>>,
        grab: f32,
    },
    Outline {
        id: onestore::ExGuid,
        grab: [f32; 2],
        pending_press: Option<[f32; 2]>,
    },
    Image {
        id: onestore::ExGuid,
        handle: [i8; 2],
        /// Document point of the press.
        press: [f32; 2],
        pending_press: Option<[f32; 2]>,
    },
}

#[derive(Clone, Copy)]
enum PointerFeedback<'a> {
    Hover(onestore::ExGuid),
    Move(onestore::ExGuid, [f32; 2]),
    Resize(&'a TextOutline),
    /// A picture being moved or resized, drawn at this origin and size.
    Image(onestore::ExGuid, [f32; 2], [f32; 2]),
}

/// A non-text object holding focus, which hides the text caret and suspends typing.
#[derive(Clone, Copy, Debug, PartialEq)]
enum ObjectFocus {
    ReadOnly(usize),
    Image(onestore::ExGuid),
}

impl ObjectFocus {
    fn read_only(self) -> Option<usize> {
        match self {
            Self::ReadOnly(index) => Some(index),
            Self::Image(_) => None,
        }
    }
}

struct State {
    window: Arc<Window>,
    instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    renderer: Renderer,
    engine: TextEngine,
    editor: CanvasEditor,
    session: Option<Session>,
    initial: Vec<(onestore::ExGuid, TextDocument)>,
    initial_layouts: Vec<(onestore::ExGuid, onestore::document::Layout)>,
    initial_date: Option<u64>,
    scene: Option<(PageScene, [f32; 2])>,
    viewport: Viewport,
    display_scale: f32,
    pointer: [f32; 2],
    pointer_inside: bool,
    last_click: Option<(Instant, [f32; 2], u8)>,
    drag: Option<Drag>,
    object_focus: Option<ObjectFocus>,
    modifiers: ModifiersState,
    focused: bool,
    occluded: bool,
    caret: bool,
    blink_at: Instant,
    clipboard: arboard::Clipboard,
    access_adapter: accesskit_winit::Adapter,
    accessibility: accessibility::Accessibility,
}

impl State {
    fn edit_date(&mut self, field: DateField) -> Result<(), Box<dyn Error>> {
        let Some(date) = self.editor.date() else {
            return Ok(());
        };
        let timestamp = date.timestamp();
        self.editor.finish_composition();
        macos::clear_marked_text(&self.window);
        self.drag = None;
        if let Some((timestamp, text)) = macos::edit_date(timestamp, field)? {
            self.editor.change_date(&mut self.engine, timestamp, text)?;
        }
        self.changed()
    }

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
        window.set_cursor(winit::window::CursorIcon::Text);
        window.request_redraw();
        eprintln!("Canvas GPU: {:?}; scale factor {dpr}", adapter.get_info());
        Ok(Self {
            window,
            instance,
            surface,
            config,
            renderer,
            engine,
            editor,
            session,
            initial,
            initial_date,
            initial_layouts,
            scene,
            viewport: Viewport {
                size: [size.width, size.height],
                scale: dpr * 96.0 / 72.0,
                origin: [48.0 * dpr; 2],
            },
            display_scale: dpr,
            pointer: [0.0; 2],
            pointer_inside: false,
            last_click: None,
            drag: None,
            object_focus: None,
            modifiers: ModifiersState::empty(),
            focused: true,
            occluded: false,
            caret: true,
            blink_at: Instant::now() + Duration::from_millis(500),
            clipboard: arboard::Clipboard::new()?,
            access_adapter,
            accessibility: accessibility::Accessibility::default(),
        })
    }

    fn preview(&self) -> Option<(onestore::ExGuid, [f32; 2])> {
        let Drag::Outline {
            id,
            grab,
            pending_press,
        } = self.drag.as_ref()?
        else {
            return None;
        };
        let origin = self
            .editor
            .outlines()
            .iter()
            .find(|outline| outline.id == *id)?
            .origin();
        if pending_press.is_some() {
            return Some((*id, origin));
        }
        let point = self.viewport.document_point(self.pointer);
        let position = [point[0] - grab[0], point[1] - grab[1]];
        Some((
            *id,
            if self.modifiers.alt_key() {
                position
            } else {
                snap_to_grid(position, self.editor.margin_origin())
            },
        ))
    }

    fn image_preview(&self) -> Option<(onestore::ExGuid, [f32; 2], [f32; 2])> {
        let Drag::Image {
            id,
            handle,
            press,
            pending_press: None,
        } = self.drag.as_ref()?
        else {
            return None;
        };
        let (origin, size) = self.editor.image_placement(*id)?;
        let point = self.viewport.document_point(self.pointer);
        let delta = [point[0] - press[0], point[1] - press[1]];
        if *handle != [0, 0] {
            let (origin, size) = resize_image(origin, size, *handle, delta);
            return Some((*id, origin, size));
        }
        let origin = [origin[0] + delta[0], origin[1] + delta[1]];
        let origin = if self.modifiers.alt_key() {
            origin
        } else {
            snap_to_grid(origin, self.editor.margin_origin())
        };
        Some((*id, origin, size))
    }

    /// The selected picture's handles lie above the page; its body keeps its paint order.
    fn hit_test(&self, point: [f32; 2]) -> Option<Hit> {
        let pixel = self.display_scale / self.viewport.scale;
        if let Some(ObjectFocus::Image(id)) = self.object_focus
            && let Some((origin, size)) = self.editor.image_placement(id)
            && let Some(handle) = image_handle_at(image_rect(origin, size), pixel, point)
        {
            return Some(Hit::Image { id, handle });
        }
        page_hit_test(&self.editor, self.scene.as_ref(), point, pixel)
    }

    fn set_object_focus(&mut self, focus: Option<ObjectFocus>) {
        if self.object_focus != focus {
            self.editor.finish_composition();
            self.drag = None;
            self.object_focus = focus;
            self.window.set_ime_allowed(focus.is_none());
        }
    }

    /// Page coordinates of a focused object.
    fn object_rect(&self, focus: ObjectFocus) -> [f32; 4] {
        let (scene, offset) = self.scene.as_ref().unwrap();
        let [x0, y0, x1, y1] = match focus {
            ObjectFocus::ReadOnly(index) => scene
                .read_only(Some(&self.editor))
                .nth(index)
                .unwrap()
                .rect(),
            ObjectFocus::Image(id) => {
                let (origin, size) = self.editor.image_placement(id).unwrap();
                image_rect(origin, size)
            }
        };
        [
            x0 + offset[0],
            y0 + offset[1],
            x1 + offset[0],
            y1 + offset[1],
        ]
    }

    fn changed(&mut self) -> Result<(), Box<dyn Error>> {
        self.scroll().clamp(&mut self.viewport);
        self.caret = true;
        self.blink_at = Instant::now() + Duration::from_millis(500);
        let mut rect = self.editor.caret(1.0)?;
        let origin = self
            .preview()
            .map(|(_, origin)| origin)
            .unwrap_or_else(|| self.editor.active_outline().origin());
        rect.x0 += f64::from(origin[0]);
        rect.y0 += f64::from(origin[1]);
        rect.x1 += f64::from(origin[0]);
        rect.y1 += f64::from(origin[1]);
        let v = self.viewport;
        self.window.set_ime_cursor_area(
            PhysicalPosition::new(
                rect.x0 * f64::from(v.scale) + f64::from(v.origin[0]),
                rect.y0 * f64::from(v.scale) + f64::from(v.origin[1]),
            ),
            PhysicalSize::new(
                (rect.width() * f64::from(v.scale)).max(1.0),
                (rect.height() * f64::from(v.scale)).max(1.0),
            ),
        );
        self.update_accessibility()?;
        self.persist()?;
        if self.session.is_none() && self.editor.active_outline().title {
            let title = self
                .editor
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
        self.window.request_redraw();
        Ok(())
    }

    /// Saves the edited page to the section's replica; a page changed underneath the
    /// editor is reloaded in place of the edit.
    fn persist(&mut self) -> Result<(), Box<dyn Error>> {
        let Some(session) = &mut self.session else {
            return Ok(());
        };
        let after = self.editor.page()?;
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
        let (scene, editor) = PageScene::from_page(page.clone(), &mut self.engine)?;
        session.before = page;
        self.editor = editor;
        self.scene = Some((scene, [0.0; 2]));
        self.drag = None;
        self.object_focus = None;
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
            self.editor.page()?
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
        let preview = self.preview();
        self.access_adapter.update_if_active(|| {
            match self.accessibility.update(
                &self.editor,
                self.viewport,
                &self.window.title(),
                preview,
            ) {
                Ok(mut update) => {
                    self.accessibility.append_page_fields(
                        &mut update,
                        self.scene.as_ref(),
                        &self.editor,
                        self.viewport,
                        self.object_focus.and_then(ObjectFocus::read_only),
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
                self.set_object_focus(Some(ObjectFocus::ReadOnly(index)));
                self.window.focus_window();
                self.reveal_focus()?;
                self.changed()?;
            }
            return Ok(());
        }
        let Some(outline) = self.accessibility.outline_for_node(request.target_node) else {
            return Ok(());
        };
        match (request.action, request.data) {
            (Action::Focus, _) => {
                self.editor.focus_outline(outline)?;
                self.window.focus_window();
            }
            (Action::SetTextSelection, Some(ActionData::SetTextSelection(selection))) => {
                let selection = self.accessibility.selection(outline, selection)?;
                self.editor.focus_outline(outline)?;
                self.editor.select(selection)?;
                trace_input(&self.editor.selection());
            }
            (Action::SetValue, Some(ActionData::Value(text))) => {
                self.editor.focus_outline(outline)?;
                self.editor.select_all()?;
                self.editor.commit_text(&mut self.engine, text.into())?;
            }
            (Action::ReplaceSelectedText, Some(ActionData::Value(text))) => {
                self.editor.focus_outline(outline)?;
                self.editor.commit_text(&mut self.engine, text.into())?;
            }
            _ => return Ok(()),
        }
        self.set_object_focus(None);
        self.reveal_focus()?;
        self.changed()
    }

    fn reveal_focus(&mut self) -> Result<(), Box<dyn Error>> {
        if self.viewport.size.contains(&0) {
            return Ok(());
        }
        let rect = if let Some(focus) = self.object_focus {
            let [x0, y0, x1, y1] = self.object_rect(focus).map(f64::from);
            parley::BoundingBox { x0, y0, x1, y1 }
        } else {
            let mut rect = self.editor.caret(1.0)?;
            let origin = self.editor.active_outline().origin();
            rect.x0 += f64::from(origin[0]);
            rect.x1 += f64::from(origin[0]);
            rect.y0 += f64::from(origin[1]);
            rect.y1 += f64::from(origin[1]);
            rect
        };
        let outline = self.editor.active_outline().bounds();
        for (axis, (mut start, mut end)) in [(rect.x0, rect.x1), (rect.y0, rect.y1)]
            .into_iter()
            .enumerate()
        {
            let size = f64::from(self.viewport.size[axis]);
            let margin = f64::from(16.0 * self.display_scale).min(size * 0.25);
            let (outline_start, outline_end) =
                [(outline.x0, outline.x1), (outline.y0, outline.y1)][axis];
            let outline_start = outline_start.min(start);
            let outline_end = outline_end.max(end);
            if self.object_focus.is_none()
                && (outline_end - outline_start) * f64::from(self.viewport.scale)
                    <= size - margin * 2.0
            {
                start = outline_start;
                end = outline_end;
            }
            let start =
                start * f64::from(self.viewport.scale) + f64::from(self.viewport.origin[axis]);
            let end = end * f64::from(self.viewport.scale) + f64::from(self.viewport.origin[axis]);
            let shift = if start < margin {
                margin - start
            } else if end > size - margin {
                size - margin - end
            } else {
                0.0
            };
            self.viewport.origin[axis] += shift as f32;
        }
        Ok(())
    }

    fn zoom(&mut self, factor: f32) {
        let point = self.viewport.document_point(self.pointer);
        let dpr = self.window.scale_factor() as f32;
        self.viewport.scale = (self.viewport.scale * factor).clamp(dpr / 3.0, dpr * 16.0 / 3.0);
        self.viewport.origin = [
            self.pointer[0] - point[0] * self.viewport.scale,
            self.pointer[1] - point[1] * self.viewport.scale,
        ];
    }

    fn scroll(&self) -> scroll::Scroll {
        let editable = self
            .editor
            .visible_outlines()
            .chain(self.editor.caret_outline())
            .map(|outline| {
                let rect = outline.bounds();
                let offset = self
                    .scene
                    .as_ref()
                    .filter(|_| self.editor.has_page_outline(outline.id))
                    .map(|(_, offset)| *offset)
                    .unwrap_or([0.0; 2]);
                [
                    rect.x0 as f32 + offset[0],
                    rect.y0 as f32 + offset[1],
                    rect.x1 as f32 + offset[0],
                    rect.y1 as f32 + offset[1],
                ]
            });
        let fixed = self.scene.iter().flat_map(|(scene, offset)| {
            scene.content_bounds(&self.editor).map(|rect| {
                [
                    rect[0] + offset[0],
                    rect[1] + offset[1],
                    rect[2] + offset[0],
                    rect[3] + offset[1],
                ]
            })
        });
        scroll::Scroll::new(self.viewport, editable.chain(fixed))
    }

    fn draw(&mut self) -> Result<(), Box<dyn Error>> {
        trace_input(&("Draw", self.viewport.origin, self.viewport.scale));
        if self.occluded || self.viewport.size.contains(&0) {
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
        let mut primitives = page_primitives(
            &self.editor,
            self.scene.as_ref(),
            match &self.drag {
                Some(Drag::Resize {
                    outline: Some(outline),
                    ..
                }) => Some(PointerFeedback::Resize(outline)),
                _ => self
                    .preview()
                    .map(|(id, origin)| PointerFeedback::Move(id, origin))
                    .or_else(|| {
                        self.image_preview()
                            .map(|(id, origin, size)| PointerFeedback::Image(id, origin, size))
                    })
                    .or_else(|| {
                        if !self.pointer_inside {
                            return None;
                        }
                        match page_hit_test(
                            &self.editor,
                            self.scene.as_ref(),
                            self.viewport.document_point(self.pointer),
                            self.display_scale / self.viewport.scale,
                        ) {
                            Some(
                                Hit::Text { id, .. }
                                | Hit::Handle { id, .. }
                                | Hit::Resize { id, .. },
                            ) => Some(PointerFeedback::Hover(id)),
                            _ => None,
                        }
                    }),
            },
            self.object_focus,
            self.caret
                && self.focused
                && !matches!(
                    self.drag,
                    Some(Drag::Outline { .. } | Drag::Resize { .. } | Drag::Image { .. })
                ),
            self.viewport.scale,
            self.display_scale,
        )?;
        self.scroll()
            .append(self.viewport, self.display_scale, &mut primitives);
        self.renderer
            .draw(
                &frame.texture.create_view(&Default::default()),
                self.viewport,
                &primitives,
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
        let shift = self.modifiers.shift_key();
        let command = self.modifiers.super_key();
        let option = self.modifiers.alt_key();
        if let Some(ObjectFocus::Image(id)) = self.object_focus
            && matches!(key, Key::Named(NamedKey::Backspace | NamedKey::Delete))
        {
            self.editor.remove_image(id)?;
            self.set_object_focus(None);
            return self.changed();
        }
        if let Some(ObjectFocus::Image(id)) = self.object_focus
            && !(shift || command || option || self.modifiers.control_key())
            && let Key::Named(NamedKey::ArrowLeft | NamedKey::ArrowRight) = key
            && self.editor.step_from_image(
                &mut self.engine,
                id,
                key == &Key::Named(NamedKey::ArrowRight),
            )?
        {
            self.set_object_focus(None);
            self.reveal_focus()?;
            return self.changed();
        }
        if let Some(focus) = self.object_focus {
            let undo = matches!(focus, ObjectFocus::Image(_))
                && command
                && matches!(key, Key::Character(value) if value.eq_ignore_ascii_case("z"));
            if !undo && !read_only_shortcut(key, self.modifiers) {
                return Ok(());
            }
            if key == &Key::Named(NamedKey::Escape) {
                self.set_object_focus(None);
                self.reveal_focus()?;
                return self.changed();
            }
        }
        if command && self.modifiers.control_key() && key == &Key::Named(NamedKey::Space) {
            macos::show_character_palette();
            return Ok(());
        }
        if command && shift && self.session.is_some() {
            match key {
                Key::Character(character) if character.eq_ignore_ascii_case("k") => {
                    return self.resolve_conflict(true);
                }
                Key::Character(character) if character.eq_ignore_ascii_case("t") => {
                    return self.resolve_conflict(false);
                }
                _ => {}
            }
        }
        if matches!(
            self.drag,
            Some(Drag::Outline { .. } | Drag::Resize { .. } | Drag::Image { .. })
        ) {
            if matches!(
                key,
                Key::Named(
                    NamedKey::Alt
                        | NamedKey::AltGraph
                        | NamedKey::Control
                        | NamedKey::Shift
                        | NamedKey::Super
                        | NamedKey::Meta
                )
            ) {
                return Ok(());
            }
            self.drag = None;
            if key == &Key::Named(NamedKey::Escape) {
                return self.changed();
            }
        }
        if command && option {
            let delta = match key {
                Key::Named(NamedKey::ArrowLeft) => Some([-1.0, 0.0]),
                Key::Named(NamedKey::ArrowRight) => Some([1.0, 0.0]),
                Key::Named(NamedKey::ArrowUp) => Some([0.0, -1.0]),
                Key::Named(NamedKey::ArrowDown) => Some([0.0, 1.0]),
                _ => None,
            };
            if let Some(delta) = delta {
                let outline = self.editor.active_outline();
                let origin = outline.origin();
                let step = if shift { 10.0 } else { 1.0 };
                self.editor.move_outline(
                    outline.id,
                    [origin[0] + delta[0] * step, origin[1] + delta[1] * step],
                )?;
                self.reveal_focus()?;
                return self.changed();
            }
        }
        if self.modifiers.control_key() && key == &Key::Named(NamedKey::Tab) {
            let outlines = self.editor.outlines();
            let count = outlines.len()
                + self
                    .scene
                    .as_ref()
                    .map_or(0, |(scene, _)| scene.read_only(Some(&self.editor)).count());
            if count == 0 {
                return self.changed();
            }
            let index = self
                .object_focus
                .and_then(ObjectFocus::read_only)
                .map_or_else(
                    || {
                        outlines
                            .iter()
                            .position(|outline| outline.id == self.editor.active_outline().id)
                    },
                    |index| Some(outlines.len() + index),
                );
            let next = index.map_or(if shift { count - 1 } else { 0 }, |index| {
                if shift {
                    (index + count - 1) % count
                } else {
                    (index + 1) % count
                }
            });
            if next < outlines.len() {
                self.editor.focus_outline(outlines[next].id)?;
                self.set_object_focus(None);
            } else {
                self.set_object_focus(Some(ObjectFocus::ReadOnly(next - outlines.len())));
            }
            self.reveal_focus()?;
            return self.changed();
        }
        if command && let Key::Character(key) = key {
            match key.to_lowercase().as_str() {
                "n" if shift => {
                    let position = if let Some(focus) = self.object_focus {
                        let rect = self.object_rect(focus);
                        [rect[2] + 24.0, rect[1]]
                    } else {
                        let bounds = self.editor.active_outline().bounds();
                        [bounds.x1 as f32 + 24.0, bounds.y0 as f32]
                    };
                    self.editor.place_caret(
                        &mut self.engine,
                        snap_to_grid(position, self.editor.margin_origin()),
                        DEFAULT_OUTLINE_WIDTH,
                    )?;
                    self.set_object_focus(None);
                }
                "a" => self.editor.select_all()?,
                "z" => {
                    if shift {
                        self.editor.redo(&mut self.engine)?;
                    } else {
                        self.editor.undo(&mut self.engine)?;
                    }
                    if let Some(ObjectFocus::Image(id)) = self.object_focus
                        && self.editor.image_placement(id).is_none()
                    {
                        self.set_object_focus(None);
                    }
                }
                "c" | "x" => {
                    let [anchor, focus] = self.editor.selection().positions;
                    let selected = self
                        .editor
                        .active_outline()
                        .document()
                        .slice(anchor.min(focus)..anchor.max(focus))?;
                    let text = selected
                        .iter()
                        .map(|paragraph| {
                            paragraph
                                .project()
                                .map(|projection| projection.text().text().to_owned())
                        })
                        .collect::<Result<Vec<_>, _>>()?
                        .join("\n");
                    if !text.is_empty() {
                        self.clipboard.set_text(text)?;
                        if key.eq_ignore_ascii_case("x") {
                            self.editor.insert(&mut self.engine, "")?;
                        }
                    }
                }
                "v" => {
                    let text = self.clipboard.get_text()?;
                    self.editor.commit_text(&mut self.engine, text)?;
                }
                "+" | "=" => {
                    self.zoom(1.1);
                    return self.changed();
                }
                "-" => {
                    self.zoom(1.0 / 1.1);
                    return self.changed();
                }
                "0" => {
                    self.zoom((self.display_scale * 96.0 / 72.0) / self.viewport.scale);
                    return self.changed();
                }
                _ => return Ok(()),
            }
        } else {
            let movement = match key {
                Key::Named(NamedKey::ArrowLeft) => Some(if command {
                    Movement::LineStart
                } else if option {
                    Movement::WordLeft
                } else {
                    Movement::Left
                }),
                Key::Named(NamedKey::ArrowRight) => Some(if command {
                    Movement::LineEnd
                } else if option {
                    Movement::WordRight
                } else {
                    Movement::Right
                }),
                Key::Named(NamedKey::ArrowUp) => Some(if command {
                    Movement::DocumentStart
                } else if option {
                    Movement::ParagraphStart
                } else {
                    Movement::Up
                }),
                Key::Named(NamedKey::ArrowDown) => Some(if command {
                    Movement::DocumentEnd
                } else if option {
                    Movement::ParagraphEnd
                } else {
                    Movement::Down
                }),
                Key::Named(NamedKey::Home) if shift => Some(Movement::DocumentStart),
                Key::Named(NamedKey::End) if shift => Some(Movement::DocumentEnd),
                Key::Character(key) if self.modifiers.control_key() && !option => {
                    match key.as_str() {
                        "a" => Some(Movement::LineStart),
                        "e" => Some(Movement::LineEnd),
                        "b" => Some(Movement::Left),
                        "f" => Some(Movement::Right),
                        "p" => Some(Movement::Up),
                        "n" => Some(Movement::Down),
                        _ => None,
                    }
                }
                _ => None,
            };
            if let Some(movement) = movement {
                self.editor
                    .move_selection(&mut self.engine, movement, shift)?;
            } else if self.editor.marked_range().is_none() {
                match key {
                    Key::Named(NamedKey::Backspace) if command || option => {
                        self.editor.delete_to(
                            &mut self.engine,
                            if command {
                                Movement::LineStart
                            } else {
                                Movement::WordLeft
                            },
                        )?;
                    }
                    Key::Named(NamedKey::Delete) if command || option => {
                        self.editor.delete_to(
                            &mut self.engine,
                            if command {
                                Movement::LineEnd
                            } else {
                                Movement::WordRight
                            },
                        )?;
                    }
                    Key::Named(NamedKey::Backspace) => {
                        self.editor.delete(&mut self.engine, true)?;
                    }
                    Key::Named(NamedKey::Delete) => {
                        self.editor.delete(&mut self.engine, false)?;
                    }
                    Key::Named(NamedKey::Home | NamedKey::End) => {
                        let limits = self.scroll();
                        self.viewport.origin[1] = -if key == &Key::Named(NamedKey::Home) {
                            limits.min[1]
                        } else {
                            limits.max[1]
                        };
                        return self.changed();
                    }
                    Key::Character(key) if self.modifiers.control_key() && !option => {
                        match key.as_str() {
                            "h" => {
                                self.editor.delete(&mut self.engine, true)?;
                            }
                            "d" => {
                                self.editor.delete(&mut self.engine, false)?;
                            }
                            "k" if !self
                                .editor
                                .delete_to(&mut self.engine, Movement::LineEnd)? =>
                            {
                                self.editor.delete(&mut self.engine, false)?;
                            }
                            _ => {}
                        }
                    }
                    Key::Named(NamedKey::Enter) => {
                        self.editor.enter(&mut self.engine, shift)?;
                    }
                    Key::Named(NamedKey::Tab) => {
                        self.editor.tab(&mut self.engine, shift)?;
                    }
                    _ if !command && !self.modifiers.control_key() => {
                        if let Some(text) = text
                            .filter(|text| !text.is_empty() && !text.chars().any(char::is_control))
                        {
                            self.editor.insert(&mut self.engine, text)?;
                        }
                    }
                    _ => {}
                }
            } else if key == &Key::Named(NamedKey::Escape) {
                self.editor.cancel_composition(&mut self.engine)?;
            }
        }
        self.reveal_focus()?;
        self.changed()
    }
}

impl App {
    fn close(&self, event_loop: &ActiveEventLoop) {
        if self.state.as_ref().is_none_or(|state| {
            state.session.is_some()
                || state
                    .editor
                    .caret_outline()
                    .is_none_or(TextOutline::is_empty)
                    && state.initial_date == state.editor.date().map(|date| date.timestamp())
                    && state
                        .initial_layouts
                        .iter()
                        .map(|(id, layout)| (*id, layout))
                        .eq(state.editor.object_layouts())
                    && state
                        .initial
                        .iter()
                        .map(|(id, document)| (id, document))
                        .eq(state
                            .editor
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
                if let Some(state) = &mut self.state
                    && state.object_focus.is_none()
                    && !text.is_empty()
                    && !text.chars().any(char::is_control)
                {
                    let result = (|| -> Result<(), Box<dyn Error>> {
                        state.editor.commit_text(&mut state.engine, text)?;
                        state.reveal_focus()?;
                        state.changed()
                    })();
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
        let was_marked = state.editor.marked_range().is_some();
        let result = match event.window_event {
            accesskit_winit::WindowEvent::InitialTreeRequested => state.update_accessibility(),
            accesskit_winit::WindowEvent::ActionRequested(request) => state.access_action(request),
            accesskit_winit::WindowEvent::AccessibilityDeactivated => {
                state.accessibility.deactivate();
                Ok(())
            }
        };
        if was_marked && state.editor.marked_range().is_none() {
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
            !matches!(event, WindowEvent::Ime(_)) && state.editor.marked_range().is_some();
        let result = (|| -> Result<(), Box<dyn Error>> {
            match event {
                WindowEvent::RedrawRequested => state.draw()?,
                WindowEvent::Resized(size) => {
                    state.viewport.size = [size.width, size.height];
                    if size.width > 0 && size.height > 0 {
                        state.config.width = size.width;
                        state.config.height = size.height;
                        state
                            .surface
                            .configure(&state.renderer.device, &state.config);
                        state.changed()?;
                        // Present inside AppKit's resize transaction; a redraw on the next turn
                        // lets the window show the previous frame at the new size.
                        state.draw()?;
                    }
                }
                WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                    let scale = scale_factor as f32;
                    let ratio = scale / state.display_scale;
                    state.viewport.scale *= ratio;
                    state.viewport.origin[0] *= ratio;
                    state.viewport.origin[1] *= ratio;
                    state.display_scale = scale;
                    state.renderer.clear_glyph_cache();
                    state.changed()?;
                }
                WindowEvent::Focused(focused) => {
                    state.focused = focused;
                    if !focused {
                        state.drag = None;
                    }
                    state.changed()?;
                }
                WindowEvent::Occluded(occluded) => {
                    trace_input(&("Window occluded", occluded));
                    state.occluded = occluded;
                    if !occluded {
                        state.changed()?;
                    }
                }
                WindowEvent::ModifiersChanged(modifiers) => {
                    state.modifiers = modifiers.state();
                    if matches!(state.drag, Some(Drag::Outline { .. })) {
                        state.changed()?;
                    }
                }
                WindowEvent::CursorLeft { .. } => {
                    state.pointer_inside = false;
                    state.window.request_redraw();
                }
                WindowEvent::CursorMoved { position, .. } => {
                    state.pointer_inside = true;
                    state.pointer = [position.x as f32, position.y as f32];
                    let hit = state.hit_test(state.viewport.document_point(state.pointer));
                    let scrollbar = state
                        .scroll()
                        .hit_test(state.viewport, state.display_scale, state.pointer)
                        .is_some();
                    state.window.set_cursor(match (&state.drag, hit) {
                        (Some(Drag::Scrollbar { .. }), _) => winit::window::CursorIcon::Default,
                        (None, _) if scrollbar => winit::window::CursorIcon::Default,
                        (Some(Drag::Image { handle, .. }), _) => handle_cursor(*handle),
                        (None, Some(Hit::Image { handle, .. })) => handle_cursor(handle),
                        (Some(Drag::Resize { .. }), _) | (None, Some(Hit::Resize { .. })) => {
                            winit::window::CursorIcon::EwResize
                        }
                        (Some(Drag::Outline { .. }), _) | (None, Some(Hit::Handle { .. })) => {
                            winit::window::CursorIcon::Move
                        }
                        (None, Some(Hit::Date(_))) => winit::window::CursorIcon::Pointer,
                        (None, Some(Hit::ReadOnly(_))) => winit::window::CursorIcon::Default,
                        _ => winit::window::CursorIcon::Text,
                    });
                    match &mut state.drag {
                        Some(Drag::Scrollbar { axis, grab }) => {
                            let (axis, grab) = (*axis, *grab);
                            state.scroll().drag(
                                &mut state.viewport,
                                state.display_scale,
                                axis,
                                state.pointer[axis],
                                grab,
                            );
                            state.changed()?;
                        }
                        Some(Drag::Text { anchor, unit }) => {
                            let point = state.viewport.document_point(state.pointer);
                            let origin = state.editor.active_outline().origin();
                            let target = state.editor.selection_at(
                                point[0] - origin[0],
                                point[1] - origin[1],
                                *unit,
                            )?;
                            state
                                .editor
                                .select(drag_selection(*anchor, target, *unit))?;
                            state.changed()?;
                        }
                        Some(
                            Drag::Outline { pending_press, .. } | Drag::Image { pending_press, .. },
                        ) => {
                            if pending_press.is_some_and(|press| {
                                (0..2).any(|axis| {
                                    (state.pointer[axis] - press[axis]).abs()
                                        > 2.0 * state.display_scale
                                })
                            }) {
                                *pending_press = None;
                            }
                            state.changed()?;
                        }
                        Some(Drag::Resize { outline, grab }) => {
                            let point = state.viewport.document_point(state.pointer);
                            let width =
                                (point[0] - state.editor.active_outline().origin()[0] - *grab)
                                    .max(36.0);
                            if outline.is_some()
                                || (width - state.editor.active_outline().bounds().width() as f32)
                                    .abs()
                                    * state.viewport.scale
                                    > 2.0 * state.display_scale
                            {
                                *outline = Some(Box::new(
                                    state.editor.preview_resize(&mut state.engine, width)?,
                                ));
                                state.changed()?;
                            }
                        }
                        None => state.window.request_redraw(),
                    }
                }
                WindowEvent::MouseInput {
                    state: button_state,
                    button: MouseButton::Left,
                    ..
                } => {
                    let point = state.viewport.document_point(state.pointer);
                    if button_state == ElementState::Pressed {
                        let now = Instant::now();
                        let count = state
                            .last_click
                            .filter(|(time, point, _)| {
                                now.duration_since(*time) <= macos::double_click_interval()
                                    && (0..2).all(|axis| {
                                        (point[axis] - state.pointer[axis]).abs()
                                            <= 4.0 * state.display_scale
                                    })
                            })
                            .map_or(1, |(_, _, count)| (count % 3) + 1);
                        state.last_click = Some((now, state.pointer, count));
                        let scrollbar = state
                            .scroll()
                            .hit_test(state.viewport, state.display_scale, state.pointer)
                            .map(|(axis, grab)| Drag::Scrollbar { axis, grab });
                        if scrollbar.is_some() {
                            state.drag = scrollbar;
                        } else {
                            match state.hit_test(point) {
                                Some(Hit::Date(field)) => state.edit_date(field)?,
                                Some(Hit::ReadOnly(index)) => {
                                    state.set_object_focus(Some(ObjectFocus::ReadOnly(index)))
                                }
                                Some(Hit::Image { id, handle }) => {
                                    state.set_object_focus(Some(ObjectFocus::Image(id)));
                                    state.drag = Some(Drag::Image {
                                        id,
                                        handle,
                                        press: point,
                                        pending_press: Some(state.pointer),
                                    });
                                }
                                Some(Hit::Handle { id, grab }) => {
                                    state.set_object_focus(None);
                                    state.editor.focus_outline(id)?;
                                    state.drag = Some(Drag::Outline {
                                        id,
                                        grab,
                                        pending_press: Some(state.pointer),
                                    });
                                }
                                Some(Hit::Resize { id, grab }) => {
                                    state.set_object_focus(None);
                                    state.editor.focus_outline(id)?;
                                    state.drag = Some(Drag::Resize {
                                        outline: None,
                                        grab,
                                    });
                                }
                                Some(Hit::Text { id, point }) => {
                                    let extend = state.modifiers.shift_key()
                                        && state.object_focus.is_none()
                                        && id == state.editor.active_outline().id;
                                    state.set_object_focus(None);
                                    state.editor.focus_outline(id)?;
                                    let previous = state.editor.selection();
                                    state.editor.select_below(&mut state.engine, id, point)?;
                                    let unit = match count {
                                        2 => SelectionUnit::Word,
                                        3 => SelectionUnit::Paragraph,
                                        _ => SelectionUnit::Grapheme,
                                    };
                                    let selection =
                                        state.editor.selection_at(point[0], point[1], unit)?;
                                    let selection = if extend {
                                        Selection {
                                            positions: [
                                                previous.positions[0],
                                                selection.positions[1],
                                            ],
                                            affinities: [
                                                previous.affinities[0],
                                                selection.affinities[1],
                                            ],
                                        }
                                    } else {
                                        selection
                                    };
                                    state.editor.select(selection)?;
                                    state.drag = Some(Drag::Text {
                                        anchor: selection,
                                        unit,
                                    });
                                }
                                None => {
                                    let position = [
                                        point[0],
                                        point[1] - 7.0 * state.display_scale / state.viewport.scale,
                                    ];
                                    let position = if state.modifiers.alt_key() {
                                        position
                                    } else {
                                        snap_to_grid(position, state.editor.margin_origin())
                                    };
                                    state.editor.place_caret(
                                        &mut state.engine,
                                        position,
                                        DEFAULT_OUTLINE_WIDTH,
                                    )?;
                                    state.set_object_focus(None);
                                    state.drag = Some(Drag::Text {
                                        anchor: state.editor.selection(),
                                        unit: SelectionUnit::Grapheme,
                                    });
                                }
                            }
                        }
                    } else {
                        let preview = state.preview();
                        if let Some((id, origin, size)) = state.image_preview() {
                            state.editor.place_image(id, origin, size)?;
                        }
                        if let Some(Drag::Resize {
                            outline: Some(outline),
                            ..
                        }) = state.drag.take()
                        {
                            state
                                .editor
                                .resize(&mut state.engine, outline.bounds().width() as f32)?;
                        }
                        if let Some((id, origin)) = preview {
                            state.editor.move_outline(id, origin)?;
                        }
                    }
                    state.changed()?;
                }
                WindowEvent::MouseWheel { delta, .. } => {
                    let dpr = state.window.scale_factor() as f32;
                    let delta = match delta {
                        MouseScrollDelta::LineDelta(x, y) => [x * 32.0 * dpr, y * 32.0 * dpr],
                        MouseScrollDelta::PixelDelta(p) => [p.x as f32, p.y as f32],
                    };
                    if state.modifiers.super_key() {
                        state.zoom((delta[1] * 0.005).exp());
                    } else {
                        state.viewport.origin[0] += delta[0];
                        state.viewport.origin[1] += delta[1];
                    }
                    state.changed()?;
                }
                WindowEvent::KeyboardInput { event, .. }
                    if event.state == ElementState::Pressed =>
                {
                    state.key(&event.logical_key, event.text.as_deref())?;
                }
                WindowEvent::Ime(_) if state.object_focus.is_some() => {}
                WindowEvent::Ime(Ime::Preedit(text, cursor)) => {
                    if text.is_empty() {
                        state.editor.cancel_composition(&mut state.engine)?;
                    } else {
                        let (start, end) = cursor.unwrap_or((text.len(), text.len()));
                        let start: u32 = text
                            .get(..start)
                            .ok_or("IME range splits UTF-8")?
                            .encode_utf16()
                            .count()
                            .try_into()?;
                        let end: u32 = text
                            .get(..end)
                            .ok_or("IME range splits UTF-8")?
                            .encode_utf16()
                            .count()
                            .try_into()?;
                        state.editor.compose(&mut state.engine, text, start..end)?;
                    }
                    state.reveal_focus()?;
                    state.changed()?;
                }
                WindowEvent::Ime(Ime::Commit(text)) => {
                    state.editor.commit_text(&mut state.engine, text)?;
                    state.reveal_focus()?;
                    state.changed()?;
                }
                WindowEvent::Ime(Ime::Disabled) => {
                    state.editor.cancel_composition(&mut state.engine)?;
                    state.reveal_focus()?;
                    state.changed()?;
                }
                _ => {}
            }
            Ok(())
        })();
        if was_marked && state.editor.marked_range().is_none() {
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
        let [anchor, focus] = state.editor.selection().positions;
        if state.focused && !state.occluded && state.object_focus.is_none() && anchor == focus {
            let now = Instant::now();
            if now >= state.blink_at {
                state.caret = !state.caret;
                state.blink_at = now + Duration::from_millis(500);
                state.window.request_redraw();
            }
            event_loop.set_control_flow(ControlFlow::WaitUntil(state.blink_at));
        } else {
            event_loop.set_control_flow(ControlFlow::Wait);
        }
    }
}

#[derive(Debug, PartialEq)]
enum Hit {
    Date(DateField),
    Resize {
        id: onestore::ExGuid,
        grab: f32,
    },
    Text {
        id: onestore::ExGuid,
        point: [f32; 2],
    },
    Handle {
        id: onestore::ExGuid,
        grab: [f32; 2],
    },
    ReadOnly(usize),
    /// `handle` is [0, 0] on the picture and a direction on the selected picture's handles.
    Image {
        id: onestore::ExGuid,
        handle: [i8; 2],
    },
}

fn page_hit_test(
    editor: &CanvasEditor,
    scene: Option<&(PageScene, [f32; 2])>,
    point: [f32; 2],
    pixel: f32,
) -> Option<Hit> {
    /// Grips and text resolve front to back before any outline's padding, as a native
    /// width handle stays reachable under the next outline's left padding; the typing room
    /// below an outline comes last.
    #[derive(Clone, Copy, PartialEq)]
    enum Layer {
        Grips,
        Body,
        Below,
    }
    let hit = |outline: &TextOutline, offset: [f32; 2], layer: Layer| {
        if layer == Layer::Below {
            let outline = editor
                .outlines()
                .iter()
                .find(|source| source.id == outline.id)?;
            let local = [
                point[0] - offset[0] - outline.origin()[0],
                point[1] - offset[1] - outline.origin()[1],
            ];
            return outline.contains_extension(local).then_some(Hit::Text {
                id: outline.id,
                point: local,
            });
        }
        let (bounds, body_top) = outline_chrome(outline, pixel);
        let local = [point[0] - offset[0], point[1] - offset[1]];
        let [x, y] = local;
        if layer == Layer::Grips {
            // Width handles: 15 px inside to 6 px outside the header's right end, and 6 px
            // either side of the right border below it.
            let inside = if y < body_top { 15.0 } else { 6.0 };
            if !outline.title
                && (bounds[2] - inside * pixel..=bounds[2] + 6.0 * pixel).contains(&x)
                && (bounds[1]..=bounds[3]).contains(&y)
            {
                return Some(Hit::Resize {
                    id: outline.id,
                    grab: point[0] - outline.bounds().x1 as f32,
                });
            }
            if x >= bounds[0] && x <= bounds[2] && y >= bounds[1] && y < body_top {
                return Some(Hit::Handle {
                    id: outline.id,
                    grab: [
                        point[0] - outline.origin()[0],
                        point[1] - outline.origin()[1],
                    ],
                });
            }
            let text = outline.bounds();
            if !(text.x0..=text.x1).contains(&f64::from(x))
                || !(text.y0..=text.y1).contains(&f64::from(y))
            {
                return None;
            }
        }
        if (x >= bounds[0] && x <= bounds[2] && y >= body_top && y <= bounds[3])
            || outline.layouts().any(|(_, paragraph)| {
                paragraph.tags.iter().any(|tag| {
                    let origin = outline.origin();
                    let x = origin[0] + outline.shaped().tag_column_offset() + tag.origin[0];
                    let y = origin[1] + paragraph.origin[1] + tag.origin[1];
                    let size = canvas::outline::ParagraphTag::SIZE;
                    (x..=x + size).contains(&local[0]) && (y..=y + size).contains(&local[1])
                })
            })
        {
            return Some(Hit::Text {
                id: outline.id,
                point: [
                    local[0] - outline.origin()[0],
                    local[1] - outline.origin()[1],
                ],
            });
        }
        None
    };
    let pass = |layer| {
        if let Some(hit) = editor
            .visible_outlines()
            .rev()
            .filter(|outline| scene.is_none() || !editor.has_page_outline(outline.id))
            .find_map(|outline| hit(outline, [0.0; 2], layer))
        {
            return Some(hit);
        }
        let (scene, offset) = scene?;
        match scene.hit_test(
            [point[0] - offset[0], point[1] - offset[1]],
            Some(editor),
            |id| {
                editor
                    .visible_outlines()
                    .find(|outline| outline.id == id)
                    .and_then(|outline| hit(outline, *offset, layer))
            },
        )? {
            canvas::gpu::page::SceneHit::Outline(hit) => Some(hit),
            canvas::gpu::page::SceneHit::Date(field) => Some(Hit::Date(field)),
            canvas::gpu::page::SceneHit::ReadOnly(index) => Some(Hit::ReadOnly(index)),
            canvas::gpu::page::SceneHit::Image(id) => Some(Hit::Image { id, handle: [0, 0] }),
        }
    };
    [Layer::Grips, Layer::Body, Layer::Below]
        .into_iter()
        .find_map(pass)
}

fn page_primitives<'a>(
    editor: &'a CanvasEditor,
    scene: Option<&'a (PageScene, [f32; 2])>,
    preview: Option<PointerFeedback<'a>>,
    object_focus: Option<ObjectFocus>,
    show_caret: bool,
    scale: f32,
    display_scale: f32,
) -> Result<Vec<Primitive<'a>>, Box<dyn Error>> {
    let mut primitives = Vec::new();
    let paint = |id, offset: [f32; 2], primitives: &mut Vec<_>| {
        let outline = editor
            .visible_outlines()
            .find(|outline| outline.id == id)
            .ok_or(canvas::gpu::page::SceneError::MissingOutline)?;
        let outline = match preview {
            Some(PointerFeedback::Resize(resized)) if resized.id == outline.id => resized,
            _ => outline,
        };
        let origin = match preview {
            Some(PointerFeedback::Move(id, origin)) if id == outline.id => origin,
            _ => outline.origin(),
        };
        if (object_focus.is_none() && outline.id == editor.active_outline().id)
            || matches!(preview, Some(PointerFeedback::Hover(id) | PointerFeedback::Move(id, _)) if id == outline.id)
            || matches!(preview, Some(PointerFeedback::Resize(resized)) if resized.id == outline.id)
        {
            append_outline_chrome(
                outline,
                [origin[0] + offset[0], origin[1] + offset[1]],
                display_scale / scale,
                primitives,
            );
        }
        append_outline(
            (object_focus.is_none()
                && outline.id == editor.active_outline().id
                && !matches!(preview, Some(PointerFeedback::Resize(_))))
            .then_some(editor),
            outline,
            [origin[0] + offset[0], origin[1] + offset[1]],
            show_caret,
            scale,
            display_scale / scale,
            primitives,
        )?;
        if let Some((scene, _)) = scene {
            scene.append_outline_objects(
                outline.shaped(),
                [origin[0] + offset[0], origin[1] + offset[1]],
                primitives,
            );
        }
        Ok::<_, Box<dyn Error>>(())
    };
    if let Some((scene, origin)) = scene {
        let moving = match preview {
            Some(PointerFeedback::Image(id, origin, size)) => Some((id, image_rect(origin, size))),
            _ => None,
        };
        scene.append_primitives_with(&mut primitives, *origin, Some(editor), moving, &paint)?;
    }
    for outline in editor
        .visible_outlines()
        .filter(|outline| scene.is_none() || !editor.has_page_outline(outline.id))
    {
        paint(outline.id, [0.0; 2], &mut primitives)?;
    }
    if object_focus.is_none()
        && let Some(outline) = editor.caret_outline()
    {
        append_outline(
            Some(editor),
            outline,
            outline.origin(),
            show_caret,
            scale,
            display_scale / scale,
            &mut primitives,
        )?;
    }
    if let Some(ObjectFocus::Image(id)) = object_focus {
        let (origin, size) = match preview {
            Some(PointerFeedback::Image(moving, origin, size)) if moving == id => (origin, size),
            _ => editor
                .image_placement(id)
                .ok_or("The selected picture is missing.")?,
        };
        append_image_chrome(
            image_rect(origin, size),
            display_scale / scale,
            &mut primitives,
        );
    }
    if let Some(ObjectFocus::ReadOnly(index)) = object_focus {
        let (scene, offset) = scene.unwrap();
        let [x0, y0, x1, y1] = scene.read_only(Some(editor)).nth(index).unwrap().rect();
        let [x0, y0, x1, y1] = [
            x0 + offset[0],
            y0 + offset[1],
            x1 + offset[0],
            y1 + offset[1],
        ];
        let border = 2.0 / scale;
        for rect in [
            [x0, y0, x1, y0 + border],
            [x0, y1 - border, x1, y1],
            [x0, y0, x0 + border, y1],
            [x1 - border, y0, x1, y1],
        ] {
            primitives.push(Primitive::Rect {
                rect,
                color: [0.25, 0.45, 0.7, 1.0],
            });
        }
    }
    Ok(primitives)
}

/// Native picture handles sit on a selection border drawn 5 px outside the picture, named
/// by their direction from its center.
fn image_handles(rect: [f32; 4], pixel: f32) -> impl Iterator<Item = ([i8; 2], [f32; 2])> {
    let border = [
        rect[0] - 5.0 * pixel,
        rect[1] - 5.0 * pixel,
        rect[2] + 5.0 * pixel,
        rect[3] + 5.0 * pixel,
    ];
    [-1, 0, 1]
        .into_iter()
        .flat_map(|y| [-1, 0, 1].map(|x| [x, y]))
        .filter(|handle| *handle != [0, 0])
        .map(move |handle| {
            (
                handle,
                std::array::from_fn(|axis| {
                    let [start, end] = [border[axis], border[axis + 2]];
                    start + (end - start) * f32::from(handle[axis] + 1) * 0.5
                }),
            )
        })
}

fn image_handle_at(rect: [f32; 4], pixel: f32, point: [f32; 2]) -> Option<[i8; 2]> {
    image_handles(rect, pixel)
        .find(|(_, center)| (0..2).all(|axis| (point[axis] - center[axis]).abs() <= 5.0 * pixel))
        .map(|(handle, _)| handle)
}

fn handle_cursor(handle: [i8; 2]) -> winit::window::CursorIcon {
    use winit::window::CursorIcon;
    match handle {
        [0, 0] => CursorIcon::Move,
        [_, 0] => CursorIcon::EwResize,
        [0, _] => CursorIcon::NsResize,
        [x, y] if x == y => CursorIcon::NwseResize,
        _ => CursorIcon::NeswResize,
    }
}

fn image_rect(origin: [f32; 2], size: [f32; 2]) -> [f32; 4] {
    [
        origin[0],
        origin[1],
        origin[0] + size[0],
        origin[1] + size[1],
    ]
}

/// Drags a picture's `handle` by `delta`: edges stretch one axis, corners keep the aspect
/// ratio at the larger of the two scales, and the opposite side stays fixed.
fn resize_image(
    origin: [f32; 2],
    size: [f32; 2],
    handle: [i8; 2],
    delta: [f32; 2],
) -> ([f32; 2], [f32; 2]) {
    let mut scale: [f32; 2] = std::array::from_fn(|axis| {
        (size[axis] + f32::from(handle[axis]) * delta[axis]) / size[axis]
    });
    if !handle.contains(&0) {
        scale = [scale[0].max(scale[1]); 2];
    }
    let resized: [f32; 2] = std::array::from_fn(|axis| (size[axis] * scale[axis]).max(1.0));
    let origin = std::array::from_fn(|axis| {
        if handle[axis] < 0 {
            origin[axis] + size[axis] - resized[axis]
        } else {
            origin[axis]
        }
    });
    (origin, resized)
}

fn append_image_chrome(rect: [f32; 4], pixel: f32, primitives: &mut Vec<Primitive<'_>>) {
    let mut tint = canvas::gpu::colorref(0x00e0d2e6);
    // Native pictures take this tint at 25% in sRGB; 10% in linear light matches it.
    tint[3] = 0.1;
    primitives.push(Primitive::Rect { rect, color: tint });
    primitives.push(Primitive::RoundedRect {
        rect: [
            rect[0] - 5.0 * pixel,
            rect[1] - 5.0 * pixel,
            rect[2] + 5.0 * pixel,
            rect[3] + 5.0 * pixel,
        ],
        radius: [0.0; 2],
        stroke: Some(Stroke::Dashed(pixel)),
        color: canvas::gpu::colorref(0x00ff9a31),
    });
    for (handle, [x, y]) in image_handles(rect, pixel) {
        let (half, radius) = if handle.contains(&0) {
            (3.5, 0.0)
        } else {
            (4.0, 4.0)
        };
        let rect = [
            x - half * pixel,
            y - half * pixel,
            x + half * pixel,
            y + half * pixel,
        ];
        primitives.push(Primitive::RoundedRect {
            rect,
            radius: [radius * pixel; 2],
            stroke: None,
            color: canvas::gpu::colorref(0x00ffefe7),
        });
        primitives.push(Primitive::RoundedRect {
            rect,
            radius: [radius * pixel; 2],
            stroke: Some(Stroke::Solid(pixel)),
            color: canvas::gpu::colorref(0x00dea67b),
        });
    }
}

fn drag_selection(anchor: Selection, target: Selection, unit: SelectionUnit) -> Selection {
    if unit == SelectionUnit::Grapheme {
        return Selection {
            positions: [anchor.positions[0], target.positions[1]],
            affinities: [anchor.affinities[0], target.affinities[1]],
        };
    }
    let backwards = target.positions[0] < anchor.positions[0].min(anchor.positions[1]);
    let start = usize::from((anchor.positions[0] > anchor.positions[1]) != backwards);
    let end = usize::from((target.positions[0] > target.positions[1]) == backwards);
    Selection {
        positions: [anchor.positions[start], target.positions[end]],
        affinities: [anchor.affinities[start], target.affinities[end]],
    }
}

fn snap_to_grid(point: [f32; 2], margin: [f32; 2]) -> [f32; 2] {
    std::array::from_fn(|axis| {
        let offset = margin[axis];
        let cell = (point[axis] - offset) / 18.0;
        let nearest = cell.round();
        // Native midpoints remain free; account for the source coordinate's float precision.
        if ((cell - nearest).abs() - 0.5).abs() <= f32::EPSILON * cell.abs().max(1.0) {
            point[axis]
        } else {
            nearest * 18.0 + offset
        }
    })
}

fn outline_chrome(outline: &TextOutline, pixel: f32) -> ([f32; 4], f32) {
    let bounds = outline.bounds();
    // Native chrome combines page-scaled gutters with a fixed screen inset.
    let inset = 5.0 * pixel;
    let body_top = bounds.y0 as f32 - inset;
    if outline.title {
        let (_, paragraph) = outline.layouts().last().unwrap();
        let bottom = outline.origin()[1] + paragraph.origin[1] + paragraph.text.height();
        return (
            [
                bounds.x0 as f32 - inset - 6.0 * pixel,
                body_top,
                bounds.x1 as f32 + inset - 2.0 * pixel,
                bottom + inset,
            ],
            body_top,
        );
    }
    (
        [
            bounds.x0 as f32 - 7.5 - inset,
            body_top - HANDLE_HEIGHT,
            bounds.x1 as f32 + inset,
            bounds.y1 as f32 + HANDLE_HEIGHT + inset,
        ],
        body_top,
    )
}

fn append_outline_chrome(
    outline: &TextOutline,
    origin: [f32; 2],
    pixel: f32,
    primitives: &mut Vec<Primitive<'_>>,
) {
    let [x, y] = origin;
    let (bounds, body_top) = outline_chrome(outline, pixel);
    let [dx, dy] = [x - outline.origin()[0], y - outline.origin()[1]];
    let [left, top, right, bottom] = [
        bounds[0] + dx,
        bounds[1] + dy,
        bounds[2] + dx,
        bounds[3] + dy,
    ];
    let body_top = body_top + dy;
    if outline.title {
        primitives.push(Primitive::RoundedRect {
            rect: [left, top, right, bottom],
            radius: [6.0 * pixel, (bottom - top) * 0.5],
            stroke: Some(Stroke::Dashed(pixel)),
            color: canvas::gpu::colorref(0x007f7f7f),
        });
        return;
    }
    primitives.push(Primitive::RoundedRect {
        rect: [left, top, right, body_top],
        radius: [3.0 * pixel; 2],
        stroke: None,
        color: canvas::gpu::colorref(0x00e8ebed),
    });
    primitives.push(Primitive::RoundedRect {
        rect: [right - 9.0 * pixel, top, right, body_top],
        radius: [3.0 * pixel; 2],
        stroke: None,
        color: canvas::gpu::colorref(0x00e5dee7),
    });
    primitives.push(Primitive::RoundedRect {
        rect: [left, top, right, bottom],
        radius: [3.0 * pixel; 2],
        stroke: Some(Stroke::Solid(pixel)),
        color: canvas::gpu::colorref(0x00d9cfd8),
    });
    let middle = (top + body_top) * 0.5;
    for offset in [-3.0, 0.0, 3.0] {
        let center = (left + right) * 0.5 + offset * pixel;
        primitives.push(Primitive::RoundedRect {
            rect: [
                center - pixel * 0.5,
                middle - pixel * 0.5,
                center + pixel * 0.5,
                middle + pixel * 0.5,
            ],
            radius: [pixel * 0.5; 2],
            stroke: None,
            color: canvas::gpu::colorref(0x00b4a5b4),
        });
    }
    for column in [0.0, 1.0, 2.0] {
        let half = (column + 0.5) * pixel;
        for x in [
            right - (8.0 - column) * pixel,
            right - (2.0 + column) * pixel,
        ] {
            primitives.push(Primitive::Rect {
                rect: [x, middle - half, x + pixel, middle + half],
                color: canvas::gpu::colorref(0x00b4a5b4),
            });
        }
    }
}

fn append_outline<'a>(
    editor: Option<&'a CanvasEditor>,
    outline: &'a TextOutline,
    origin: [f32; 2],
    show_caret: bool,
    scale: f32,
    pixel: f32,
    primitives: &mut Vec<Primitive<'a>>,
) -> Result<(), Box<dyn Error>> {
    let [x, y] = origin;
    outline.shaped().append_table_primitives(primitives, origin);
    outline
        .shaped()
        .append_background_primitives(primitives, origin);
    if let Some(editor) = editor {
        let [_, selection_color] = macos::text_colors();
        for rect in editor.selection_rects()? {
            primitives.push(Primitive::Rect {
                rect: [
                    rect.x0 as f32 + x,
                    rect.y0 as f32 + y,
                    rect.x1 as f32 + x,
                    rect.y1 as f32 + y,
                ],
                color: selection_color,
            });
        }
    }
    for (index, (_, paragraph)) in outline.layouts().enumerate() {
        outline
            .shaped()
            .append_paragraph_primitives(index, paragraph, origin, primitives);
    }
    if let Some(editor) = editor {
        for rect in editor.marked_rects()? {
            primitives.push(Primitive::Rect {
                rect: [
                    rect.x0 as f32 + x,
                    rect.y1 as f32 + y - 1.0 / scale,
                    rect.x1 as f32 + x,
                    rect.y1 as f32 + y,
                ],
                color: [0.0, 0.0, 0.0, 1.0],
            });
        }
        let [anchor, focus] = editor.selection().positions;
        if show_caret && anchor == focus {
            let rect = editor.caret(2.0 * pixel)?;
            primitives.push(Primitive::RoundedRect {
                rect: [
                    rect.x0 as f32 + x,
                    rect.y0 as f32 + y,
                    rect.x1 as f32 + x,
                    rect.y1 as f32 + y,
                ],
                radius: [pixel; 2],
                stroke: None,
                color: macos::text_colors()[0],
            });
        }
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_buttons_keep_accessibility_identity_and_match_mouse_hits_after_reflow() {
        let mut engine = TextEngine::default();
        let mut fields = [vec!["Header"], vec!["Monday", "6:14 AM"]].map(|text| {
            TextOutline::new(
                &mut engine,
                TextDocument::new(
                    text.into_iter()
                        .map(|text| Paragraph::new(text.into(), Default::default()))
                        .collect(),
                )
                .unwrap(),
                468.0,
                [0.0; 2],
            )
            .unwrap()
            .snapshot()
        });
        fields[0].title = true;
        let page = Page {
            identity: None,
            title: "Header".into(),
            created: Some(1),
            margin_origin: [36.0, 14.4],
            definitions: Default::default(),
            objects: vec![onestore::page::PageObject::Title(onestore::page::Title {
                id: onestore::ExGuid::default(),
                date: Some(fields[1].id),
                layout: Default::default(),
                outlines: fields.into(),
            })],
        };
        let (scene, mut editor) = PageScene::from_page(page, &mut engine).unwrap();
        let scene = (scene, [20.0, 40.0]);
        let viewport = Viewport {
            size: [1000, 700],
            scale: 2.0,
            origin: [-30.0, -50.0],
        };
        let mut access = accessibility::Accessibility::default();
        let mut identities = Vec::new();
        for phase in 0..3 {
            if phase == 1 {
                editor
                    .insert(&mut engine, &"A wrapped title ".repeat(25))
                    .unwrap();
            }
            if phase == 2 {
                editor
                    .change_date(&mut engine, 2, ["Wednesday".into(), "8:40 AM".into()])
                    .unwrap();
            }
            let mut update = access.update(&editor, viewport, "Header", None).unwrap();
            access.append_page_fields(&mut update, Some(&scene), &editor, viewport, None);
            let buttons: Vec<_> = update
                .nodes
                .iter()
                .filter(|(_, node)| node.role() == accesskit::Role::Button)
                .collect();
            assert_eq!(buttons.len(), 2);
            for (index, (id, node)) in buttons.iter().enumerate() {
                let field = [DateField::Date, DateField::Time][index];
                assert_eq!(access.date_for_node(*id), Some(field));
                assert!(node.supports_action(accesskit::Action::Click));
                let rect = node.bounds().unwrap();
                let point = viewport.document_point([
                    ((rect.x0 + rect.x1) * 0.5) as f32,
                    ((rect.y0 + rect.y1) * 0.5) as f32,
                ]);
                assert_eq!(
                    page_hit_test(&editor, Some(&scene), point, 0.5),
                    Some(Hit::Date(field))
                );
                if phase == 0 {
                    identities.push(*id);
                } else {
                    assert_eq!(*id, identities[index]);
                }
            }
            if phase == 2 {
                assert_eq!(buttons[1].1.value(), Some("8:40 AM"));
            }
            accesskit_consumer::Tree::new(update, true);
        }
    }

    #[test]
    fn extension_hits_yield_to_objects_and_keep_their_source_coordinate_frame() {
        let mut engine = TextEngine::default();
        let upper = TextOutline::new(
            &mut engine,
            TextDocument::new(vec![Paragraph::new("Upper".into(), Default::default())]).unwrap(),
            240.0,
            [36.0, 90.0],
        )
        .unwrap();
        let bottom = upper.bounds().y1 as f32;
        let id = upper.id;
        let lower = TextOutline::new(
            &mut engine,
            TextDocument::new(vec![Paragraph::new("Lower".into(), Default::default())]).unwrap(),
            240.0,
            [36.0, bottom + 20.0],
        )
        .unwrap();
        let lower_id = lower.id;
        let source = upper.snapshot();
        let mut editor =
            CanvasEditor::from_text_outlines(vec![lower, upper], Default::default(), None).unwrap();
        assert!(
            matches!(page_hit_test(&editor, None, [60.0, bottom + 22.0], 0.75), Some(Hit::Text { id, .. }) if id == lower_id)
        );
        editor.move_outline(lower_id, [600.0, 500.0]).unwrap();
        for pixel in [0.375, 0.75, 1.5] {
            let hit = page_hit_test(&editor, None, [60.0, bottom + 26.0], pixel).unwrap();
            assert!(
                matches!(hit, Hit::Text { id: hit_id, point } if hit_id == id && (point[1] - (bottom + 26.0 - 90.0)).abs() < 0.001)
            );
            assert_eq!(
                page_hit_test(&editor, None, [60.0, bottom + 30.0], pixel),
                None
            );
        }
        let page = onestore::page::Page {
            identity: None,
            title: String::new(),
            created: None,
            margin_origin: [0.0; 2],
            definitions: Default::default(),
            objects: vec![onestore::page::PageObject::Outline(source)],
        };
        let (scene, editor) = PageScene::from_page(page, &mut engine).unwrap();
        let offset = [50.0, 300.0];
        assert!(
            matches!(page_hit_test(&editor, Some(&(scene, offset)), [60.0 + offset[0], bottom + 26.0 + offset[1]], 0.75), Some(Hit::Text { id: hit_id, .. }) if hit_id == id)
        );
    }

    #[test]
    fn title_chrome_selects_text_and_exposes_a_named_editable_field() {
        let mut engine = TextEngine::default();
        let mut source = TextOutline::new(
            &mut engine,
            TextDocument::new(vec![Paragraph::new(
                "Header".into(),
                Format {
                    font_size: Some(8.0),
                    line_spacing: Some(20.751953),
                    ..Default::default()
                },
            )])
            .unwrap(),
            468.0,
            [36.0, 14.4],
        )
        .unwrap()
        .snapshot();
        source.title = true;
        source.min_width = Some(162.0);
        source.layout.max_width = None;
        source.layout.width_set_by_user = None;
        source.layout.max_height = Some(21.6);
        let outline = TextOutline::from_outline(&mut engine, &source, &Default::default()).unwrap();
        let editor =
            CanvasEditor::from_text_outlines(vec![outline], Default::default(), None).unwrap();
        assert_eq!(editor.active_outline().bounds().width(), 162.0);
        assert_eq!(editor.active_outline().snapshot().min_width, Some(162.0));
        let (native_rect, _) = outline_chrome(editor.active_outline(), 0.75);
        for (axis, (actual, expected)) in native_rect
            .iter()
            .zip([85.0, 98.0, 315.0, 134.0])
            .enumerate()
        {
            let origin = if axis % 2 == 0 { 48.0 } else { 83.0 };
            assert!((actual / 0.75 + origin - expected).abs() < 1.5);
        }
        let id = editor.active_outline().id;
        let (rect, _) = outline_chrome(editor.active_outline(), 1.0);
        for point in [
            [rect[0] + 1.0, rect[1] + 1.0],
            [rect[2] - 1.0, rect[1] + 1.0],
            [40.0, 20.0],
        ] {
            assert!(
                matches!(page_hit_test(&editor, None, point, 1.0), Some(Hit::Text { id: hit, .. }) if hit == id)
            );
        }
        let primitives = page_primitives(&editor, None, None, None, false, 1.0, 1.0).unwrap();
        let borders = primitives
            .iter()
            .filter_map(|p| match p {
                Primitive::RoundedRect { stroke, .. } => Some(*stroke),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(borders, [Some(Stroke::Dashed(1.0))]);
        assert!(primitives.iter().any(|primitive| matches!(primitive, Primitive::RoundedRect { rect, radius, stroke: Some(Stroke::Dashed(_)), .. } if radius[0] == 6.0 && radius[1] * 2.0 == rect[3] - rect[1])));
        let mut access = accessibility::Accessibility::default();
        let update = access
            .update(
                &editor,
                Viewport {
                    size: [800, 600],
                    scale: 1.0,
                    origin: [0.0; 2],
                },
                "Header",
                None,
            )
            .unwrap();
        assert!(update.nodes.iter().any(|(_, node)| node.role()
            == accesskit::Role::MultilineTextInput
            && node.label() == Some("Page title")));
    }

    #[test]
    fn provisional_lines_share_one_drawn_hit_tested_and_accessible_outline() {
        let mut engine = TextEngine::default();
        let mut editor = CanvasEditor::new(
            &mut engine,
            TextDocument::new(vec![Paragraph::new("body".into(), Format::default())]).unwrap(),
            240.0,
        )
        .unwrap();
        let id = editor.active_outline().id;
        for _ in 0..2 {
            editor
                .move_selection(&mut engine, Movement::Down, false)
                .unwrap();
        }
        let primitives = page_primitives(&editor, None, None, None, true, 1.0, 1.0).unwrap();
        assert_eq!(
            primitives
                .iter()
                .filter(|p| matches!(p, Primitive::Text { .. }))
                .count(),
            3
        );
        let caret = editor.caret(1.0).unwrap();
        assert!(
            matches!(page_hit_test(&editor, None, [caret.x0 as f32, ((caret.y0 + caret.y1) * 0.5) as f32], 1.0), Some(Hit::Text { id: hit, .. }) if hit == id)
        );
        let mut access = accessibility::Accessibility::default();
        let update = access
            .update(
                &editor,
                Viewport {
                    size: [800, 600],
                    scale: 1.0,
                    origin: [0.0; 2],
                },
                "Test",
                None,
            )
            .unwrap();
        let fields = update
            .nodes
            .iter()
            .filter(|(_, node)| node.role() == accesskit::Role::MultilineTextInput)
            .collect::<Vec<_>>();
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].1.value(), Some("body\n\n"));
        assert_eq!(editor.outlines()[0].document().nodes().len(), 1);
        editor
            .place_caret(&mut engine, [300.0, 200.0], 240.0)
            .unwrap();
        assert_eq!(
            editor
                .visible_outlines()
                .next()
                .unwrap()
                .document()
                .nodes()
                .len(),
            1
        );
    }

    #[test]
    fn chrome_follows_focus_hover_and_drag_without_changing_hit_geometry() {
        let mut engine = TextEngine::default();
        let outlines = [[0.0, 0.0], [220.0, 0.0]]
            .into_iter()
            .map(|origin| {
                TextOutline::new(
                    &mut engine,
                    TextDocument::new(vec![Paragraph::new("text".into(), Format::default())])
                        .unwrap(),
                    100.0,
                    origin,
                )
                .unwrap()
            })
            .collect();
        let editor = CanvasEditor::from_text_outlines(outlines, Default::default(), None).unwrap();
        let second = editor.outlines()[1].id;
        let borders = |feedback| {
            page_primitives(&editor, None, feedback, None, false, 1.0, 1.0)
                .unwrap()
                .into_iter()
                .filter_map(|p| match p {
                    Primitive::RoundedRect {
                        rect,
                        stroke: Some(Stroke::Solid(1.0)),
                        ..
                    } => Some(rect),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(borders(None).len(), 1);
        let hovered = borders(Some(PointerFeedback::Hover(second)));
        assert_eq!(hovered.len(), 2);
        let moved = borders(Some(PointerFeedback::Move(second, [240.0, 50.0])));
        assert_eq!(moved[1][0] - hovered[1][0], 20.0);
        assert_eq!(moved[1][1] - hovered[1][1], 50.0);
        assert!(
            matches!(page_hit_test(&editor, None, [225.0, 5.0], 1.0), Some(Hit::Text { id, .. }) if id == second)
        );
    }

    #[test]
    fn a_covered_width_handle_stays_reachable_beside_the_outline_above_it() {
        let mut engine = TextEngine::default();
        let pixel = 0.75;
        let mut first = TextOutline::new(
            &mut engine,
            TextDocument::new(vec![Paragraph::new("text".into(), Format::default())]).unwrap(),
            100.0,
            [36.0, 68.4],
        )
        .unwrap();
        let editor =
            CanvasEditor::from_text_outlines(vec![first.clone()], Default::default(), None)
                .unwrap();
        first = editor.preview_resize(&mut engine, 226.5).unwrap();
        let (frame, body_top) = outline_chrome(&first, pixel);
        // The casual night page: the next outline starts 7.5 pt right of the first one.
        let second = TextOutline::new(
            &mut engine,
            TextDocument::new(vec![Paragraph::new("text".into(), Format::default())]).unwrap(),
            100.0,
            [first.bounds().x1 as f32 + 7.5, 68.4],
        )
        .unwrap();
        let (first_id, second_id) = (first.id, second.id);
        let editor =
            CanvasEditor::from_text_outlines(vec![first, second], Default::default(), None)
                .unwrap();
        let header = (frame[1] + body_top) / 2.0;
        let hit = |x| page_hit_test(&editor, None, [x, header], pixel);
        assert!(
            matches!(hit(frame[2] - 12.0 * pixel), Some(Hit::Resize { id, .. }) if id == first_id)
        );
        assert!(
            matches!(hit(frame[2] - 2.0 * pixel), Some(Hit::Handle { id, .. }) if id == second_id)
        );
        let body = |x| page_hit_test(&editor, None, [x, body_top + 20.0], pixel);
        assert!(
            matches!(body(frame[2] - 4.0 * pixel), Some(Hit::Resize { id, .. }) if id == first_id)
        );
        assert!(
            matches!(body(frame[2] - 8.0 * pixel), Some(Hit::Text { id, .. }) if id == second_id)
        );
        assert!(
            matches!(body(frame[2] - 12.0 * pixel), Some(Hit::Text { id, .. }) if id == first_id)
        );
    }

    #[test]
    fn picture_handles_resize_as_onenote_does() {
        // Native drags of the 333 x 200.1 pt mockup on "av: casual night in the trees".
        let (origin, size) = ([468.0, 86.4], [333.0, 200.1]);
        let (corner_origin, corner) = resize_image(origin, size, [1, -1], [-75.0, 30.75]);
        assert!((corner[0] - 281.827).abs() < 0.01 && (corner[1] - 169.351).abs() < 0.01);
        assert!((corner_origin[1] - 117.15).abs() < 0.01 && corner_origin[0] == 468.0);
        assert_eq!(
            resize_image(origin, size, [1, 0], [46.5, 10.0]),
            (origin, [379.5, 200.1])
        );
        let (left_origin, left) = resize_image(origin, size, [-1, 0], [400.0, 0.0]);
        assert_eq!(left, [1.0, 200.1]);
        assert_eq!(left_origin[0], 468.0 + 333.0 - 1.0);
        let pixel = 0.75;
        let rect = image_rect(origin, size);
        assert_eq!(
            image_handle_at(rect, pixel, [468.0 - 3.75, 86.4 - 3.75]),
            Some([-1, -1])
        );
        assert_eq!(
            image_handle_at(rect, pixel, [468.0 + 166.5, 286.5 + 3.75]),
            Some([0, 1])
        );
        assert_eq!(image_handle_at(rect, pixel, [600.0, 150.0]), None);
    }

    #[test]
    fn grouped_drag_keeps_the_initial_word_when_reversing_direction() {
        let selection = |start, end| {
            Selection::from([start, end].map(|offset| canvas::document::TextPosition {
                paragraph: 0,
                offset,
            }))
        };
        let anchor = selection(6, 10);
        assert_eq!(
            drag_selection(anchor, selection(11, 16), SelectionUnit::Word).positions,
            selection(6, 16).positions
        );
        assert_eq!(
            drag_selection(anchor, selection(0, 5), SelectionUnit::Word).positions,
            selection(10, 0).positions
        );
        assert_eq!(
            drag_selection(anchor, anchor, SelectionUnit::Word).positions,
            anchor.positions
        );
        let anchor = selection(10, 3);
        assert_eq!(
            drag_selection(anchor, selection(7, 7), SelectionUnit::Grapheme).positions,
            selection(10, 7).positions
        );
    }

    const DEFAULT_MARGIN: [f32; 2] = [36.0, 14.4];

    #[test]
    fn native_grid_follows_the_page_margin_origin() {
        // "av: casual night in the trees": margin 36.75; a 22.5 x 15 pt drag lands one cell on.
        let margin = [36.75, 14.4];
        let result = snap_to_grid([468.75 + 22.5, 86.4 + 15.0], margin);
        assert!((result[0] - 486.75).abs() < 0.0001);
        assert!((result[1] - 104.4).abs() < 0.0001);
    }

    #[test]
    fn native_grid_matches_drag_offsets_midpoints_and_zoom() {
        for (pixels, points) in [
            (5.0, 0.0),
            (6.0, 0.0),
            (7.0, 0.0),
            (8.0, 0.0),
            (11.0, 0.0),
            (12.0, 9.0),
            (13.0, 18.0),
            (17.0, 18.0),
            (18.0, 18.0),
            (19.0, 18.0),
            (25.0, 18.0),
            (37.0, 36.0),
            (-6.0, 0.0),
            (-7.0, 0.0),
            (-12.0, -9.0),
            (-13.0, -18.0),
            (-19.0, -18.0),
        ] {
            let origin = [486.0, 230.40001];
            let proposed = origin.map(|v| v + pixels * 72.0 / 96.0);
            let result = snap_to_grid(proposed, DEFAULT_MARGIN);
            for axis in 0..2 {
                assert!((result[axis] - origin[axis] - points).abs() < 0.00004);
            }
        }
        for pixels in [6.0, 7.0, 12.0, 13.0, 19.0] {
            let result = snap_to_grid(
                [491.25 + pixels * 0.75, 235.65 + pixels * 0.75],
                DEFAULT_MARGIN,
            );
            assert!((result[0] - 504.0).abs() < 0.00004);
            assert!((result[1] - 248.4).abs() < 0.00004);
        }
        for (scale, delta, expected) in [
            (96.0 / 72.0, [27.0, 31.0], [54.0, 104.4]),
            (96.0 / 72.0, [13.0, 17.0], [54.0, 104.4]),
            (192.0 / 72.0, [13.0, 17.0], [36.0, 104.4]),
        ] {
            for dpr in [1.0, 2.0] {
                let origin = [36.0, 90.0];
                let proposed =
                    std::array::from_fn(|axis| origin[axis] + delta[axis] * dpr / (scale * dpr));
                let result = snap_to_grid(proposed, DEFAULT_MARGIN);
                for axis in 0..2 {
                    assert!((result[axis] - expected[axis]).abs() < 0.00004);
                }
            }
        }
        for point in [[9.0, 5.4], [-9.0, -12.6], [423.0, 239.4]] {
            assert_eq!(snap_to_grid(point, DEFAULT_MARGIN), point);
        }
    }

    #[test]
    fn native_fresh_click_grid_uses_screen_offset_at_both_zoom_levels() {
        for (x, y, scale, expected) in [
            (900.0, 500.0, 96.0 / 72.0, [639.0, 302.4]),
            (901.0, 500.0, 96.0 / 72.0, [648.0, 302.4]),
            (906.0, 500.0, 96.0 / 72.0, [648.0, 302.4]),
            (913.0, 500.0, 96.0 / 72.0, [648.0, 302.4]),
            (900.0, 501.0, 96.0 / 72.0, [639.0, 302.4]),
            (900.0, 506.0, 96.0 / 72.0, [639.0, 320.4]),
            (900.0, 520.0, 96.0 / 72.0, [639.0, 320.4]),
            (900.0, 500.0, 192.0 / 72.0, [324.0, 158.4]),
        ] {
            for dpr in [1.0, 2.0] {
                let viewport = Viewport {
                    size: [(1600.0 * dpr) as u32, (900.0 * dpr) as u32],
                    origin: [48.0 * dpr, 83.0 * dpr],
                    scale: scale * dpr,
                };
                let point = viewport.document_point([x * dpr, y * dpr]);
                let result = snap_to_grid(
                    [point[0], point[1] - 7.0 * dpr / viewport.scale],
                    DEFAULT_MARGIN,
                );
                for axis in 0..2 {
                    assert!((result[axis] - expected[axis]).abs() < 0.00004);
                }
            }
        }
    }

    #[test]
    fn blank_caret_and_composition_have_accessible_text_without_outline_chrome() {
        let mut engine = TextEngine::default();
        let mut editor = CanvasEditor::new(
            &mut engine,
            TextDocument::new(vec![Paragraph::new(String::new(), Default::default())]).unwrap(),
            240.0,
        )
        .unwrap();
        editor
            .place_caret(&mut engine, [120.0, 100.0], 240.0)
            .unwrap();
        let mut access = accessibility::Accessibility::default();
        let viewport = Viewport {
            size: [1000, 720],
            scale: 96.0 / 72.0,
            origin: [48.0; 2],
        };
        let initial = access.update(&editor, viewport, "Page", None).unwrap();
        let id = editor.active_outline().id;
        assert_eq!(access.outline_for_node(initial.focus), Some(id));
        let rectangles = |editor: &CanvasEditor| {
            page_primitives(editor, None, None, None, true, viewport.scale, 1.0)
                .unwrap()
                .into_iter()
                .filter_map(|p| match p {
                    Primitive::Rect { rect, color }
                    | Primitive::RoundedRect { rect, color, .. } => Some((rect, color)),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(rectangles(&editor).len(), 1);
        assert_eq!(
            page_hit_test(&editor, None, [120.0, 100.0], 1.0 / viewport.scale),
            None
        );
        editor.compose(&mut engine, "に".into(), 1..1).unwrap();
        assert!(editor.outlines().is_empty());
        assert!(
            rectangles(&editor)
                .iter()
                .all(|(_, color)| *color == [0.0, 0.0, 0.0, 1.0]
                    || *color == macos::text_colors()[0])
        );
        let preedit = access.update(&editor, viewport, "Page", None).unwrap();
        assert_eq!(preedit.focus, initial.focus);
        editor.commit_text(&mut engine, "日本".into()).unwrap();
        let committed = access.update(&editor, viewport, "Page", None).unwrap();
        assert_eq!(committed.focus, initial.focus);
        assert!(rectangles(&editor).len() > 1);
        editor.undo(&mut engine).unwrap();
        assert!(editor.outlines().is_empty());
        assert_eq!(rectangles(&editor).len(), 1);
        let empty = access.update(&editor, viewport, "Page", None).unwrap();
        assert_eq!(empty.focus, committed.focus);
        assert_eq!(access.outline_for_node(committed.focus), Some(id));
        editor.redo(&mut engine).unwrap();
        editor.select_all().unwrap();
        editor.delete(&mut engine, true).unwrap();
        assert!(editor.outlines().is_empty());
        assert_eq!(rectangles(&editor).len(), 1);
        let retired = access.update(&editor, viewport, "Page", None).unwrap();
        assert_eq!(retired.focus, committed.focus);
        assert_eq!(access.outline_for_node(retired.focus), Some(id));
        editor.undo(&mut engine).unwrap();
        assert!(rectangles(&editor).len() > 1);
        let restored = access.update(&editor, viewport, "Page", None).unwrap();
        assert_eq!(restored.focus, committed.focus);
        editor.redo(&mut engine).unwrap();
        assert_eq!(rectangles(&editor).len(), 1);
        assert!(editor.caret_outline().unwrap().is_empty());
        editor.insert(&mut engine, "   ").unwrap();
        assert!(editor.caret_outline().unwrap().is_empty());
        editor.compose(&mut engine, "に".into(), 1..1).unwrap();
        assert!(!editor.caret_outline().unwrap().is_empty());
    }

    #[test]
    fn table_glyphs_highlights_and_selection_share_cell_paint_bounds() {
        let mut engine = TextEngine::default();
        let mut editor = CanvasEditor::new(
            &mut engine,
            TextDocument::new(vec![Paragraph::new(
                "M".into(),
                Format {
                    font_size: Some(130.0),
                    highlight: Some(0xffff),
                    ..Default::default()
                },
            )])
            .unwrap(),
            180.0,
        )
        .unwrap();
        editor
            .move_selection(&mut engine, Movement::LineEnd, false)
            .unwrap();
        editor.tab(&mut engine, false).unwrap();
        editor.insert(&mut engine, "R").unwrap();
        editor.tab(&mut engine, true).unwrap();
        let mut primitives = Vec::new();
        append_outline(
            Some(&editor),
            editor.active_outline(),
            [24.0, 48.0],
            false,
            1.0,
            1.0,
            &mut primitives,
        )
        .unwrap();
        let bounds = editor.active_outline().shaped().tables[0]
            .cells
            .iter()
            .map(|cell| {
                let [left, top, right, bottom] = cell.text_bounds();
                [left + 24.0, top + 48.0, right + 24.0, bottom + 48.0]
            })
            .collect::<Vec<_>>();
        let text = primitives
            .iter()
            .filter_map(|primitive| match primitive {
                Primitive::Text { clip, .. } => Some(clip.unwrap()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(text, bounds);
        let highlights = primitives
            .iter()
            .filter_map(|primitive| match primitive {
                Primitive::Rect { rect, color } if *color == canvas::gpu::colorref(0xffff) => {
                    Some(*rect)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(highlights.len(), 2);
        for (rect, bounds) in highlights.iter().zip(&bounds) {
            assert_eq!(rect[2], bounds[2]);
            assert!(rect[0] >= bounds[0] && rect[1] >= bounds[1] && rect[3] <= bounds[3]);
        }
        let selection = primitives
            .iter()
            .find_map(|primitive| match primitive {
                Primitive::Rect { rect, color } if *color == macos::text_colors()[1] => Some(*rect),
                _ => None,
            })
            .unwrap();
        assert_eq!(selection[2], bounds[0][2]);
        assert!(editor.caret(1.0).unwrap().x0 + 24.0 > f64::from(bounds[0][2]));
    }

    #[test]
    fn editable_tables_paint_borders_before_selection_and_cell_text() {
        let mut engine = TextEngine::default();
        let mut editor = CanvasEditor::new(
            &mut engine,
            TextDocument::new(vec![Paragraph::new("Left".into(), Format::default())]).unwrap(),
            180.0,
        )
        .unwrap();
        editor
            .move_selection(&mut engine, Movement::LineEnd, false)
            .unwrap();
        editor.tab(&mut engine, false).unwrap();
        editor.insert(&mut engine, "Right").unwrap();
        editor.tab(&mut engine, true).unwrap();
        let mut primitives = Vec::new();
        append_outline(
            Some(&editor),
            editor.active_outline(),
            [24.0, 48.0],
            false,
            1.0,
            1.0,
            &mut primitives,
        )
        .unwrap();
        let color = canvas::gpu::colorref(0x00a3a3a3);
        assert!(matches!(primitives[0], Primitive::RoundedRect {
            stroke: Some(canvas::gpu::Stroke::Solid(0.75)), color: actual, ..
        } if actual == color));
        assert!(matches!(primitives[1], Primitive::Rect { color: actual, .. } if actual == color));
        assert!(
            matches!(primitives[2], Primitive::Rect { color, .. } if color == macos::text_colors()[1])
        );
        let painted = primitives
            .iter()
            .filter_map(|primitive| match primitive {
                Primitive::Text { layout, origin, .. } => Some((layout.id(), *origin)),
                _ => None,
            })
            .collect::<Vec<_>>();
        let expected = editor
            .active_outline()
            .layouts()
            .map(|(_, paragraph)| {
                (
                    paragraph.text.id(),
                    [paragraph.origin[0] + 24.0, paragraph.origin[1] + 48.0],
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(painted, expected);
        assert_eq!(painted.len(), 2);
    }

    #[test]
    fn resize_preview_paints_reflowed_text_without_changing_the_editor() {
        let mut engine = TextEngine::default();
        let editor = CanvasEditor::new(
            &mut engine,
            TextDocument::new(vec![Paragraph::new(
                "A paragraph with enough words to wrap when the resize handle moves inward.".into(),
                Format::default(),
            )])
            .unwrap(),
            240.0,
        )
        .unwrap();
        let original = editor.active_outline().snapshot();
        let original_layout = editor
            .active_outline()
            .paragraph_layout(0)
            .unwrap()
            .text
            .id();
        let resized = editor.preview_resize(&mut engine, 72.0).unwrap();
        let primitives = page_primitives(
            &editor,
            None,
            Some(PointerFeedback::Resize(&resized)),
            None,
            false,
            96.0 / 72.0,
            1.0,
        )
        .unwrap();
        let painted = primitives
            .iter()
            .find_map(|primitive| match primitive {
                Primitive::Text { layout, .. } => Some(*layout),
                _ => None,
            })
            .unwrap();
        assert_eq!(painted.id(), resized.paragraph_layout(0).unwrap().text.id());
        assert!(
            painted.lines().count()
                > editor
                    .active_outline()
                    .paragraph_layout(0)
                    .unwrap()
                    .text
                    .lines()
                    .count()
        );
        assert_eq!(editor.active_outline().layout(), &original.layout);
        assert_eq!(
            editor
                .active_outline()
                .paragraph_layout(0)
                .unwrap()
                .text
                .id(),
            original_layout
        );
        assert!(primitives.iter().any(
            |primitive| matches!(primitive, Primitive::RoundedRect { stroke, .. } if stroke.is_some())
        ));
    }

    #[test]
    fn native_chrome_geometry_preserves_text_and_matches_hit_regions_across_zoom_and_dpi() {
        let mut engine = TextEngine::default();
        let format = Format {
            font: Some("Arial".into()),
            font_size: Some(11.0),
            ..Default::default()
        };
        let mut editor = CanvasEditor::new(&mut engine, TextDocument::new(vec![
            Paragraph::new("HAMBURGEFONTS abcdefghijklmnopqrstuvwxyz 0123456789 HAMBURGEFONTS abcdefghijklmnopqrstuvwxyz 0123456789 HAMBURGEFONTS abcdefghijklmnopqrstuvwxyz 0123456789".into(), format.clone()),
            Paragraph::new("Second paragraph abcdefghijklmnopqrstuvwxyz.".into(), format),
        ]).unwrap(), 220.0).unwrap();
        let id = editor.active_outline().id;
        editor.move_outline(id, [36.0, 90.0]).unwrap();
        let document = editor.active_outline().document().clone();
        let layout_ids: Vec<_> = editor
            .active_outline()
            .layouts()
            .map(|(_, p)| p.text.id())
            .collect();
        for (zoom, expected) in [
            (0.5, [62.0, 134.0, 223.0, 219.0]),
            (1.0, [81.0, 189.0, 394.0, 348.0]),
            (1.5, [101.0, 245.0, 565.0, 479.0]),
            (2.0, [120.0, 300.0, 736.0, 608.0]),
        ] {
            for dpr in [1.0, 2.0] {
                let scale = zoom * (96.0 / 72.0) * dpr;
                let pixel = dpr / scale;
                let (frame, body_top) = outline_chrome(editor.active_outline(), pixel);
                for (index, coordinate) in frame.iter().enumerate() {
                    let origin = if index % 2 == 0 { 48.0 } else { 83.0 };
                    let screen = coordinate * scale / dpr + origin;
                    assert!(
                        (screen - expected[index]).abs() <= 1.5,
                        "zoom={zoom} dpr={dpr} edge={index}: {screen} vs {}",
                        expected[index]
                    );
                }
                let header = [36.0, (frame[1] + body_top) / 2.0];
                assert!(
                    matches!(page_hit_test(&editor, None, header, pixel), Some(Hit::Handle {id: hit, ..}) if hit == id)
                );
                assert!(matches!(
                    page_hit_test(&editor, None, [frame[2] - 4.0 * pixel, header[1]], pixel),
                    Some(Hit::Resize { id: hit, .. }) if hit == id
                ));
                let padding = [frame[0] + pixel, 90.0];
                assert!(
                    matches!(page_hit_test(&editor, None, padding, pixel), Some(Hit::Text {id: hit, ..}) if hit == id)
                );
                assert_eq!(
                    page_hit_test(&editor, None, [frame[0] - pixel, 90.0], pixel),
                    None
                );
                let primitives =
                    page_primitives(&editor, None, None, None, false, scale, dpr).unwrap();
                assert!(
                    matches!(&primitives[0], Primitive::RoundedRect {rect, ..} if *rect == [frame[0], frame[1], frame[2], body_top])
                );
                assert!(
                    primitives.iter().any(
                        |p| matches!(p, Primitive::Text {origin, ..} if *origin == [36.0, 90.0])
                    )
                );
            }
        }
        assert_eq!(editor.active_outline().document(), &document);
        assert_eq!(
            editor
                .active_outline()
                .layouts()
                .map(|(_, p)| p.text.id())
                .collect::<Vec<_>>(),
            layout_ids
        );
    }

    #[test]
    fn a_later_text_body_takes_priority_over_an_earlier_drag_handle() {
        let mut engine = TextEngine::default();
        let mut editor = CanvasEditor::new(
            &mut engine,
            TextDocument::new(vec![Paragraph::new("first".into(), Default::default())]).unwrap(),
            240.0,
        )
        .unwrap();
        let first = editor.active_outline().id;
        editor.move_outline(first, [0.0, 20.0]).unwrap();
        assert_eq!(
            page_hit_test(&editor, None, [5.0, 12.0], 1.0),
            Some(Hit::Handle {
                id: first,
                grab: [5.0, -8.0]
            })
        );
        let second = editor
            .create_outline(&mut engine, [0.0, 10.0], 240.0)
            .unwrap();
        assert_eq!(
            page_hit_test(&editor, None, [5.0, 12.0], 1.0),
            Some(Hit::Text {
                id: second,
                point: [5.0, 2.0]
            })
        );
        editor.undo(&mut engine).unwrap();
        assert_eq!(
            page_hit_test(&editor, None, [5.0, 12.0], 1.0),
            Some(Hit::Handle {
                id: first,
                grab: [5.0, -8.0]
            })
        );
    }

    #[test]
    fn overlapping_objects_follow_paint_order_through_creation_movement_and_undo() {
        use onestore::page::{Outline, PageObject, Unsupported};
        for readonly_on_top in [false, true] {
            let mut engine = TextEngine::default();
            let document = TextDocument::new(vec![Paragraph::new(
                "Imported text".into(),
                Default::default(),
            )])
            .unwrap();
            let id = onestore::ExGuid {
                guid: [5; 16],
                n: 1,
            };
            let mut objects = vec![
                PageObject::Outline(Outline {
                    id,
                    title: false,
                    min_width: None,
                    layout: onestore::document::Layout {
                        x: Some(30.0),
                        y: Some(40.0),
                        max_width: Some(180.0),
                        ..Default::default()
                    },
                    indents: vec![18.0, 0.0],
                    paragraphs: document.nodes().to_vec(),
                    unsupported: Vec::new(),
                }),
                PageObject::Unsupported(Unsupported {
                    id: Default::default(),
                    jcid: 0xdead,
                    layout: onestore::document::Layout {
                        x: Some(20.0),
                        y: Some(20.0),
                        max_width: Some(200.0),
                        max_height: Some(100.0),
                        ..Default::default()
                    },
                }),
            ];
            if !readonly_on_top {
                objects.reverse();
            }
            let page = Page {
                identity: None,
                created: None,
                title: String::new(),
                margin_origin: [0.0; 2],
                definitions: Default::default(),
                objects,
            };
            let (scene, mut editor) = PageScene::from_page(page, &mut engine).unwrap();
            let scene = (scene, [0.0; 2]);
            let imported_layout = editor
                .active_outline()
                .layouts()
                .next()
                .unwrap()
                .1
                .text
                .id();
            let texts: Vec<_> = page_primitives(&editor, Some(&scene), None, None, false, 1.0, 1.0)
                .unwrap()
                .into_iter()
                .filter_map(|p| match p {
                    Primitive::Text { layout, .. } => Some(layout.id()),
                    _ => None,
                })
                .collect();
            assert_eq!(texts.len(), 2);
            assert_eq!(texts[usize::from(!readonly_on_top)], imported_layout);
            let expected = if readonly_on_top {
                Hit::ReadOnly(0)
            } else {
                Hit::Text {
                    id,
                    point: [10.0, 5.0],
                }
            };
            assert_eq!(
                page_hit_test(&editor, Some(&scene), [40.0, 45.0], 1.0),
                Some(expected)
            );
            let header = page_hit_test(&editor, Some(&scene), [40.0, 32.0], 1.0);
            if readonly_on_top {
                assert_eq!(header, Some(Hit::ReadOnly(0)));
            } else {
                assert_eq!(
                    header,
                    Some(Hit::Handle {
                        id,
                        grab: [10.0, -8.0]
                    })
                );
            }
            let annotation = editor
                .create_outline(&mut engine, [30.0, 40.0], 180.0)
                .unwrap();
            assert_eq!(
                page_hit_test(&editor, Some(&scene), [40.0, 45.0], 1.0),
                Some(Hit::Text {
                    id: annotation,
                    point: [10.0, 5.0]
                })
            );
            assert_eq!(
                page_hit_test(&editor, Some(&scene), [40.0, 32.0], 1.0),
                Some(Hit::Handle {
                    id: annotation,
                    grab: [10.0, -8.0]
                })
            );
            let texts: Vec<_> = page_primitives(&editor, Some(&scene), None, None, false, 1.0, 1.0)
                .unwrap()
                .into_iter()
                .filter_map(|p| match p {
                    Primitive::Text { layout, .. } => Some(layout.id()),
                    _ => None,
                })
                .collect();
            assert_eq!(
                *texts.last().unwrap(),
                editor
                    .active_outline()
                    .layouts()
                    .next()
                    .unwrap()
                    .1
                    .text
                    .id()
            );
            editor.move_outline(annotation, [500.0, 500.0]).unwrap();
            assert!(
                !matches!(page_hit_test(&editor, Some(&scene), [40.0,45.0], 1.0), Some(Hit::Text { id, .. }) if id == annotation)
            );
            editor.undo(&mut engine).unwrap();
            assert_eq!(
                page_hit_test(&editor, Some(&scene), [40.0, 45.0], 1.0),
                Some(Hit::Text {
                    id: annotation,
                    point: [10.0, 5.0]
                })
            );
            editor.undo(&mut engine).unwrap();
            assert_eq!(editor.outlines().len(), 1);
            assert_eq!(editor.active_outline().document(), &document);
            assert_eq!(
                page_hit_test(&editor, Some(&scene), [10.0, 10.0], 1.0),
                None
            );
        }
    }

    #[test]
    fn read_only_focus_accepts_navigation_without_edit_shortcuts() {
        for modifiers in [
            ModifiersState::empty(),
            ModifiersState::SHIFT,
            ModifiersState::ALT,
            ModifiersState::SUPER,
            ModifiersState::SUPER | ModifiersState::SHIFT,
            ModifiersState::SUPER | ModifiersState::ALT,
        ] {
            for key in [
                Key::Character("a".into()),
                Key::Character("v".into()),
                Key::Character("c".into()),
                Key::Character("x".into()),
                Key::Character("z".into()),
                Key::Character("🌳".into()),
                Key::Named(NamedKey::Backspace),
                Key::Named(NamedKey::Delete),
                Key::Named(NamedKey::Enter),
                Key::Named(NamedKey::ArrowLeft),
                Key::Named(NamedKey::ArrowRight),
                Key::Named(NamedKey::ArrowUp),
                Key::Named(NamedKey::ArrowDown),
            ] {
                assert!(
                    !read_only_shortcut(&key, modifiers),
                    "{key:?} {modifiers:?}"
                );
            }
        }
        for (key, modifiers) in [
            (Key::Named(NamedKey::Escape), ModifiersState::empty()),
            (Key::Named(NamedKey::Tab), ModifiersState::CONTROL),
            (
                Key::Named(NamedKey::Tab),
                ModifiersState::CONTROL | ModifiersState::SHIFT,
            ),
            (
                Key::Character("N".into()),
                ModifiersState::SUPER | ModifiersState::SHIFT,
            ),
            (Key::Character("+".into()), ModifiersState::SUPER),
            (Key::Character("0".into()), ModifiersState::SUPER),
            (Key::Character("-".into()), ModifiersState::SUPER),
        ] {
            assert!(read_only_shortcut(&key, modifiers));
        }
        assert!(!read_only_shortcut(
            &Key::Character("n".into()),
            ModifiersState::SUPER
        ));
        assert!(!read_only_shortcut(
            &Key::Named(NamedKey::Tab),
            ModifiersState::empty()
        ));
    }

    #[test]
    fn read_only_focus_retires_text_overlays_and_draws_a_scaled_focus_border() {
        use onestore::page::{PageObject, Unsupported};
        let mut engine = TextEngine::default();
        let mut editor = CanvasEditor::new(
            &mut engine,
            TextDocument::new(vec![Paragraph::new("Draft".into(), Default::default())]).unwrap(),
            240.0,
        )
        .unwrap();
        let page = Page {
            identity: None,
            created: None,
            title: String::new(),
            margin_origin: [0.0; 2],
            definitions: Default::default(),
            objects: vec![PageObject::Unsupported(Unsupported {
                id: Default::default(),
                jcid: 0xdead,
                layout: onestore::document::Layout {
                    x: Some(100.0),
                    y: Some(80.0),
                    max_width: Some(180.0),
                    max_height: Some(80.0),
                    ..Default::default()
                },
            })],
        };
        let scene = (PageScene::new(page, &mut engine).unwrap(), [20.0, 30.0]);
        let original = editor.active_outline().document().clone();
        let rectangles = |primitives: Vec<Primitive<'_>>| {
            primitives
                .into_iter()
                .filter_map(|p| match p {
                    Primitive::Rect { rect, color }
                    | Primitive::RoundedRect { rect, color, .. } => Some((rect, color)),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        for scale in [1.0, 2.0] {
            let text = rectangles(
                page_primitives(&editor, Some(&scene), None, None, true, scale, 1.0).unwrap(),
            );
            assert!(
                text.iter()
                    .any(|(_, color)| *color == macos::text_colors()[0])
            );
            let selected = rectangles(
                page_primitives(
                    &editor,
                    Some(&scene),
                    None,
                    Some(ObjectFocus::ReadOnly(0)),
                    true,
                    scale,
                    1.0,
                )
                .unwrap(),
            );
            assert!(
                !selected
                    .iter()
                    .any(|(_, color)| *color == macos::text_colors()[0])
            );
            assert_eq!(
                selected,
                rectangles(
                    page_primitives(
                        &editor,
                        Some(&scene),
                        None,
                        Some(ObjectFocus::ReadOnly(0)),
                        false,
                        scale,
                        1.0
                    )
                    .unwrap()
                )
            );
            assert_eq!(
                selected.last().unwrap().0,
                [300.0 - 2.0 / scale, 110.0, 300.0, 190.0]
            );
        }
        editor.select_all().unwrap();
        let text = rectangles(
            page_primitives(&editor, Some(&scene), None, None, false, 1.0, 1.0).unwrap(),
        );
        assert!(
            text.iter()
                .any(|(_, color)| *color == macos::text_colors()[1])
        );
        let selected = rectangles(
            page_primitives(
                &editor,
                Some(&scene),
                None,
                Some(ObjectFocus::ReadOnly(0)),
                true,
                1.0,
                1.0,
            )
            .unwrap(),
        );
        assert!(
            !selected
                .iter()
                .any(|(_, color)| *color == macos::text_colors()[1])
        );
        assert_eq!(editor.active_outline().document(), &original);
    }

    #[test]
    #[ignore = "requires CANVAS_TEST_SECTION and CANVAS_TEST_PAGE private fixture inputs"]
    fn imported_page_widgets_and_accessibility_follow_edits() {
        let bytes = std::fs::read(std::env::var_os("CANVAS_TEST_SECTION").unwrap()).unwrap();
        let page = {
            let store = onestore::Store::parse(&bytes).unwrap();
            let index = onestore::RevisionIndex::parse(&store).unwrap();
            Page::from_document(
                &onestore::document::Document::parse(&index).unwrap(),
                &std::env::var("CANVAS_TEST_PAGE").unwrap(),
            )
            .unwrap()
        };
        drop(bytes);
        let mut engine = TextEngine::default();
        if let Some(font) = std::env::var_os("CANVAS_TEST_SUBSTITUTE") {
            engine
                .register_substitute(parley::fontique::Blob::new(Arc::new(
                    std::fs::read(font).unwrap(),
                )))
                .unwrap();
        }
        let (scene, mut editor) = PageScene::from_page(page, &mut engine).unwrap();
        let scene = (scene, [0.0; 2]);
        let viewport = Viewport {
            size: [1000, 720],
            scale: 96.0 / 72.0,
            origin: [48.0; 2],
        };
        let mut tag_hits = 0;
        for outline in editor.outlines() {
            for (_, paragraph) in outline.layouts() {
                for tag in &paragraph.tags {
                    let point = [
                        outline.origin()[0]
                            + tag.origin[0]
                            + canvas::outline::ParagraphTag::SIZE / 2.0,
                        outline.origin()[1]
                            + paragraph.origin[1]
                            + tag.origin[1]
                            + canvas::outline::ParagraphTag::SIZE / 2.0,
                    ];
                    assert!(
                        matches!(page_hit_test(&editor, Some(&scene), point, 1.0), Some(Hit::Text { id, .. }) if id == outline.id),
                        "Tag center {point:?} did not target outline {:?}",
                        outline.id
                    );
                    tag_hits += 1;
                }
            }
        }
        eprintln!("Verified {tag_hits} imported tag-gutter targets");
        let mut access = accessibility::Accessibility::default();
        let tags = |editor: &CanvasEditor| {
            page_primitives(editor, Some(&scene), None, None, true, viewport.scale, 1.0)
                .unwrap()
                .into_iter()
                .filter_map(|primitive| match primitive {
                    Primitive::Tag { tag, origin } => Some((tag.clone(), origin)),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        let original_tags = tags(&editor);
        let ids: Vec<_> = editor.outlines().iter().map(|outline| outline.id).collect();
        for id in ids {
            editor.focus_outline(id).unwrap();
            let original = editor.active_outline().document().clone();
            editor
                .select(
                    [canvas::document::TextPosition {
                        paragraph: 0,
                        offset: 0,
                    }; 2]
                        .into(),
                )
                .unwrap();
            let before = access.update(&editor, viewport, "Test", None).unwrap();
            let before = accesskit_consumer::Tree::new(before, true);
            let before = before.state().focus().unwrap().document_range().text();
            editor.insert(&mut engine, "CANVAS CHECK ").unwrap();
            {
                let update = access.update(&editor, viewport, "Test", None).unwrap();
                let tree = accesskit_consumer::Tree::new(update, true);
                assert_eq!(
                    tree.state().focus().unwrap().document_range().text(),
                    format!("CANVAS CHECK {before}")
                );
                let primitives =
                    page_primitives(&editor, Some(&scene), None, None, true, viewport.scale, 1.0)
                        .unwrap();
                assert!(!primitives.is_empty());
                let edited_tags = tags(&editor);
                assert_eq!(edited_tags.len(), original_tags.len());
                for ((edited, _), (original, _)) in edited_tags.iter().zip(&original_tags) {
                    assert_eq!(edited.icon, original.icon);
                    assert_eq!(edited.label, original.label);
                    assert_eq!(edited.disabled, original.disabled);
                }
            }
            editor.undo(&mut engine).unwrap();
            assert_eq!(editor.active_outline().document(), &original);
            assert_eq!(tags(&editor), original_tags);
            let update = access.update(&editor, viewport, "Test", None).unwrap();
            let tree = accesskit_consumer::Tree::new(update, true);
            assert_eq!(
                tree.state().focus().unwrap().document_range().text(),
                before
            );
        }
        eprintln!(
            "Rendered and restored {} imported outline widgets and accessibility fields",
            editor.outlines().len()
        );
    }
}
