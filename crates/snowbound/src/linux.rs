//! Linux, on X11 and Wayland: window controls drawn in the title bar, dialogs through zenity
//! or kdialog, the colour scheme from the XDG settings portal, and text conventions from
//! the C library's locale.

use canvas::date::DateField;
use std::{
    ffi::CStr,
    path::PathBuf,
    process::Command,
    sync::{
        OnceLock,
        atomic::{AtomicU32, Ordering},
    },
    time::Duration,
};
use ui::{Flags, Spec, Ui, px};
use winit::{
    error::EventLoopError,
    event_loop::{EventLoop, EventLoopProxy},
    platform::wayland::{EventLoopExtWayland, WindowAttributesExtWayland},
    window::{ResizeDirection, Theme, Window, WindowAttributes},
};

/// The title bar's leading margin; the window controls sit at its trailing end.
pub const LEADING: f32 = 8.0;
/// The margin past the window controls.
pub const TRAILING: f32 = 8.0;
/// How far inside the window's edges a press resizes it, and how far along them a corner
/// reaches, in logical pixels.
const EDGE: f32 = 5.0;
const CORNER: f32 = 16.0;
const MINIMIZE: &[&str] = &[include_str!("../assets/icons/window-minimize.svg")];
const MAXIMIZE: &[&str] = &[include_str!("../assets/icons/window-maximize.svg")];
const RESTORE: &[&str] = &[include_str!("../assets/icons/window-restore.svg")];

static QUIT: OnceLock<EventLoopProxy<crate::UserEvent>> = OnceLock::new();
/// Breeze's corner radius in units of the decoration's pixel grid, as `f32` bits; zero where
/// KWin leaves windows square.
static BREEZE_RADIUS: AtomicU32 = AtomicU32::new(0);

pub fn event_loop(headless: bool) -> Result<EventLoop<crate::UserEvent>, EventLoopError> {
    // Month and day names and date orders follow the user's locale; other categories stay C.
    unsafe { libc::setlocale(libc::LC_TIME, c"".as_ptr()) };
    let event_loop = EventLoop::with_user_event().build()?;
    if !headless {
        crate::desktop::prepare(&event_loop);
    }
    QUIT.set(event_loop.create_proxy())
        .expect("Only one application event loop is created");
    watch_settings(event_loop.create_proxy());
    if desktop() == Desktop::Kde {
        follow_breeze_radius(event_loop.create_proxy());
    }
    let libadwaita = desktop() == Desktop::Gnome && event_loop.is_wayland();
    sctk_adwaita::LIBADWAITA_EDGE.store(libadwaita, Ordering::Relaxed);
    Ok(event_loop)
}

/// How far the window's corners round unless it is maximized or full screen: KWin rounds
/// the bottom corners of Breeze's borderless windows, and on GNOME's Wayland the window cuts
/// its own to libadwaita's radius, which winit's Adwaita frame edges.
pub fn corner_radius(window: &Window) -> f32 {
    if !window.is_decorated() || window.is_maximized() || window.fullscreen().is_some() {
        return 0.0;
    }
    if cuts_corners() {
        return sctk_adwaita::WINDOW_RADIUS;
    }
    // Breeze snaps its radius to the device pixel grid.
    let units = f32::from_bits(BREEZE_RADIUS.load(Ordering::Relaxed));
    let scale = window.scale_factor() as f32;
    (units * scale).round() / scale
}

/// Whether the window erases its own bottom corners to transparent pixels: under winit's
/// Adwaita frame on GNOME's Wayland, which draws libadwaita's window edge. Mutter's X11
/// frames stay square at the bottom, as libadwaita's own `ssd-frame` style has them.
pub fn cuts_corners() -> bool {
    sctk_adwaita::LIBADWAITA_EDGE.load(Ordering::Relaxed)
}

/// Reads Breeze's corner radius now and again whenever KWin is told to reload its
/// configuration, as Breeze's and the border size's settings pages do, redrawing then.
fn follow_breeze_radius(proxy: EventLoopProxy<crate::UserEvent>) {
    let Ok(connection) = zbus::blocking::connection::Builder::session()
        .and_then(|builder| builder.method_timeout(Duration::from_millis(500)).build())
    else {
        return;
    };
    let read = |connection: &zbus::blocking::Connection| {
        let units = breeze_radius(connection).unwrap_or(0.0);
        BREEZE_RADIUS.store(units.to_bits(), Ordering::Relaxed);
    };
    // Before the window's first frame, so its corners never change under the user.
    read(&connection);
    std::thread::spawn(move || {
        let Ok(rule) = zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .interface("org.kde.KWin")
            .and_then(|rule| rule.member("reloadConfig"))
            .map(|rule| rule.build())
        else {
            return;
        };
        let Ok(signals) =
            zbus::blocking::MessageIterator::for_match_rule(rule, &connection, Some(8))
        else {
            return;
        };
        for _ in signals {
            read(&connection);
            let _ = proxy.send_event(crate::UserEvent::Redraw);
        }
    });
}

/// Breeze's `Frame_FrameRadius` of 2.5 small spacings where KWin 6.5 or later decorates
/// with Breeze, borderless and with rounded corners, as Breeze's own
/// `Decoration::recalculateBorders` decides.
fn breeze_radius(connection: &zbus::blocking::Connection) -> Option<f32> {
    let proxy =
        zbus::blocking::Proxy::new(connection, "org.kde.KWin", "/KWin", "org.kde.KWin").ok()?;
    let info: String = proxy.call("supportInformation", &()).ok()?;
    let field = |name: &str| {
        info.lines()
            .find_map(|line| line.strip_prefix(name)?.strip_prefix(": "))
    };
    let mut version = field("KWin version")?.split('.').map(str::parse::<u32>);
    let (Some(Ok(major)), Some(Ok(minor))) = (version.next(), version.next()) else {
        return None;
    };
    let breezerc = kconfig("breezerc");
    let rounded = kconfig_entries(&breezerc)
        .get(&("Common", "RoundedCorners"))
        .is_none_or(|value| *value != "false");
    // KDecoration's BorderSize::None is 0.
    let applies = (major, minor) >= (6, 5)
        && field("Plugin")? == "org.kde.breeze"
        && field("borderSize")? == "0"
        && rounded;
    Some(if applies {
        2.5 * field("smallSpacing")?.parse::<f32>().ok()?
    } else {
        0.0
    })
}

/// Decorated by the window manager, or on Wayland by the compositor where it offers
/// xdg-decoration and otherwise by winit's Adwaita frame; named so the desktop entry supplies
/// the icon on Wayland, while X11 takes it from the window.
pub fn window_attributes() -> WindowAttributes {
    use crate::desktop::APP_ID;
    Window::default_attributes()
        .with_name(APP_ID, APP_ID)
        .with_window_icon(crate::desktop::window_icon())
        .with_transparent(cuts_corners())
}

/// The window manager or the frame draws the title bar; the icon is the window's own where
/// the compositor allows.
pub fn install_title_bar(window: &Window) {
    crate::desktop::set_toplevel_icon(window);
}

/// Wayland's clipboard through the window's own connection, as not every compositor offers
/// a clipboard to clients without a window; X11's otherwise.
pub enum Clipboard {
    Wayland(smithay_clipboard::Clipboard),
    X11(arboard::Clipboard),
}

impl Clipboard {
    pub fn new(window: &Window) -> Result<Self, arboard::Error> {
        use winit::raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
        match window.display_handle().map(|handle| handle.as_raw()) {
            // The display outlives the clipboard, which the window's state owns.
            Ok(RawDisplayHandle::Wayland(handle)) => Ok(Self::Wayland(unsafe {
                smithay_clipboard::Clipboard::new(handle.display.as_ptr())
            })),
            _ => arboard::Clipboard::new().map(Self::X11),
        }
    }

    pub fn set_text(&mut self, text: String) -> Result<(), arboard::Error> {
        match self {
            Self::Wayland(clipboard) => {
                clipboard.store(text);
                Ok(())
            }
            Self::X11(clipboard) => clipboard.set_text(text),
        }
    }

    pub fn get_text(&mut self) -> Result<String, Box<dyn std::error::Error>> {
        Ok(match self {
            Self::Wayland(clipboard) => clipboard.load()?,
            Self::X11(clipboard) => clipboard.get_text()?,
        })
    }
}

/// Whether the system draws the title bar: the window manager, or on Wayland the compositor.
/// Where winit's Adwaita frame decorates the window instead, as on GNOME, the frame has no
/// header and the toolbar's row is the title bar, as it is where no frame could be made.
pub fn system_titlebar(window: &Window) -> bool {
    window.is_decorated() && !sctk_adwaita::FRAMED.load(Ordering::Relaxed)
}

/// Window managers show no document of a window's.
pub fn represent(_: &Window, _: Option<&std::path::Path>) {}

/// None: the title bars desktops draw are opaque.
pub fn install_backdrop(_: &Window) -> bool {
    false
}

/// The title bar's fills with the window focused and not, where the desktop's are known and
/// suit `appearance`: KWin's from the KDE colour scheme, or on GNOME the Adwaita header bar
/// that winit's frame and GNOME's both draw.
pub fn titlebar(appearance: Theme) -> Option<[[f32; 4]; 2]> {
    let fills = match desktop() {
        Desktop::Kde => kde_titlebar(&kconfig("kdeglobals")),
        Desktop::Gnome => adwaita_titlebar(appearance),
        Desktop::Other => return None,
    }
    .map(|[red, green, blue]| draw::srgb(red, green, blue));
    // Past mid grey, as against a colour scheme chosen in Options, the theme's own strip.
    let [red, green, blue, _] = fills[0];
    let dark = 0.2126 * red + 0.7152 * green + 0.0722 * blue < 0.18;
    (dark == (appearance == Theme::Dark)).then_some(fills)
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Desktop {
    Gnome,
    Kde,
    Other,
}

/// The desktop the session runs, whose look and motion the chrome follows.
fn desktop() -> Desktop {
    static DESKTOP: OnceLock<Desktop> = OnceLock::new();
    *DESKTOP.get_or_init(|| {
        let desktops = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
        let on = |name: &str| desktops.split(':').any(|desktop| desktop == name);
        if on("KDE") {
            Desktop::Kde
        } else if on("GNOME") {
            Desktop::Gnome
        } else {
            Desktop::Other
        }
    })
}

/// Menus in `appearance` as the desktop draws its own: libadwaita's popover menus on GNOME
/// and Breeze's on KDE, each moving as that desktop moves them.
pub fn menu(appearance: Theme) -> Option<ui::Menu> {
    match desktop() {
        Desktop::Gnome => {
            // GNOME's interface font, as "Family Size".
            let points = portal_setting("org.gnome.desktop.interface", "font-name")
                .and_then(|font| String::try_from(font).ok())
                .and_then(|font| font.rsplit_once(' ')?.1.parse().ok());
            Some(adwaita_menu(appearance, points.unwrap_or(11.0)))
        }
        Desktop::Kde => Some(breeze_menu(
            appearance,
            &kconfig("kdeglobals"),
            &kconfig("kwinrc"),
        )),
        Desktop::Other => None,
    }
}

/// libadwaita 1.7's `popover.menu` at `points`, which GTK 4 shows and hides at once.
fn adwaita_menu(appearance: Theme, points: f32) -> ui::Menu {
    let dark = appearance == Theme::Dark;
    let fill = if dark { [0x36, 0x36, 0x3a] } else { [0xff; 3] };
    // `--popover-fg-color`, and `currentColor` mixed with transparency at each strength.
    let (ink, strength) = if dark {
        ([0xff; 3], 1.0)
    } else {
        ([0, 0, 6], 0.8)
    };
    let ink = |alpha: f32| linear(over(ink, strength * alpha, fill));
    ui::Menu {
        fill: linear(fill),
        border: linear(over([0; 3], 0.14, fill)),
        shadows: vec![
            css_shadow(0.09, 5.0, 1.0, 1.0),
            css_shadow(0.05, 14.0, 2.0, 3.0),
        ],
        text: ink(1.0),
        // The accelerator's `--dim-opacity`, and `--disabled-opacity`.
        dim: ink(0.55),
        disabled: ink(0.5),
        highlight: ink(0.1),
        highlight_border: None,
        // `$border_color`.
        rule: ink(0.15),
        font_size: points * 4.0 / 3.0,
        radius: 15.0,
        pad: 6.0,
        row: 32.0,
        row_radius: 9.0,
        row_pad: 12.0,
        rule_band: 13.0,
        rule_inset: 0.0,
        motion: ui::PopupMotion::Cut,
    }
}

/// Breeze's `QMenu` in `appearance`, coloured by kdeglobals `files` where the scheme suits
/// it, and faded by KWin's Fading Popups unless kwinrc `kwin` turns them off.
fn breeze_menu(appearance: Theme, files: &[String], kwin: &[String]) -> ui::Menu {
    let entries = kconfig_entries(files);
    let color = |group, key| kde_color(entries.get(&(group, key))?);
    // Breeze Light's or Breeze Dark's window, its text, a view and the focus decoration.
    let stock = if appearance == Theme::Dark {
        [[32, 35, 38], [252; 3], [20, 22, 24], [61, 174, 233]]
    } else {
        [[239, 240, 241], [35, 38, 41], [255; 3], [61, 174, 233]]
    };
    let scheme = [
        ("Colors:Window", "BackgroundNormal"),
        ("Colors:Window", "ForegroundNormal"),
        ("Colors:View", "BackgroundNormal"),
        ("Colors:View", "DecorationFocus"),
    ];
    let mut colors: [[u8; 3]; 4] = std::array::from_fn(|index| {
        let (group, key) = scheme[index];
        color(group, key).unwrap_or(stock[index])
    });
    let window = linear(colors[0]);
    let dark = 0.2126 * window[0] + 0.7152 * window[1] + 0.0722 * window[2] < 0.18;
    if dark != (appearance == Theme::Dark) {
        colors = stock;
    }
    let [window, text, view, focus] = colors;
    // As `Helper::frameBackgroundColor`, `frameOutlineColor` and `focusOutlineColor` mix them.
    let fill = over(view, 0.3, window);
    let outline = linear(over(text, 0.2, window));
    let kwin = kconfig_entries(kwin);
    let factor = entries
        .get(&("KDE", "AnimationDurationFactor"))
        .and_then(|factor| factor.parse().ok())
        .unwrap_or(1.0_f32);
    let fades = kwin
        .get(&("Plugins", "fadingpopupsEnabled"))
        .is_none_or(|enabled| *enabled != "false");
    let points = entries
        .get(&("General", "font"))
        .and_then(|font| font.split(',').nth(1)?.parse().ok())
        .unwrap_or(10.0_f32);
    ui::Menu {
        fill: linear(fill),
        border: outline,
        // `ShadowHelper`'s default large shadow, which KWin draws around the menu.
        shadows: vec![
            css_shadow(0.22, 20.0, 5.0, 0.0),
            css_shadow(0.12, 10.0, 2.0, 0.0),
        ],
        text: linear(text),
        // Accelerators draw at 70% opacity; disabled text approximates the scheme's fade.
        dim: linear(over(text, 0.7, fill)),
        disabled: linear(over(text, 0.35, fill)),
        highlight: linear(over(focus, 0.3, fill)),
        highlight_border: Some(linear(over(text, 0.15, focus))),
        rule: outline,
        font_size: points * 4.0 / 3.0,
        radius: 5.0,
        pad: 4.0,
        row: 26.0,
        row_radius: 5.0,
        // `MenuItem_MarginWidth` and `MenuItem_ExtraLeftMargin`.
        row_pad: 9.0,
        rule_band: 7.0,
        rule_inset: 5.0,
        motion: if fades && factor > 0.0 {
            ui::PopupMotion::Fade([0.15 * factor, 0.6 * factor])
        } else {
            ui::PopupMotion::Cut
        },
    }
}

/// `top` over `under` at `alpha`, blended in sRGB as GTK and Qt paint.
fn over(top: [u8; 3], alpha: f32, under: [u8; 3]) -> [u8; 3] {
    std::array::from_fn(|channel| {
        let [top, under] = [top[channel], under[channel]].map(f32::from);
        (under + (top - under) * alpha).round() as u8
    })
}

fn linear([red, green, blue]: [u8; 3]) -> [f32; 4] {
    draw::srgb(red, green, blue)
}

/// A black `box-shadow` at `alpha`, as dark over white as GTK and Qt blend it in sRGB.
fn css_shadow(alpha: f32, blur: f32, drop: f32, spread: f32) -> ui::Shadow {
    let [left, ..] = linear(over([0; 3], alpha, [0xff; 3]));
    ui::Shadow {
        color: [0.0, 0.0, 0.0, 1.0 - left],
        blur,
        drop,
        spread,
    }
}

/// KConfig file `name` from the system's configuration directories up to the user's, in
/// rising precedence.
fn kconfig(name: &str) -> Vec<String> {
    let system = std::env::var("XDG_CONFIG_DIRS")
        .ok()
        .filter(|dirs| !dirs.is_empty())
        .unwrap_or_else(|| "/etc/xdg".into());
    let mut dirs: Vec<PathBuf> = std::env::split_paths(&system).collect();
    dirs.reverse();
    dirs.extend(xdg_base("XDG_CONFIG_HOME", ".config"));
    dirs.iter()
        .filter_map(|dir| std::fs::read_to_string(dir.join(name)).ok())
        .collect()
}

/// The entries of KConfig `files` in rising precedence, by group and key.
fn kconfig_entries(files: &[String]) -> std::collections::HashMap<(&str, &str), &str> {
    let mut entries = std::collections::HashMap::new();
    for file in files {
        let mut group = "";
        for line in file.lines().map(str::trim) {
            if let Some(name) = line
                .strip_prefix('[')
                .and_then(|line| line.strip_suffix(']'))
            {
                group = name;
            } else if let Some((key, value)) = line.split_once('=') {
                entries.insert((group, key.trim()), value.trim());
            }
        }
    }
    entries
}

/// KWin's title bar fills, focused and not, from kdeglobals `files` in rising precedence: the
/// colour scheme's header colours where it has them, as KWin prefers, else its window
/// manager's, else Breeze Light's header.
fn kde_titlebar(files: &[String]) -> [[u8; 3]; 2] {
    let entries = kconfig_entries(files);
    let color = |group, key| kde_color(entries.get(&(group, key))?);
    let pair = |active: [u8; 3], inactive: Option<[u8; 3]>| [active, inactive.unwrap_or(active)];
    if let Some(active) = color("Colors:Header", "BackgroundNormal") {
        pair(active, color("Colors:Header][Inactive", "BackgroundNormal"))
    } else if let Some(active) = color("WM", "activeBackground") {
        pair(active, color("WM", "inactiveBackground"))
    } else {
        [[222, 224, 226], [239, 240, 241]]
    }
}

/// A KConfig colour: `r,g,b` with an optional alpha, or `#rrggbb`.
fn kde_color(value: &str) -> Option<[u8; 3]> {
    if let Some(hex) = value.strip_prefix('#') {
        let [_, red, green, blue] = u32::from_str_radix(hex, 16)
            .ok()
            .filter(|_| hex.len() == 6)?
            .to_be_bytes();
        return Some([red, green, blue]);
    }
    let mut channels = value.split(',').map(|channel| channel.trim().parse().ok());
    Some([channels.next()??, channels.next()??, channels.next()??])
}

/// The header bar of winit's Adwaita frame, focused and not.
fn adwaita_titlebar(appearance: Theme) -> [[u8; 3]; 2] {
    let theme = match appearance {
        Theme::Dark => sctk_adwaita::theme::ColorTheme::dark(),
        Theme::Light => sctk_adwaita::theme::ColorTheme::light(),
    };
    [theme.active, theme.inactive].map(|colors| {
        let color = colors.headerbar.to_color_u8();
        [color.red(), color.green(), color.blue()]
    })
}

/// The window's buttons at the title bar's trailing end, in the order GNOME's
/// `button-layout` lists them, which is close alone by default; elsewhere minimize,
/// maximize and close.
pub fn window_controls(ui: &mut Ui, window: &Window) {
    static CONTROLS: OnceLock<Vec<&'static str>> = OnceLock::new();
    let controls = CONTROLS.get_or_init(|| {
        let layout = portal_setting("org.gnome.desktop.wm.preferences", "button-layout")
            .and_then(|value| String::try_from(value).ok());
        match layout {
            Some(layout) => layout
                .split([':', ','])
                .filter_map(|name| {
                    ["minimize", "maximize", "close"]
                        .into_iter()
                        .find(|known| *known == name)
                })
                .collect(),
            None if desktop() == Desktop::Gnome => vec!["close"],
            None => vec!["minimize", "maximize", "close"],
        }
    });
    let text = ui.theme.text;
    let disc = |alpha| [text[0], text[1], text[2], alpha];
    for control in controls {
        let icon = match *control {
            "minimize" => MINIMIZE,
            "maximize" if window.is_maximized() => RESTORE,
            "maximize" => MAXIMIZE,
            _ => crate::art::CLOSE,
        };
        // Round, on a faint disc, as libadwaita's are.
        let clicked = ui
            .leaf(
                control,
                Spec {
                    flags: Flags::CLICKABLE,
                    size: [px(ui::shell::TOOL); 2],
                    icon: Some(icon),
                    color: Some(text),
                    fill: Some(disc(0.1)),
                    hover_fill: Some(disc(0.18)),
                    radius: ui::shell::TOOL / 2.0,
                    center: true,
                    role: Some(accesskit::Role::Button),
                    ..Spec::default()
                },
            )
            .clicked;
        let name = match *control {
            "minimize" => "Minimize",
            "maximize" if window.is_maximized() => "Restore",
            "maximize" => "Maximize",
            _ => "Close",
        };
        if let Some(node) = ui.access(ui.id(control)) {
            node.set_label(name);
        }
        match *control {
            _ if !clicked => {}
            "minimize" => window.set_minimized(true),
            "maximize" => zoom(window),
            _ => {
                if let Some(proxy) = QUIT.get() {
                    let _ = proxy.send_event(crate::UserEvent::Quit);
                }
            }
        }
    }
}

/// The edge or corner a press at `pointer`, in logical pixels, resizes the window from, when
/// the window has no frame to take it.
pub fn resize_direction(window: &Window, pointer: [f32; 2]) -> Option<ResizeDirection> {
    if window.is_decorated() {
        return None;
    }
    let size = window.inner_size().to_logical::<f32>(window.scale_factor());
    let [x, y] = pointer;
    let near = |value: f32, extent: f32, reach: f32| (value < reach, value > extent - reach);
    let (mut west, mut east) = near(x, size.width, EDGE);
    let (mut north, mut south) = near(y, size.height, EDGE);
    if north || south {
        (west, east) = near(x, size.width, CORNER);
    }
    if west || east {
        (north, south) = near(y, size.height, CORNER);
    }
    let direction = match (north, south, west, east) {
        (true, _, true, _) => ResizeDirection::NorthWest,
        (true, _, _, true) => ResizeDirection::NorthEast,
        (_, true, true, _) => ResizeDirection::SouthWest,
        (_, true, _, true) => ResizeDirection::SouthEast,
        (true, ..) => ResizeDirection::North,
        (_, true, ..) => ResizeDirection::South,
        (.., true, _) => ResizeDirection::West,
        (.., true) => ResizeDirection::East,
        _ => return None,
    };
    (!window.is_maximized() && window.is_resizable()).then_some(direction)
}

pub fn zoom(window: &Window) {
    window.set_maximized(!window.is_maximized());
}

/// Winit delivers typed text with its key events on Linux.
pub fn install_text_input(_: &Window) {}

pub fn move_cursor() -> winit::window::CursorIcon {
    winit::window::CursorIcon::Move
}

/// No system border lies under the chrome.
pub fn cover_border_line(_: &mut Ui, _: f32) {}

pub fn with_pool<R>(run: impl FnOnce() -> R) -> R {
    run()
}

/// The interface keeps fontconfig's font and scrollbars overlay the content.
pub fn system_interface(_: &mut Ui) {}

/// Resizing takes the window's edges.
pub fn resize_grip(_: &mut Ui, _: &Window, _: [f32; 2]) {}

/// Options opens from the sidebar's footer and Ctrl+Comma, as Linux apps have no shared menu.
pub fn install_menu() {}

/// The toolbar and keyboard stand in for a menu bar.
pub fn update_menu(_: impl FnOnce() -> Vec<crate::commands::Status>) {}

pub fn update_tag_menu(_: &[canvas::editor::NoteTag]) {}

pub fn clear_marked_text(_: &Window) {}

#[cfg(feature = "wgpu")]
pub fn configure_presentation(_: &wgpu::Surface<'_>) {}

#[cfg(feature = "wgpu")]
pub fn commit_presentation(_: &Window) {}

/// winit reports no tablet pressure here, so every stroke keeps its pen's width.
pub fn pen_pressure() -> Option<f32> {
    None
}

/// GTK's default double-click time.
pub fn double_click_interval() -> Duration {
    Duration::from_millis(400)
}

/// The desktop's colour scheme from the settings portal, which GNOME, KDE and others serve;
/// light where it states no preference.
pub fn appearance(_: &Window) -> Theme {
    let dark = portal_color_scheme()
        .unwrap_or_else(|| std::env::var("GTK_THEME").is_ok_and(|theme| theme.ends_with(":dark")));
    if dark { Theme::Dark } else { Theme::Light }
}

/// Asks for the desktop's colours again whenever the settings portal reports a change to the
/// colour scheme, or to KDE's colours.
fn watch_settings(proxy: EventLoopProxy<crate::UserEvent>) {
    std::thread::spawn(move || {
        let Ok(connection) = zbus::blocking::Connection::session() else {
            return;
        };
        let Ok(portal) = zbus::blocking::Proxy::new(
            &connection,
            "org.freedesktop.portal.Desktop",
            "/org/freedesktop/portal/desktop",
            "org.freedesktop.portal.Settings",
        ) else {
            return;
        };
        let Ok(changes) = portal.receive_signal("SettingChanged") else {
            return;
        };
        for change in changes {
            let Ok((namespace, _, _)) = change
                .body()
                .deserialize::<(String, String, zbus::zvariant::OwnedValue)>()
            else {
                continue;
            };
            if (namespace == "org.freedesktop.appearance"
                || namespace.starts_with("org.kde.kdeglobals"))
                && proxy.send_event(crate::UserEvent::Appearance).is_err()
            {
                return;
            }
        }
    });
}

fn portal_color_scheme() -> Option<bool> {
    let scheme = portal_setting("org.freedesktop.appearance", "color-scheme")?;
    match u32::try_from(scheme).ok()? {
        1 => Some(true),
        2 => Some(false),
        _ => None,
    }
}

/// Setting `key` in `namespace` from the settings portal.
fn portal_setting(namespace: &str, key: &str) -> Option<zbus::zvariant::OwnedValue> {
    use zbus::zvariant::{OwnedValue, Value};
    // A missing portal must not hold up the window for D-Bus's 25 s default.
    let connection = zbus::blocking::connection::Builder::session()
        .ok()?
        .method_timeout(Duration::from_millis(500))
        .build()
        .ok()?;
    let proxy = zbus::blocking::Proxy::new(
        &connection,
        "org.freedesktop.portal.Desktop",
        "/org/freedesktop/portal/desktop",
        "org.freedesktop.portal.Settings",
    )
    .ok()?;
    let key = (namespace, key);
    // Portals before ReadOne answer Read, with the value in a second variant.
    let reply: OwnedValue = proxy
        .call("ReadOne", &key)
        .or_else(|_| proxy.call("Read", &key))
        .ok()?;
    let mut value = Value::from(reply);
    while let Value::Value(inner) = value {
        value = *inner;
    }
    value.try_into_owned().ok()
}

pub const SHOW_FILE: &str = "Show in Files";

/// Selects `file` in the desktop's file manager, or opens its folder where none answers
/// `org.freedesktop.FileManager1`.
pub fn show_file(file: &std::path::Path) {
    let shown = zbus::blocking::connection::Builder::session()
        .and_then(|builder| builder.method_timeout(Duration::from_secs(2)).build())
        .and_then(|connection| {
            connection.call_method(
                Some("org.freedesktop.FileManager1"),
                "/org/freedesktop/FileManager1",
                Some("org.freedesktop.FileManager1"),
                "ShowItems",
                &(vec![file_uri(file)], ""),
            )
        });
    if shown.is_err()
        && let Some(folder) = file.parent()
    {
        let _ = Command::new("xdg-open").arg(folder).spawn();
    }
}

/// The `file://` URI of absolute `path`, its bytes outside RFC 3986's unreserved set and `/`
/// percent-encoded.
fn file_uri(path: &std::path::Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut uri = "file://".to_owned();
    for &byte in path.as_os_str().as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                uri.push(byte as char)
            }
            _ => uri.push_str(&format!("%{byte:02X}")),
        }
    }
    uri
}

pub fn cache_dir() -> Option<PathBuf> {
    xdg_dir("XDG_CACHE_HOME", ".cache")
}

/// The share holding `path` where it is mounted from an SMB server (CIFS or SMB 3).
pub fn smb_mount(path: &std::path::Path) -> Option<crate::library::Mount> {
    let mounts = std::fs::read_to_string("/proc/self/mounts").ok()?;
    // The mount holding the path is the one at its longest ancestor.
    let (source, point, options, within) = mounts
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let (source, point, kind, options) = (
                fields.next()?,
                fields.next()?,
                fields.next()?,
                fields.next()?,
            );
            if !matches!(kind, "cifs" | "smb3") {
                return None;
            }
            let point = point.replace("\\040", " ");
            let within = crate::library::within_mount(path, &point)?;
            Some((
                source.replace("\\040", " "),
                point,
                options.to_owned(),
                within,
            ))
        })
        .max_by_key(|(_, point, _, _)| point.len())?;
    crate::library::Mount::parse(&source, &within, &options)
}

/// The password the Secret Service keeps for `mount`'s account, as GNOME's file manager
/// saves one, or what the user types when it keeps none.
pub fn smb_login(mount: &crate::library::Mount) -> Result<crate::library::Login, String> {
    let user = mount.user.clone().unwrap_or_default();
    let mut lookup = Command::new("secret-tool");
    lookup.args(["lookup", "protocol", "smb", "server", mount.host()]);
    if !user.is_empty() {
        lookup.args(["user", &user]);
    }
    if let Ok(output) = lookup.output()
        && output.status.success()
        && !output.stdout.is_empty()
    {
        return Ok(crate::library::Login {
            user,
            password: String::from_utf8_lossy(&output.stdout)
                .trim_end_matches('\n')
                .to_owned(),
            domain: mount.domain.clone(),
        });
    }
    let title = format!("Sign in to {}", mount.server);
    let asked = dialog(
        ["--password", "--username", &format!("--title={title}")],
        ["--password", &title],
    )
    .map_err(str::to_owned)?
    .ok_or_else(|| "Signing in was canceled".to_owned())?;
    // zenity answers "user|password"; kdialog only the password.
    let (typed, password) = asked
        .split_once('|')
        .map_or((user.clone(), asked.clone()), |(typed, password)| {
            (typed.to_owned(), password.to_owned())
        });
    Ok(crate::library::Login {
        user: if typed.is_empty() { user } else { typed },
        password,
        domain: mount.domain.clone(),
    })
}

/// What the sign-in offers for keeping a password, where the Secret Service's tool is
/// installed to keep it.
pub fn remember_label() -> Option<&'static str> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .any(|folder| folder.join("secret-tool").is_file())
        .then_some("Remember this password")
}

/// Keeps `login`'s password for `mount`'s server in the Secret Service, where `smb_login`
/// and GNOME's file manager look for it.
pub fn save_login(
    mount: &crate::library::Mount,
    login: &crate::library::Login,
) -> Result<(), String> {
    use std::io::Write;
    let label = format!("{} on {}", login.user, mount.host());
    let mut store = Command::new("secret-tool")
        .args(["store", "--label", &label, "protocol", "smb"])
        .args(["server", mount.host(), "user", &login.user])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| error.to_string())?;
    store
        .stdin
        .take()
        .ok_or("The Secret Service didn't take the password")?
        .write_all(login.password.as_bytes())
        .map_err(|error| error.to_string())?;
    match store.wait() {
        Ok(status) if status.success() => Ok(()),
        _ => Err("The Secret Service didn't keep the password".to_owned()),
    }
}

pub fn settings_dir() -> Option<PathBuf> {
    xdg_dir("XDG_CONFIG_HOME", ".config")
}

/// The app's folder in the XDG base directory `variable` names, or in `fallback` under home.
fn xdg_dir(variable: &str, fallback: &str) -> Option<PathBuf> {
    Some(xdg_base(variable, fallback)?.join("snowbound"))
}

pub(crate) fn xdg_base(variable: &str, fallback: &str) -> Option<PathBuf> {
    std::env::var_os(variable)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| Some(PathBuf::from(std::env::var_os("HOME")?).join(fallback)))
}

/// The account's full name from its passwd entry, or its login where that has none.
pub fn user_name() -> String {
    let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
    let mut found = std::ptr::null_mut();
    let mut buffer = vec![0 as libc::c_char; 16 * 1024];
    unsafe {
        libc::getpwuid_r(
            libc::getuid(),
            &mut entry,
            buffer.as_mut_ptr(),
            buffer.len(),
            &mut found,
        )
    };
    let field = |value: *const libc::c_char| {
        (!found.is_null() && !value.is_null()).then(|| {
            unsafe { CStr::from_ptr(value) }
                .to_string_lossy()
                .into_owned()
        })
    };
    // GECOS holds the full name before any comma-separated office and phone fields.
    field(entry.pw_gecos)
        .and_then(|gecos| gecos.split(',').next().map(str::to_owned))
        .filter(|name| !name.trim().is_empty())
        .or_else(|| field(entry.pw_name))
        .or_else(|| std::env::var("USER").ok())
        .unwrap_or_default()
}

/// The keyboard's language is not exposed portably, so pasted text takes the locale's.
pub fn input_language() -> String {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .filter_map(|name| std::env::var(name).ok())
        .find(|value| !value.is_empty())
        .map(|locale| locale_tag(&locale))
        .unwrap_or_default()
}

/// A POSIX locale name, such as `pt_BR.UTF-8@euro`, as a BCP-47 tag.
fn locale_tag(locale: &str) -> String {
    let name = locale.split(['.', '@']).next().unwrap_or_default();
    if matches!(name, "C" | "POSIX") {
        String::new()
    } else {
        name.replace('_', "-")
    }
}

const FILETIME_EPOCH: i64 = 11_644_473_600;

fn local_time(filetime: u64) -> libc::tm {
    let seconds = (filetime / 10_000_000) as i64 - FILETIME_EPOCH;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&seconds, &mut tm) };
    tm
}

fn format(tm: &libc::tm, pattern: &CStr) -> String {
    let mut buffer = [0u8; 256];
    let length = unsafe {
        libc::strftime(
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            pattern.as_ptr(),
            tm,
        )
    };
    String::from_utf8_lossy(&buffer[..length]).into_owned()
}

/// FILETIME as the locale's short date, as OneNote labels a conflict page.
pub fn short_date(filetime: u64) -> String {
    format(&local_time(filetime), c"%x")
}

/// The page's date and time labels, spelled out as OneNote shows them.
fn date_labels(tm: &libc::tm) -> [String; 2] {
    let meridiem = unsafe { CStr::from_ptr(libc::nl_langinfo(libc::AM_STR)) };
    let time: &CStr = if meridiem.is_empty() {
        c"%H:%M"
    } else {
        c"%-I:%M %p"
    };
    [format(tm, c"%A, %B %-d, %Y"), format(tm, time)]
}

/// FILETIME as a new page's title shows it: the long date and the short time.
pub fn date_text(filetime: u64) -> [String; 2] {
    date_labels(&local_time(filetime))
}

/// Asks for the page's date or time with the desktop's dialog tool.
pub fn edit_date(
    timestamp: u64,
    field: DateField,
    title: &str,
) -> Result<Option<(u64, [String; 2])>, &'static str> {
    let tm = local_time(timestamp);
    let answer = match field {
        DateField::Date => dialog(
            [
                "--calendar",
                &format!("--title={title}"),
                "--text=",
                &format!("--day={}", tm.tm_mday),
                &format!("--month={}", tm.tm_mon + 1),
                &format!("--year={}", tm.tm_year + 1900),
                "--date-format=%Y-%m-%d",
            ],
            ["--calendar", title, "--dateformat", "yyyy-MM-dd"],
        )?,
        DateField::Time => {
            let current = format(&tm, c"%H:%M");
            dialog(
                [
                    "--entry",
                    &format!("--title={title}"),
                    "--text=Time, as hours and minutes:",
                    &format!("--entry-text={current}"),
                ],
                ["--inputbox", "Time, as hours and minutes:", &current],
            )?
        }
    };
    let Some(answer) = answer else {
        return Ok(None);
    };
    let numbers: Vec<i32> = answer
        .split(|c: char| !c.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .map(|part| part.parse().unwrap_or(i32::MAX))
        .collect();
    let afternoon = answer.to_lowercase().contains("pm");
    merge_date(timestamp, tm, field, &numbers, afternoon).map(Some)
}

fn merge_date(
    timestamp: u64,
    mut tm: libc::tm,
    field: DateField,
    numbers: &[i32],
    afternoon: bool,
) -> Result<(u64, [String; 2]), &'static str> {
    match (field, numbers) {
        (DateField::Date, &[year, month, day])
            if (1..=12).contains(&month) && (1..=31).contains(&day) =>
        {
            tm.tm_year = year - 1900;
            tm.tm_mon = month - 1;
            tm.tm_mday = day;
        }
        (DateField::Time, &[hour, minute])
            if (0..24).contains(&hour) && (0..60).contains(&minute) =>
        {
            tm.tm_hour = if afternoon && hour < 12 {
                hour + 12
            } else {
                hour
            };
            tm.tm_min = minute;
        }
        _ => return Err(crate::DATE_UNCHOSEN),
    }
    tm.tm_isdst = -1;
    let seconds = unsafe { libc::mktime(&mut tm) };
    let outside_range = crate::DATE_OUT_OF_RANGE;
    let updated = u64::try_from(seconds + FILETIME_EPOCH)
        .ok()
        .and_then(|seconds| seconds.checked_mul(10_000_000))
        .and_then(|ticks| ticks.checked_add(timestamp % 10_000_000))
        .ok_or(outside_range)?;
    Ok((updated, date_labels(&tm)))
}

/// Runs zenity, or kdialog where zenity is missing; the answer is None when cancelled.
fn dialog<const Z: usize, const K: usize>(
    zenity: [&str; Z],
    kdialog: [&str; K],
) -> Result<Option<String>, &'static str> {
    let output = Command::new("zenity")
        .args(zenity)
        .output()
        .or_else(|_| Command::new("kdialog").args(kdialog).output())
        .map_err(|_| "Install zenity or kdialog for Snowbound's dialogs.")?;
    Ok(output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned()))
}

/// Asks for a file to insert, one of `types` (extensions) unless empty, titled `title`.
pub fn pick_file(title: &str, types: &[&str]) -> Option<PathBuf> {
    let patterns = match types {
        [] => "*".to_owned(),
        types => types
            .iter()
            .map(|kind| format!("*.{kind}"))
            .collect::<Vec<_>>()
            .join(" "),
    };
    let asked = dialog(
        [
            "--file-selection",
            &format!("--title={title}"),
            &format!("--file-filter={patterns}"),
        ],
        ["--getopenfilename", ".", &patterns, "--title", title],
    );
    asked
        .unwrap_or_else(|error| {
            eprintln!("{error}");
            None
        })
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
}

/// No icon theme lookup; the page draws a blank page for the file.
pub fn file_icon(_: &std::path::Path) -> Option<Vec<u8>> {
    None
}

/// winit reports drops without a position; the file goes to the caret.
pub fn drop_point(_: &Window) -> Option<[f32; 2]> {
    None
}

/// Opens a copy of an attachment with the desktop's application for it.
pub fn open_file(path: &std::path::Path) {
    reveal(path);
}

/// Opens GNOME's character map, whose picks the user copies in.
pub fn character_palette() {
    if let Err(error) = Command::new("gnome-characters").spawn() {
        eprintln!("Cannot open the character map: {error}");
    }
}

/// Opens `target`, a folder or a link's URL, with the desktop's handler.
pub fn reveal(target: impl AsRef<std::ffi::OsStr>) {
    let target = target.as_ref();
    if let Err(error) = Command::new("xdg-open").arg(target).spawn() {
        eprintln!("Cannot open {}: {error}", target.display());
    }
}

/// Asks whether to go ahead with `action`; false when no tool can ask.
pub fn confirm(message: &str, detail: &str, cancel: &str, action: &str) -> bool {
    let status = Command::new("zenity")
        .args([
            "--question",
            &format!("--title={message}"),
            &format!("--text={detail}"),
            &format!("--ok-label={action}"),
            &format!("--cancel-label={cancel}"),
        ])
        .status()
        .or_else(|_| {
            Command::new("kdialog")
                .args(["--warningcontinuecancel", detail, "--title", message])
                .args(["--continue-label", action])
                .status()
        });
    match status {
        Ok(status) => status.success(),
        Err(_) => {
            eprintln!("Install zenity or kdialog to answer: {message}");
            false
        }
    }
}

/// Asks for a notebook's table of contents or a section file, titled `title`; None when
/// cancelled or when no tool can ask.
pub fn pick_notebook(title: &str) -> Option<PathBuf> {
    let asked = dialog(
        [
            "--file-selection",
            &format!("--title={title}"),
            "--file-filter=OneNote notebooks and sections | *.onetoc2 *.one",
        ],
        [
            "--getopenfilename",
            ".",
            "*.onetoc2 *.one",
            "--title",
            title,
        ],
    );
    asked
        .unwrap_or_else(|error| {
            eprintln!("{error}");
            None
        })
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
}

/// Asks where to put something named `name` by default; the dialog names its own button.
pub fn pick_new(
    title: &str,
    name: &str,
    _action: &str,
    _folder: Option<&std::path::Path>,
) -> Option<PathBuf> {
    let asked = dialog(
        [
            "--file-selection",
            "--save",
            &format!("--title={title}"),
            &format!("--filename={name}"),
        ],
        ["--getsavefilename", name, "--title", title],
    );
    asked
        .unwrap_or_else(|error| {
            eprintln!("{error}");
            None
        })
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
}

/// Tells the user something they asked for could not be done: `message`, then what to do.
pub fn alert(message: &str, detail: &str) {
    show(["--warning", "--sorry"], message, detail);
}

/// Tells the user how something they asked for turned out.
pub fn inform(message: &str, detail: &str) {
    show(["--info", "--msgbox"], message, detail);
}

/// Shows `message` and `detail` in zenity's or else kdialog's dialog of the `kinds`.
fn show([zenity, kdialog]: [&'static str; 2], message: &str, detail: &str) {
    let (title, text) = (message.to_owned(), detail.to_owned());
    std::thread::spawn(move || {
        let shown = Command::new("zenity")
            .args([
                zenity,
                &format!("--title={title}"),
                &format!("--text={text}"),
            ])
            .status()
            .or_else(|_| {
                Command::new("kdialog")
                    .args([kdialog, &text, "--title", &title])
                    .status()
            });
        if shown.is_err() {
            eprintln!("{title}: {text}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(year: i32, month: i32, day: i32, hour: i32, minute: i32) -> libc::tm {
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        (tm.tm_year, tm.tm_mon, tm.tm_mday, tm.tm_hour, tm.tm_min) =
            (year - 1900, month - 1, day, hour, minute);
        tm.tm_isdst = -1;
        tm
    }

    fn filetime(mut tm: libc::tm) -> u64 {
        (unsafe { libc::mktime(&mut tm) } + FILETIME_EPOCH) as u64 * 10_000_000
    }

    #[test]
    fn date_changes_keep_the_other_field_and_the_filetime_fraction() {
        let original = filetime(at(2026, 9, 26, 15, 4)) + 9_123_456;
        let tm = local_time(original);
        let (updated, labels) =
            merge_date(original, tm, DateField::Date, &[2027, 2, 28], false).unwrap();
        assert_eq!(updated, filetime(at(2027, 2, 28, 15, 4)) + 9_123_456);
        assert!(labels.iter().all(|label| !label.is_empty()));
        let (updated, _) = merge_date(original, tm, DateField::Time, &[9, 30], true).unwrap();
        assert_eq!(updated, filetime(at(2026, 9, 26, 21, 30)) + 9_123_456);
        assert!(merge_date(original, tm, DateField::Date, &[2026, 13, 1], false).is_err());
        assert!(merge_date(original, tm, DateField::Time, &[9], false).is_err());
        assert!(merge_date(original, tm, DateField::Date, &[1500, 1, 1], false).is_err());
    }

    #[test]
    fn locales_become_language_tags() {
        assert_eq!(locale_tag("pt_BR.UTF-8@euro"), "pt-BR");
        assert_eq!(locale_tag("de_DE"), "de-DE");
        assert_eq!(locale_tag("C.UTF-8"), "");
    }

    #[test]
    fn kwin_title_bars_take_the_colour_scheme_s_header_then_its_window_manager_colours() {
        let breeze_dark = "[Colors:Header]\nBackgroundNormal=41,44,48\nForegroundNormal=252,252,252\n\
            [Colors:Header][Inactive]\nBackgroundNormal=32,35,38\n\
            [Colors:Window]\nBackgroundNormal=32,35,38\n\
            [WM]\nactiveBackground=49,54,59\ninactiveBackground=42,46,50\n";
        assert_eq!(
            kde_titlebar(&[breeze_dark.into()]),
            [[41, 44, 48], [32, 35, 38]]
        );
        let legacy = "[General]\nColorScheme=Old\n\n[WM]\nactiveBackground=48,174,232\n\
            inactiveBackground=#eff0f1\n";
        assert_eq!(
            kde_titlebar(&[legacy.into()]),
            [[48, 174, 232], [239, 240, 241]]
        );
        let header_only = "[Colors:Header]\nBackgroundNormal = 10, 20, 30, 255\n";
        assert_eq!(kde_titlebar(&[header_only.into()]), [[10, 20, 30]; 2]);
        // The user's file, last, overrides the system's.
        assert_eq!(
            kde_titlebar(&[breeze_dark.into(), header_only.into()]),
            [[10, 20, 30], [32, 35, 38]]
        );
        assert_eq!(
            kde_titlebar(&["# no colours\n[KDE]\nLookAndFeelPackage=x\n".into()]),
            [[222, 224, 226], [239, 240, 241]],
            "Breeze Light's header by default"
        );
    }

    #[test]
    fn breeze_menus_take_the_scheme_s_colours_and_kwin_s_fade() {
        let srgb = |[red, green, blue]: [u8; 3]| draw::srgb(red, green, blue);
        let menu = breeze_menu(Theme::Light, &[], &[]);
        // As measured from Dolphin's context menu under Breeze Light.
        assert_eq!(menu.fill, srgb([0xf4, 0xf5, 0xf5]));
        assert_eq!(menu.border, srgb([0xc6, 0xc8, 0xc9]));
        assert_eq!(menu.highlight, srgb([0xbd, 0xe0, 0xf1]));
        assert_eq!(menu.highlight_border, Some(srgb([0x39, 0x9a, 0xcc])));
        assert_eq!(menu.font_size, 10.0 * 4.0 / 3.0);
        assert_eq!(menu.motion, ui::PopupMotion::Fade([0.15, 0.6]));

        let settings = "[General]\nfont=Noto Sans,12,-1,5,400,0,0,0,0,0\n\
            [KDE]\nAnimationDurationFactor=0.5\n\
            [Colors:Window]\nBackgroundNormal=32,35,38\nForegroundNormal=252,252,252\n";
        let menu = breeze_menu(Theme::Dark, &[settings.into()], &[]);
        assert_eq!(menu.text, srgb([252; 3]));
        assert_eq!(menu.font_size, 16.0);
        assert_eq!(menu.motion, ui::PopupMotion::Fade([0.075, 0.3]));
        assert_eq!(
            breeze_menu(Theme::Light, &[settings.into()], &[]).fill,
            srgb([0xf4, 0xf5, 0xf5]),
            "a dark scheme under a light appearance takes Breeze Light's"
        );

        let instant = "[KDE]\nAnimationDurationFactor=0\n";
        let unfaded = "[Plugins]\nfadingpopupsEnabled=false\n";
        for (kdeglobals, kwinrc) in [(instant, ""), ("", unfaded)] {
            let motion = breeze_menu(Theme::Light, &[kdeglobals.into()], &[kwinrc.into()]).motion;
            assert_eq!(motion, ui::PopupMotion::Cut);
        }
    }

    #[test]
    fn adwaita_menus_mix_the_popover_s_ink_as_its_stylesheet_does() {
        let srgb = |[red, green, blue]: [u8; 3]| draw::srgb(red, green, blue);
        let light = adwaita_menu(Theme::Light, 11.0);
        assert_eq!(light.text, srgb([51, 51, 56]));
        assert_eq!(light.highlight, srgb([235; 3]));
        assert_eq!(light.border, srgb([219; 3]));
        let dark = adwaita_menu(Theme::Dark, 11.0);
        assert_eq!(dark.highlight, srgb([74, 74, 78]));
        assert_eq!(dark.motion, ui::PopupMotion::Cut);
    }

    #[test]
    fn file_uris_escape_all_but_unreserved_bytes() {
        let path = std::path::Path::new("/home/me/Notes & Plans/Café.one");
        assert_eq!(
            file_uri(path),
            "file:///home/me/Notes%20%26%20Plans/Caf%C3%A9.one"
        );
    }

    #[test]
    fn kconfig_colours_read_as_channels_or_hex() {
        assert_eq!(kde_color("222,224,226"), Some([222, 224, 226]));
        assert_eq!(kde_color("1, 2, 3, 128"), Some([1, 2, 3]));
        assert_eq!(kde_color("#2a2e32"), Some([0x2a, 0x2e, 0x32]));
        assert_eq!(kde_color("#2a2e"), None);
        assert_eq!(kde_color("256,0,0"), None);
        assert_eq!(kde_color("1,2"), None);
    }
}
