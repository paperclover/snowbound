//! Snowbound's own message, question and entry dialogs, for desktops with neither zenity nor
//! kdialog, as GNOME's image-based systems ship: the app runs itself as the dialog, which
//! answers on stdout and exits successfully once accepted.

use draw::Renderer;
use std::{error::Error, ffi::OsString, process::Command, sync::Arc, time::Instant};
use ui::{Axis, Id, Spec, Ui, children, fill, fit, px};
use winit::{
    application::ApplicationHandler,
    dpi::LogicalSize,
    event::{ElementState, WindowEvent},
    event_loop::{ActiveEventLoop, EventLoop},
    keyboard::{Key, NamedKey},
    window::{Window, WindowId},
};

/// The argument that runs the app as a dialog.
pub const DIALOG: &str = "--dialog";

pub enum Ask<'a> {
    /// A message with an OK button.
    Message,
    /// Whether to go ahead: buttons for `cancel` and for `action`.
    Question { cancel: &'a str, action: &'a str },
    /// A line of text, starting as `value`.
    Entry { value: &'a str },
    /// A user name, starting as `user`, and a password, answered as zenity does: `user|password`.
    Login { user: &'a str },
}

/// Shows `title` and `detail` as `ask` asks, then waits: the answer, empty but for an entry,
/// or None when cancelled.
pub fn ask(title: &str, detail: &str, ask: Ask) -> Option<String> {
    let (kind, extra): (&str, &[&str]) = match &ask {
        Ask::Message => ("message", &[]),
        Ask::Question { cancel, action } => ("question", &[cancel, action]),
        Ask::Entry { value } => ("entry", &[value]),
        Ask::Login { user } => ("login", &[user]),
    };
    let output = crate::loader::executable()
        .and_then(|exe| {
            Command::new(exe)
                .args([DIALOG, kind, title, detail])
                .args(extra)
                .stderr(std::process::Stdio::inherit())
                .output()
        })
        .inspect_err(|error| eprintln!("Cannot show {title:?}: {error}"))
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The dialog `ask` would show, given the arguments after `DIALOG`.
pub fn run(mut args: impl Iterator<Item = OsString>) -> Result<(), Box<dyn Error>> {
    let mut next = || {
        args.next()
            .and_then(|arg| arg.into_string().ok())
            .unwrap_or_default()
    };
    let [kind, title, detail, first, second] = [(); 5].map(|()| next());
    let mut dialog = Dialog {
        kind,
        title,
        detail,
        buttons: [first, second],
        fields: Default::default(),
        shown: None,
        accepted: false,
    };
    if dialog.kind == "entry" || dialog.kind == "login" {
        dialog.fields[0] = std::mem::take(&mut dialog.buttons[0]);
    }
    EventLoop::new()?.run_app(&mut dialog)?;
    if !dialog.accepted {
        std::process::exit(1);
    }
    let [first, second] = &dialog.fields;
    match dialog.kind.as_str() {
        "entry" => print!("{first}"),
        "login" => print!("{first}|{second}"),
        _ => {}
    }
    Ok(())
}

struct Dialog {
    kind: String,
    title: String,
    detail: String,
    /// A question's cancel and action labels.
    buttons: [String; 2],
    /// An entry's text, or a login's user name and password.
    fields: [String; 2],
    shown: Option<Shown>,
    accepted: bool,
}

struct Shown {
    window: Arc<Window>,
    surface: crate::surface::Surface,
    renderer: Renderer,
    ui: Ui,
}

fn field(index: usize) -> Id {
    Id::ROOT.child(("field", index))
}

impl Dialog {
    fn finish(&mut self, event_loop: &ActiveEventLoop, accepted: bool) {
        self.accepted = accepted;
        // The window's resources end while the event loop still holds its display.
        self.shown = None;
        event_loop.exit();
    }

    /// Lays out and paints the dialog; Some once a button answers it.
    fn frame(&mut self) -> Result<Option<bool>, Box<dyn Error>> {
        let Some(shown) = &mut self.shown else {
            return Ok(None);
        };
        let ui = &mut shown.ui;
        let scale = shown.window.scale_factor() as f32;
        let size = shown.window.inner_size();
        ui.begin(
            [size.width as f32 / scale, size.height as f32 / scale],
            scale,
            Instant::now(),
        );
        let theme = ui.theme.clone();
        ui.open(
            "dialog",
            Spec {
                axis: Axis::Y,
                size: [fill(), fill()],
                fill: Some(theme.popup),
                pad: [20.0, 16.0],
                gap: 10.0,
                role: Some(accesskit::Role::Dialog),
                ..Spec::default()
            },
        );
        let text = |ui: &mut Ui, part, text: &str, bold| {
            ui.leaf(
                part,
                Spec {
                    size: [fill(), fit()],
                    text: Some(text),
                    bold,
                    overflow: ui::Overflow::Wrap,
                    ..Spec::default()
                },
            );
        };
        text(ui, "title", &self.title, true);
        if !self.detail.is_empty() {
            text(ui, "detail", &self.detail, false);
        }
        let entry = Spec {
            size: [fill(), px(theme.font_size * 2.0)],
            fill: Some(theme.base),
            border: Some(theme.accent),
            radius: 4.0,
            pad: [6.0, 0.0],
            ..Spec::default()
        };
        let fields = match self.kind.as_str() {
            "entry" => 1,
            "login" => 2,
            _ => 0,
        };
        if fields > 0 && ui.focused().is_none() {
            ui.set_focus(Some(field(0)));
        }
        for (index, value) in self.fields.iter_mut().take(fields).enumerate() {
            if index == 1 {
                text(ui, "password label", "Password:", false);
                ui::password_field(ui, field(index), value, "", entry.clone());
            } else {
                ui::text_field(ui, field(index), value, "", entry.clone());
            }
        }
        ui.leaf(
            "space",
            Spec {
                size: [fill(), fill()],
                ..Spec::default()
            },
        );
        ui.open(
            "buttons",
            Spec {
                size: [fill(), children()],
                gap: 8.0,
                ..Spec::default()
            },
        );
        ui.leaf(
            "space",
            Spec {
                size: [fill(), px(0.0)],
                ..Spec::default()
            },
        );
        let [cancel, action] = match self.kind.as_str() {
            "question" => [self.buttons[0].as_str(), self.buttons[1].as_str()],
            "message" => ["", "OK"],
            _ => ["Cancel", "OK"],
        };
        let mut answer = None;
        if !cancel.is_empty() && ui::button(ui, "cancel", cancel).clicked {
            answer = Some(false);
        }
        if ui::button(ui, "action", action).clicked {
            answer = Some(true);
        }
        ui.close();
        ui.close();
        ui.end();
        shown.window.set_cursor(ui.cursor().unwrap_or_default());
        if let Some(frame) = shown.surface.frame(&shown.renderer)? {
            let interface = ui.layers();
            let layers: Vec<_> = interface
                .iter()
                .filter_map(|layer| match layer {
                    ui::Layer::Primitives(primitives) => Some(primitives.layer(scale)),
                    ui::Layer::Custom { .. } => None,
                })
                .collect();
            shown
                .renderer
                .draw(&frame.target, shown.surface.size, theme.popup, &layers)
                .map_err(|error| format!("Dialog drawing failed: {error:?}"))?;
            shown.surface.present(&shown.renderer, frame);
        }
        if ui.wants_frame() {
            shown.window.request_redraw();
        }
        Ok(answer)
    }
}

impl ApplicationHandler for Dialog {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.shown.is_some() {
            return;
        }
        let height = match self.kind.as_str() {
            "login" => 260.0,
            "entry" => 200.0,
            _ => 170.0,
        };
        let shown = (|| -> Result<Shown, Box<dyn Error>> {
            let window = Arc::new(
                event_loop.create_window(
                    crate::platform::window_attributes()
                        .with_title(&self.title)
                        .with_inner_size(LogicalSize::new(440.0, height))
                        .with_resizable(false),
                )?,
            );
            let (surface, renderer) =
                pollster::block_on(crate::surface::Surface::new(window.clone(), false))?;
            let appearance = crate::platform::appearance(&window);
            window.set_theme(Some(appearance));
            let ui = Ui::new(
                crate::theme(appearance, false, false),
                crate::platform::double_click_interval(),
            );
            Ok(Shown {
                window,
                surface,
                renderer,
                ui,
            })
        })();
        match shown {
            Ok(shown) => {
                shown.window.request_redraw();
                self.shown = Some(shown);
            }
            Err(error) => {
                eprintln!("Cannot show {:?}: {error}", self.title);
                self.finish(event_loop, false);
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        let Some(shown) = &mut self.shown else {
            return;
        };
        let scale = shown.window.scale_factor() as f32;
        let input = match event {
            WindowEvent::CloseRequested => return self.finish(event_loop, false),
            WindowEvent::RedrawRequested => {
                match self.frame() {
                    Ok(Some(answer)) => self.finish(event_loop, answer),
                    Ok(None) => {}
                    Err(error) => {
                        eprintln!("{error}");
                        self.finish(event_loop, false);
                    }
                }
                return;
            }
            WindowEvent::Resized(size) => {
                shown.surface.size = [size.width, size.height];
                shown.surface.configure(&shown.renderer);
                return shown.window.request_redraw();
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                match event.logical_key {
                    Key::Named(NamedKey::Enter) => return self.finish(event_loop, true),
                    Key::Named(NamedKey::Escape) => return self.finish(event_loop, false),
                    key => ui::Event::Key {
                        key,
                        text: event.text.map(|text| text.to_string()),
                    },
                }
            }
            WindowEvent::ModifiersChanged(modifiers) => ui::Event::Modifiers(modifiers.state()),
            WindowEvent::Ime(ime) => ui::Event::Ime(ime),
            WindowEvent::CursorMoved { position, .. } => {
                ui::Event::PointerMoved([position.x as f32 / scale, position.y as f32 / scale])
            }
            WindowEvent::CursorLeft { .. } => ui::Event::PointerLeft,
            WindowEvent::MouseInput { state, button, .. } => {
                // A press away from the controls moves the window, which has no title bar
                // under winit's Adwaita frame.
                if state == ElementState::Pressed
                    && shown.ui.cursor().is_none()
                    && let Err(error) = shown.window.drag_window()
                {
                    eprintln!("{error}");
                }
                ui::Event::Button {
                    button,
                    pressed: state == ElementState::Pressed,
                    at: Instant::now(),
                }
            }
            _ => return,
        };
        shown.ui.event(input);
        shown.window.request_redraw();
    }
}
