mod accessibility;
mod macos;
#[cfg(test)]
mod profile;

use one_canvas::{
    document::TextDocument,
    editor::{CanvasEditor, Movement, TextOutline},
    layout::TextEngine,
    page::Page,
    text::Paragraph,
};
use one_canvas_gpu::{Primitive, Renderer, Viewport, page::PageScene};
use onestore::document::Format;
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
const NEW_OUTLINE_WIDTH: f32 = 240.0;

#[derive(Debug)]
enum UserEvent {
    Quit,
    Accessibility(accesskit_winit::Event),
}

impl From<accesskit_winit::Event> for UserEvent {
    fn from(event: accesskit_winit::Event) -> Self {
        Self::Accessibility(event)
    }
}

fn trace_input(event: &impl std::fmt::Debug) {
    if std::env::var_os("ONE_CANVAS_TRACE_INPUT").is_some() {
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
}

enum Drag {
    Text,
    Outline {
        id: onestore::ExGuid,
        grab: [f32; 2],
        pending_press: Option<[f32; 2]>,
    },
}

struct State {
    window: Arc<Window>,
    instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    renderer: Renderer,
    engine: TextEngine,
    editor: CanvasEditor,
    initial: Vec<(onestore::ExGuid, onestore::document::Layout, TextDocument)>,
    scene: Option<(PageScene, [f32; 2])>,
    viewport: Viewport,
    display_scale: f32,
    pointer: [f32; 2],
    drag: Option<Drag>,
    read_only_focus: Option<usize>,
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
                    })
                    .with_inner_size(LogicalSize::new(1000.0, 720.0)),
            )?,
        );
        let access_adapter =
            accesskit_winit::Adapter::with_event_loop_proxy(event_loop, &window, proxy);
        window.set_visible(true);
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle(
            Box::new(window.clone()),
        ));
        let surface = instance.create_surface(window.clone())?;
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
        };
        let initial = editor
            .outlines()
            .iter()
            .map(|outline| {
                (
                    outline.id,
                    outline.layout().clone(),
                    outline.document().clone(),
                )
            })
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
            initial,
            scene,
            viewport: Viewport {
                size: [size.width, size.height],
                scale: dpr * 96.0 / 72.0,
                origin: [48.0 * dpr; 2],
            },
            display_scale: dpr,
            pointer: [0.0; 2],
            drag: None,
            read_only_focus: None,
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
                snap_to_grid(position)
            },
        ))
    }

    fn set_read_only_focus(&mut self, index: Option<usize>) {
        if self.read_only_focus != index {
            self.editor.finish_composition();
            self.drag = None;
            self.read_only_focus = index;
            self.window.set_ime_allowed(index.is_none());
        }
    }

    fn changed(&mut self) -> Result<(), Box<dyn Error>> {
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
        self.window.request_redraw();
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
                    self.accessibility.append_read_only(
                        &mut update,
                        self.scene.as_ref(),
                        self.viewport,
                        self.read_only_focus,
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
        if let Some(index) = self.accessibility.read_only_for_node(request.target_node) {
            if request.action == Action::Focus {
                self.set_read_only_focus(Some(index));
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
        self.set_read_only_focus(None);
        self.reveal_focus()?;
        self.changed()
    }

    fn reveal_focus(&mut self) -> Result<(), Box<dyn Error>> {
        if self.viewport.size.contains(&0) {
            return Ok(());
        }
        let rect = if let Some(index) = self.read_only_focus {
            let (scene, offset) = self.scene.as_ref().unwrap();
            let [x0, y0, x1, y1] = scene.read_only().nth(index).unwrap().rect;
            parley::BoundingBox {
                x0: f64::from(x0 + offset[0]),
                y0: f64::from(y0 + offset[1]),
                x1: f64::from(x1 + offset[0]),
                y1: f64::from(y1 + offset[1]),
            }
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
            if self.read_only_focus.is_none()
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
        let primitives = page_primitives(
            &self.editor,
            self.scene.as_ref(),
            self.preview(),
            self.read_only_focus,
            self.caret && self.focused && !matches!(self.drag, Some(Drag::Outline { .. })),
            self.viewport.scale,
            self.display_scale,
        )?;
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
        if self.read_only_focus.is_some() {
            if !read_only_shortcut(key, self.modifiers) {
                return Ok(());
            }
            if key == &Key::Named(NamedKey::Escape) {
                self.set_read_only_focus(None);
                self.reveal_focus()?;
                return self.changed();
            }
        }
        if matches!(self.drag, Some(Drag::Outline { .. })) {
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
                    .map_or(0, |(scene, _)| scene.read_only().count());
            if count == 0 {
                return self.changed();
            }
            let index = self.read_only_focus.map_or_else(
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
                self.set_read_only_focus(None);
            } else {
                self.set_read_only_focus(Some(next - outlines.len()));
            }
            self.reveal_focus()?;
            return self.changed();
        }
        if command && let Key::Character(key) = key {
            match key.to_lowercase().as_str() {
                "n" if shift => {
                    let position = if let Some(index) = self.read_only_focus {
                        let (scene, offset) = self.scene.as_ref().unwrap();
                        let rect = scene.read_only().nth(index).unwrap().rect;
                        [rect[2] + offset[0] + 24.0, rect[1] + offset[1]]
                    } else {
                        let bounds = self.editor.active_outline().bounds();
                        [bounds.x1 as f32 + 24.0, bounds.y0 as f32]
                    };
                    self.editor.place_caret(
                        &mut self.engine,
                        snap_to_grid(position),
                        NEW_OUTLINE_WIDTH,
                    )?;
                    self.set_read_only_focus(None);
                }
                "a" => self.editor.select_all()?,
                "z" => {
                    if shift {
                        self.editor.redo(&mut self.engine)?;
                    } else {
                        self.editor.undo(&mut self.engine)?;
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
                Key::Named(NamedKey::ArrowUp) => Some(Movement::Up),
                Key::Named(NamedKey::ArrowDown) => Some(Movement::Down),
                Key::Named(NamedKey::Home) => Some(Movement::LineStart),
                Key::Named(NamedKey::End) => Some(Movement::LineEnd),
                _ => None,
            };
            if let Some(movement) = movement {
                self.editor.move_selection(movement, shift)?;
            } else if self.editor.marked_range().is_none() {
                match key {
                    Key::Named(NamedKey::Backspace) => {
                        self.editor.delete(&mut self.engine, true)?;
                    }
                    Key::Named(NamedKey::Delete) => {
                        self.editor.delete(&mut self.engine, false)?;
                    }
                    Key::Named(NamedKey::Enter) => self.editor.insert(&mut self.engine, "\n")?,
                    Key::Named(NamedKey::Tab) => self.editor.insert(&mut self.engine, "\t")?,
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
            state
                .editor
                .caret_outline()
                .is_none_or(TextOutline::is_empty)
                && state
                    .initial
                    .iter()
                    .map(|(id, layout, document)| (id, layout, document))
                    .eq(state
                        .editor
                        .outlines()
                        .iter()
                        .map(|outline| (&outline.id, outline.layout(), outline.document())))
        }) || macos::discard_changes()
        {
            event_loop.exit();
        }
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        let event = match event {
            UserEvent::Quit => {
                self.close(event_loop);
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
                WindowEvent::CursorMoved { position, .. } => {
                    state.pointer = [position.x as f32, position.y as f32];
                    match &mut state.drag {
                        Some(Drag::Text) => {
                            let point = state.viewport.document_point(state.pointer);
                            let origin = state.editor.active_outline().origin();
                            state.editor.select_at(
                                point[0] - origin[0],
                                point[1] - origin[1],
                                true,
                            )?;
                            state.changed()?;
                        }
                        Some(Drag::Outline { pending_press, .. }) => {
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
                        None => {}
                    }
                }
                WindowEvent::MouseInput {
                    state: button_state,
                    button: MouseButton::Left,
                    ..
                } => {
                    let point = state.viewport.document_point(state.pointer);
                    if button_state == ElementState::Pressed {
                        match page_hit_test(
                            &state.editor,
                            state.scene.as_ref(),
                            point,
                            state.display_scale / state.viewport.scale,
                        ) {
                            Some(Hit::ReadOnly(index)) => state.set_read_only_focus(Some(index)),
                            Some(Hit::Handle { id, grab }) => {
                                state.set_read_only_focus(None);
                                state.editor.focus_outline(id)?;
                                state.drag = Some(Drag::Outline {
                                    id,
                                    grab,
                                    pending_press: Some(state.pointer),
                                });
                            }
                            Some(Hit::Text { id, point }) => {
                                let extend = state.modifiers.shift_key()
                                    && state.read_only_focus.is_none()
                                    && id == state.editor.active_outline().id;
                                state.set_read_only_focus(None);
                                state.editor.focus_outline(id)?;
                                state.editor.select_at(point[0], point[1], extend)?;
                                state.drag = Some(Drag::Text);
                            }
                            None => {
                                let position = [
                                    point[0],
                                    point[1] - 7.0 * state.display_scale / state.viewport.scale,
                                ];
                                let position = if state.modifiers.alt_key() {
                                    position
                                } else {
                                    snap_to_grid(position)
                                };
                                state.editor.place_caret(
                                    &mut state.engine,
                                    position,
                                    NEW_OUTLINE_WIDTH,
                                )?;
                                state.set_read_only_focus(None);
                                state.drag = Some(Drag::Text);
                            }
                        }
                    } else {
                        let preview = state.preview();
                        state.drag = None;
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
                WindowEvent::Ime(_) if state.read_only_focus.is_some() => {}
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
        if state.focused && !state.occluded && state.read_only_focus.is_none() && anchor == focus {
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
    Text {
        id: onestore::ExGuid,
        point: [f32; 2],
    },
    Handle {
        id: onestore::ExGuid,
        grab: [f32; 2],
    },
    ReadOnly(usize),
}

fn page_hit_test(
    editor: &CanvasEditor,
    scene: Option<&(PageScene, [f32; 2])>,
    point: [f32; 2],
    pixel: f32,
) -> Option<Hit> {
    let hit = |outline: &TextOutline, offset: [f32; 2]| {
        let (bounds, body_top) = outline_chrome(outline, pixel);
        let local = [point[0] - offset[0], point[1] - offset[1]];
        let [x, y] = local;
        if x >= bounds[0] && x <= bounds[2] && y >= bounds[1] && y < body_top {
            return Some(Hit::Handle {
                id: outline.id,
                grab: [
                    point[0] - outline.origin()[0],
                    point[1] - outline.origin()[1],
                ],
            });
        }
        if (x >= bounds[0] && x <= bounds[2] && y >= body_top && y <= bounds[3])
            || outline.layouts().any(|(_, paragraph)| {
                paragraph.tags.iter().any(|tag| {
                    let origin = outline.origin();
                    let x = origin[0] + tag.origin[0];
                    let y = origin[1] + paragraph.origin[1] + tag.origin[1];
                    let size = one_canvas::outline::ParagraphTag::SIZE;
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
    if let Some(hit) = editor
        .outlines()
        .iter()
        .rev()
        .filter(|outline| scene.is_none_or(|(scene, _)| !scene.contains_outline(outline.id)))
        .find_map(|outline| hit(outline, [0.0; 2]))
    {
        return Some(hit);
    }
    let (scene, offset) = scene?;
    match scene.hit_test([point[0] - offset[0], point[1] - offset[1]], |id| {
        editor
            .outlines()
            .iter()
            .find(|outline| outline.id == id)
            .and_then(|outline| hit(outline, *offset))
    })? {
        one_canvas_gpu::page::SceneHit::Outline(hit) => Some(hit),
        one_canvas_gpu::page::SceneHit::ReadOnly(index) => Some(Hit::ReadOnly(index)),
    }
}

fn page_primitives<'a>(
    editor: &'a CanvasEditor,
    scene: Option<&'a (PageScene, [f32; 2])>,
    preview: Option<(onestore::ExGuid, [f32; 2])>,
    read_only_focus: Option<usize>,
    show_caret: bool,
    scale: f32,
    display_scale: f32,
) -> Result<Vec<Primitive<'a>>, Box<dyn Error>> {
    let mut primitives = Vec::new();
    let paint = |id, offset: [f32; 2], primitives: &mut Vec<_>| {
        let outline = editor
            .outlines()
            .iter()
            .find(|outline| outline.id == id)
            .ok_or(one_canvas_gpu::page::SceneError::MissingOutline)?;
        let origin = preview
            .filter(|(id, _)| *id == outline.id)
            .map(|(_, origin)| origin)
            .unwrap_or_else(|| outline.origin());
        append_outline(
            (read_only_focus.is_none() && outline.id == editor.active_outline().id)
                .then_some(editor),
            outline,
            [origin[0] + offset[0], origin[1] + offset[1]],
            show_caret,
            scale,
            display_scale / scale,
            primitives,
        )
    };
    if let Some((scene, origin)) = scene {
        scene.append_primitives_with(&mut primitives, *origin, &paint)?;
    }
    for outline in editor
        .outlines()
        .iter()
        .filter(|outline| scene.is_none_or(|(scene, _)| !scene.contains_outline(outline.id)))
    {
        paint(outline.id, [0.0; 2], &mut primitives)?;
    }
    if read_only_focus.is_none()
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
    if let Some(index) = read_only_focus {
        let (scene, offset) = scene.unwrap();
        let [x0, y0, x1, y1] = scene.read_only().nth(index).unwrap().rect;
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

fn snap_to_grid(point: [f32; 2]) -> [f32; 2] {
    std::array::from_fn(|axis| {
        let offset = [0.0, 14.4][axis];
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
    let (bounds, body_top) = outline_chrome(outline, pixel);
    let [dx, dy] = [x - outline.origin()[0], y - outline.origin()[1]];
    let [left, top, right, bottom] = [
        bounds[0] + dx,
        bounds[1] + dy,
        bounds[2] + dx,
        bounds[3] + dy,
    ];
    let body_top = body_top + dy;
    let border = if editor.is_some() {
        [0.25, 0.45, 0.7, 0.65]
    } else {
        [0.5, 0.5, 0.5, 0.2]
    };
    if editor.is_none_or(|editor| editor.caret_outline().is_none()) {
        for rect in [
            [left, top, right, body_top],
            [left, body_top, left + pixel, bottom],
            [right - pixel, body_top, right, bottom],
            [left + pixel, bottom - pixel, right - pixel, bottom],
        ] {
            primitives.push(Primitive::Rect {
                rect,
                color: border,
            });
        }
    }
    for (_, paragraph) in outline.layouts() {
        let layout = &paragraph.text;
        let paragraph_origin = paragraph.origin;
        for (rect, color) in layout.backgrounds() {
            primitives.push(Primitive::Rect {
                rect: [
                    rect.x0 as f32 + x + paragraph_origin[0],
                    rect.y0 as f32 + y + paragraph_origin[1],
                    rect.x1 as f32 + x + paragraph_origin[0],
                    rect.y1 as f32 + y + paragraph_origin[1],
                ],
                color: one_canvas_gpu::colorref(color),
            });
        }
    }
    if let Some(editor) = editor {
        for rect in editor.selection_rects()? {
            primitives.push(Primitive::Rect {
                rect: [
                    rect.x0 as f32 + x,
                    rect.y0 as f32 + y,
                    rect.x1 as f32 + x,
                    rect.y1 as f32 + y,
                ],
                color: [0.55, 0.73, 1.0, 0.5],
            });
        }
    }
    for (_, paragraph) in outline.layouts() {
        let layout = &paragraph.text;
        let paragraph_origin = paragraph.origin;
        primitives.push(Primitive::Text {
            layout,
            origin: [x + paragraph_origin[0], y + paragraph_origin[1]],
        });
        for (layout, origin) in &paragraph.markers {
            primitives.push(Primitive::Text {
                layout,
                origin: [x + origin[0], y + (origin[1] + paragraph_origin[1])],
            });
        }
        for tag in &paragraph.tags {
            primitives.push(Primitive::Tag {
                tag,
                origin: [x, y + paragraph_origin[1]],
            });
        }
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
            let rect = editor.caret(1.0 / scale)?;
            primitives.push(Primitive::Rect {
                rect: [
                    rect.x0 as f32 + x,
                    rect.y0 as f32 + y,
                    rect.x1 as f32 + x,
                    rect.y1 as f32 + y,
                ],
                color: [0.0, 0.0, 0.0, 1.0],
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
    while let Some(arg) = args.next() {
        if arg == "--substitute-font" {
            substitutes.push(PathBuf::from(
                args.next()
                    .ok_or("Provide a font file after --substitute-font.")?,
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
    if editable && !positional.is_empty() {
        return Err(
            "Use --page with a section file and page title, without a text file or width.".into(),
        );
    }
    if positional.len() > 2 {
        return Err(
            "Usage: one-canvas-app [TEXT_FILE] [WIDTH_POINTS] [--reference SECTION PAGE_TITLE | --page SECTION PAGE_TITLE] [--substitute-font FONT_FILE]..."
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
            NEW_OUTLINE_WIDTH
        } else {
            480.0
        });
    let input = if editable {
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
            let result = snap_to_grid(proposed);
            for axis in 0..2 {
                assert!((result[axis] - origin[axis] - points).abs() < 0.00004);
            }
        }
        for pixels in [6.0, 7.0, 12.0, 13.0, 19.0] {
            let result = snap_to_grid([491.25 + pixels * 0.75, 235.65 + pixels * 0.75]);
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
                let result = snap_to_grid(proposed);
                for axis in 0..2 {
                    assert!((result[axis] - expected[axis]).abs() < 0.00004);
                }
            }
        }
        for point in [[9.0, 5.4], [-9.0, -12.6], [423.0, 239.4]] {
            assert_eq!(snap_to_grid(point), point);
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
                let result = snap_to_grid([point[0], point[1] - 7.0 * dpr / viewport.scale]);
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
                    Primitive::Rect { rect, color } => Some((rect, color)),
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
                .all(|(_, color)| *color == [0.0, 0.0, 0.0, 1.0])
        );
        let preedit = access.update(&editor, viewport, "Page", None).unwrap();
        assert_eq!(preedit.focus, initial.focus);
        editor.commit_text(&mut engine, "日本".into()).unwrap();
        let committed = access.update(&editor, viewport, "Page", None).unwrap();
        assert_eq!(committed.focus, initial.focus);
        assert_eq!(rectangles(&editor).len(), 5);
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
        assert_eq!(rectangles(&editor).len(), 5);
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
                    matches!(&primitives[0], Primitive::Rect {rect, ..} if *rect == [frame[0], frame[1], frame[2], body_top])
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
        use one_canvas::page::{Outline, PageObject, Unsupported};
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
        use one_canvas::page::{PageObject, Unsupported};
        let mut engine = TextEngine::default();
        let mut editor = CanvasEditor::new(
            &mut engine,
            TextDocument::new(vec![Paragraph::new("Draft".into(), Default::default())]).unwrap(),
            240.0,
        )
        .unwrap();
        let page = Page {
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
                    Primitive::Rect { rect, color } => Some((rect, color)),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        for scale in [1.0, 2.0] {
            let text = rectangles(
                page_primitives(&editor, Some(&scene), None, None, true, scale, 1.0).unwrap(),
            );
            assert!(text.iter().any(|(_, color)| *color == [0.0, 0.0, 0.0, 1.0]));
            let selected = rectangles(
                page_primitives(&editor, Some(&scene), None, Some(0), true, scale, 1.0).unwrap(),
            );
            assert!(
                !selected
                    .iter()
                    .any(|(_, color)| *color == [0.0, 0.0, 0.0, 1.0])
            );
            assert_eq!(
                selected,
                rectangles(
                    page_primitives(&editor, Some(&scene), None, Some(0), false, scale, 1.0)
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
                .any(|(_, color)| *color == [0.55, 0.73, 1.0, 0.5])
        );
        let selected = rectangles(
            page_primitives(&editor, Some(&scene), None, Some(0), true, 1.0, 1.0).unwrap(),
        );
        assert!(
            !selected
                .iter()
                .any(|(_, color)| *color == [0.55, 0.73, 1.0, 0.5])
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
                            + one_canvas::outline::ParagraphTag::SIZE / 2.0,
                        outline.origin()[1]
                            + paragraph.origin[1]
                            + tag.origin[1]
                            + one_canvas::outline::ParagraphTag::SIZE / 2.0,
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
                    [one_canvas::document::TextPosition {
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
