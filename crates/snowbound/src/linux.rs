//! Linux, on X11 and Wayland: window controls drawn in the title bar, dialogs through zenity
//! or kdialog, the colour scheme from the XDG settings portal, and text conventions from
//! the C library's locale.

use canvas::date::DateField;
use std::{ffi::CStr, path::PathBuf, process::Command, sync::OnceLock, time::Duration};
use ui::{Flags, Spec, Ui, px};
use winit::{
    error::EventLoopError,
    event_loop::{EventLoop, EventLoopProxy},
    platform::wayland::WindowAttributesExtWayland,
    window::{Icon, ResizeDirection, Theme, Window, WindowAttributes},
};

/// The name the desktop entry, the icon theme and window matching know the app by.
const APP_ID: &str = "snowbound";
/// The title bar's leading margin; the window controls sit at its trailing end.
pub const LEADING: f32 = 8.0;
/// Square, as compositors that round windows don't report by how much.
pub const CORNER_RADIUS: f32 = 0.0;
/// How far inside the window's edges a press resizes it, and how far along them a corner
/// reaches, in logical pixels.
const EDGE: f32 = 5.0;
const CORNER: f32 = 16.0;
const MINIMIZE: &[&str] = &[include_str!("../assets/icons/window-minimize.svg")];
const MAXIMIZE: &[&str] = &[include_str!("../assets/icons/window-maximize.svg")];
const RESTORE: &[&str] = &[include_str!("../assets/icons/window-restore.svg")];

static QUIT: OnceLock<EventLoopProxy<crate::UserEvent>> = OnceLock::new();

pub fn event_loop(_headless: bool) -> Result<EventLoop<crate::UserEvent>, EventLoopError> {
    // Month and day names and date orders follow the user's locale; other categories stay C.
    unsafe { libc::setlocale(libc::LC_TIME, c"".as_ptr()) };
    let event_loop = EventLoop::with_user_event().build()?;
    QUIT.set(event_loop.create_proxy())
        .expect("Only one application event loop is created");
    Ok(event_loop)
}

/// Decorated by the window manager, or on Wayland by the compositor where it offers
/// xdg-decoration and otherwise by winit's Adwaita frame; named so the desktop entry supplies
/// the icon on Wayland, while X11 takes it from the window.
pub fn window_attributes() -> WindowAttributes {
    let icon = icon_pixels(64).and_then(|(side, rgba)| Icon::from_rgba(rgba, side, side).ok());
    Window::default_attributes()
        .with_name(APP_ID, APP_ID)
        .with_window_icon(icon)
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
            Self::Wayland(clipboard) => Ok(clipboard.store(text)),
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

/// Whether the system draws the title bar; false only where Wayland's client-side frame
/// could not be made, leaving the app's own.
pub fn system_titlebar(window: &Window) -> bool {
    window.is_decorated()
}

/// Minimize, maximize and close at the title bar's trailing end.
pub fn window_controls(ui: &mut Ui, window: &Window) {
    let theme = ui.theme.clone();
    let mut button = |part: &str, icon| {
        ui.leaf(
            part,
            Spec {
                flags: Flags::CLICKABLE,
                size: [px(crate::TITLE + 12.0), px(crate::TITLE)],
                icon: Some(icon),
                color: Some(theme.text),
                hover_fill: Some(theme.hover()),
                center: true,
                ..Spec::default()
            },
        )
        .clicked
    };
    if button("minimize", MINIMIZE) {
        window.set_minimized(true);
    }
    if button(
        "maximize",
        if window.is_maximized() {
            RESTORE
        } else {
            MAXIMIZE
        },
    ) {
        zoom(window);
    }
    if button("close", crate::art::CLOSE)
        && let Some(proxy) = QUIT.get()
    {
        let _ = proxy.send_event(crate::UserEvent::Quit);
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

pub fn clear_marked_text(_: &Window) {}

pub fn configure_presentation(_: &wgpu::Surface<'_>) {}

pub fn commit_presentation(_: &Window) {}

/// GNOME has no character palette to open from an app.
pub fn show_character_palette() {}

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

fn portal_color_scheme() -> Option<bool> {
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
    let key = ("org.freedesktop.appearance", "color-scheme");
    // Portals before ReadOne answer Read, with the value in a second variant.
    let reply: OwnedValue = proxy
        .call("ReadOne", &key)
        .or_else(|_| proxy.call("Read", &key))
        .ok()?;
    let mut value = Value::from(reply);
    while let Value::Value(inner) = value {
        value = *inner;
    }
    match u32::try_from(value).ok()? {
        1 => Some(true),
        2 => Some(false),
        _ => None,
    }
}

pub fn cache_dir() -> Option<PathBuf> {
    xdg_dir("XDG_CACHE_HOME", ".cache")
}

/// The share holding `path` where it is mounted from an SMB server (CIFS or SMB 3).
pub fn smb_mount(path: &std::path::Path) -> Option<crate::library::Mount> {
    let mounts = std::fs::read_to_string("/proc/self/mounts").ok()?;
    // The mount holding the path is the one at its longest ancestor.
    let (source, point, options) = mounts
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let (source, point, kind, options) =
                (fields.next()?, fields.next()?, fields.next()?, fields.next()?);
            let point = point.replace("\\040", " ");
            (matches!(kind, "cifs" | "smb3") && path.starts_with(&point))
                .then(|| (source.replace("\\040", " "), point, options.to_owned()))
        })
        .max_by_key(|(_, point, _)| point.len())?;
    let within = path.strip_prefix(&point).ok()?.to_string_lossy().into_owned();
    crate::library::Mount::parse(&source, &within, &options)
}

/// The password the Secret Service keeps for `mount`'s account, as GNOME's file manager
/// saves one, or what the user types when it keeps none.
pub fn smb_login(mount: &crate::library::Mount) -> Result<crate::library::Login, String> {
    let user = mount.user.clone().unwrap_or_default();
    let mut lookup = Command::new("secret-tool");
    lookup.args(["lookup", "protocol", "smb", "server", &mount.server]);
    if !user.is_empty() {
        lookup.args(["user", &user]);
    }
    if let Ok(output) = lookup.output()
        && output.status.success()
        && !output.stdout.is_empty()
    {
        return Ok(crate::library::Login {
            user,
            password: String::from_utf8_lossy(&output.stdout).trim_end_matches('\n').to_owned(),
            domain: mount.domain.clone(),
        });
    }
    let title = format!("Sign in to {}", mount.server);
    let asked = dialog(
        ["--password", "--username", &format!("--title={title}")],
        ["--password", &title],
    )
    .map_err(str::to_owned)?
    .ok_or_else(|| "Signing in was cancelled".to_owned())?;
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

pub fn settings_dir() -> Option<PathBuf> {
    xdg_dir("XDG_CONFIG_HOME", ".config")
}

/// The app's folder in the XDG base directory `variable` names, or in `fallback` under home.
fn xdg_dir(variable: &str, fallback: &str) -> Option<PathBuf> {
    std::env::var_os(variable)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| Some(PathBuf::from(std::env::var_os("HOME")?).join(fallback)))
        .map(|base| base.join(APP_ID))
}

/// The application's icon from the icon theme, the smallest at least `pixels` square.
pub fn app_icon(pixels: u32) -> Option<draw::RasterImage> {
    let (side, rgba) = icon_pixels(pixels)?;
    draw::RasterImage::new([side; 2], rgba).ok()
}

/// Looks beside the executable first, as the release archive installs it, then in the
/// XDG data directories.
fn icon_pixels(pixels: u32) -> Option<(u32, Vec<u8>)> {
    let home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")));
    let system = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|dirs| !dirs.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    let roots: Vec<PathBuf> = std::env::current_exe()
        .ok()
        .and_then(|exe| Some(exe.parent()?.parent()?.join("share")))
        .into_iter()
        .chain(home)
        .chain(std::env::split_paths(&system))
        .collect();
    let sizes = [16, 24, 32, 48, 64, 128, 256, 512];
    let ordered = sizes
        .iter()
        .filter(|&&side| side >= pixels)
        .chain(sizes.iter().rev().filter(|&&side| side < pixels));
    ordered
        .flat_map(|side| {
            roots.iter().map(move |root| {
                root.join(format!("icons/hicolor/{side}x{side}/apps/{APP_ID}.png"))
            })
        })
        .find_map(|path| decode_rgba(&path))
}

fn decode_rgba(path: &std::path::Path) -> Option<(u32, Vec<u8>)> {
    let file = std::io::BufReader::new(std::fs::File::open(path).ok()?);
    let mut decoder = png::Decoder::new(file);
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().ok()?;
    let mut pixels = vec![0; reader.output_buffer_size()?];
    let frame = reader.next_frame(&mut pixels).ok()?;
    pixels.truncate(frame.buffer_size());
    let rgba = match frame.color_type {
        png::ColorType::Rgba => pixels,
        png::ColorType::Rgb => pixels
            .chunks_exact(3)
            .flat_map(|pixel| [pixel[0], pixel[1], pixel[2], 255])
            .collect(),
        _ => return None,
    };
    (frame.width == frame.height).then_some((frame.width, rgba))
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
        ["--getopenfilename", ".", "*.onetoc2 *.one", "--title", title],
    );
    asked
        .unwrap_or_else(|error| {
            eprintln!("{error}");
            None
        })
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
}

/// Asks where to create something named `name` by default.
pub fn pick_new(title: &str, name: &str) -> Option<PathBuf> {
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
    let (title, text) = (message.to_owned(), detail.to_owned());
    std::thread::spawn(move || {
        let shown = Command::new("zenity")
            .args([
                "--warning",
                &format!("--title={title}"),
                &format!("--text={text}"),
            ])
            .status()
            .or_else(|_| {
                Command::new("kdialog")
                    .args(["--sorry", &text, "--title", &title])
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
}
