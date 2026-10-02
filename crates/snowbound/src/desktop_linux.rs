//! The single Linux executable's place on the desktop. Wayland shells without
//! xdg-toplevel-icon (GNOME) find a window's icon only through a desktop entry named after its
//! app ID, so a run that isn't installed writes a hidden one and removes it on exit; Install
//! adds Snowbound to the app menu under the same name, and Uninstall takes back what it wrote.

use std::{
    ffi::c_void,
    fs,
    io::{self, Write},
    os::fd::{AsFd, FromRawFd},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    ptr::NonNull,
    sync::Mutex,
};
use wayland_client::{
    Connection, Dispatch, EventQueue, Proxy,
    backend::{Backend, ObjectId},
    globals::{GlobalList, GlobalListContents, registry_queue_init},
    protocol::{wl_buffer, wl_registry, wl_shm, wl_shm_pool, wl_surface::WlSurface},
};
use wayland_protocols::xdg::{
    activation::v1::client::xdg_activation_v1::XdgActivationV1,
    shell::client::xdg_toplevel::XdgToplevel,
    toplevel_icon::v1::client::{
        xdg_toplevel_icon_manager_v1::XdgToplevelIconManagerV1,
        xdg_toplevel_icon_v1::XdgToplevelIconV1,
    },
};
use winit::{
    event_loop::EventLoop,
    platform::wayland::{EventLoopExtWayland, WindowExtWayland},
    raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle},
    window::{Icon, Window},
};

/// Wayland's app ID and X11's WM_CLASS, which name the desktop entry and the icon.
pub const APP_ID: &str = "net.paperclover.snowbound";
const TEMPLATE: &str = include_str!("../linux/snowbound.desktop");
/// The type of `.one`, `.onetoc2` and `.onepkg` files, which no shared-mime-info release names.
const MIME_TYPES: &str = include_str!("../linux/onenote.xml");
/// 512 pixels square; the smaller sizes are scaled from it.
const ICON: &[u8] = include_bytes!("../linux/snowbound.png");
const SIZES: [u32; 8] = [16, 24, 32, 48, 64, 128, 256, 512];
/// Marks the hidden entry of a run that isn't installed, holding its process ID.
const PORTABLE: &str = "X-Snowbound-Portable";

#[derive(Clone, Copy, PartialEq)]
enum Installed {
    No,
    /// By Install, into the user's data folder.
    Here,
    /// By a package, into a system data folder.
    System,
}

static INSTALLED: Mutex<Installed> = Mutex::new(Installed::No);

fn installed() -> Installed {
    *INSTALLED.lock().unwrap()
}

/// Whether the welcome offers Install.
pub fn installable() -> bool {
    installed() == Installed::No
}

/// Whether Options offers Uninstall.
pub fn uninstallable() -> bool {
    installed() == Installed::Here
}

fn data_home() -> Option<PathBuf> {
    crate::platform::xdg_base("XDG_DATA_HOME", ".local/share")
}

fn entry(data: &Path) -> PathBuf {
    data.join(format!("applications/{APP_ID}.desktop"))
}

fn mime_types(data: &Path) -> PathBuf {
    data.join(format!("mime/packages/{APP_ID}.xml"))
}

fn theme_icon(data: &Path, side: u32) -> PathBuf {
    data.join(format!("icons/hicolor/{side}x{side}/apps/{APP_ID}.png"))
}

/// Where Install copies the executable.
pub fn binary() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("HOME")?).join(".local/bin/snowbound"))
}

/// The hidden entry's icon, in the session's runtime folder, which logging out empties.
fn runtime_icon() -> Option<PathBuf> {
    let runtime = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?);
    Some(runtime.join(format!("{APP_ID}.png")))
}

/// The process that wrote the hidden entry `text`; None for any other entry.
fn portable_owner(text: &str) -> Option<i32> {
    text.lines()
        .find_map(|line| line.strip_prefix(PORTABLE)?.strip_prefix('='))
        .and_then(|pid| pid.trim().parse().ok())
}

/// Before the window opens: notes whether Snowbound is installed, removes the hidden entry a
/// run that ended without cleaning up left, and writes this run's where the shell needs it.
pub fn prepare(event_loop: &EventLoop<crate::UserEvent>) {
    let Some(data) = data_home() else {
        return;
    };
    let path = entry(&data);
    let owner = fs::read_to_string(&path)
        .ok()
        .map(|text| portable_owner(&text));
    let system = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|dirs| !dirs.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    let packaged = std::env::split_paths(&system).any(|root| entry(&root).exists());
    *INSTALLED.lock().unwrap() = match owner {
        Some(None) => Installed::Here,
        _ if packaged => Installed::System,
        _ => Installed::No,
    };
    let needed = installed() == Installed::No
        && event_loop.is_wayland()
        && !display(event_loop)
            .and_then(toplevel_icons)
            .unwrap_or(false);
    if needed {
        if let Err(error) = write_portable(&path) {
            eprintln!("Cannot write {}: {error}", path.display());
        }
    } else if let Some(Some(pid)) = owner
        && unsafe { libc::kill(pid, 0) } != 0
    {
        remove_portable(pid);
    }
}

fn write_portable(path: &Path) -> io::Result<()> {
    let icon = runtime_icon().ok_or(io::ErrorKind::NotFound)?;
    fs::write(&icon, ICON)?;
    let text = entry_text(&crate::loader::executable()?, &icon.to_string_lossy())
        + &format!("NoDisplay=true\n{PORTABLE}={}\n", std::process::id());
    write_entry(path, &text)?;
    extern "C" fn exiting() {
        remove_portable(std::process::id() as i32);
    }
    unsafe { libc::atexit(exiting) };
    Ok(())
}

/// Removes the hidden entry `pid` wrote, leaving any other.
fn remove_portable(pid: i32) {
    let Some(path) = data_home().map(|data| entry(&data)) else {
        return;
    };
    let text = fs::read_to_string(&path).unwrap_or_default();
    if portable_owner(&text) == Some(pid) {
        let _ = fs::remove_file(path);
        let _ = runtime_icon().map(fs::remove_file);
    }
}

/// The template launching `exec` and showing `icon`, a theme name or an absolute path.
fn entry_text(exec: &Path, icon: &str) -> String {
    TEMPLATE
        .lines()
        .map(|line| match line.split_once('=') {
            Some(("Exec", command)) => {
                let arguments = command
                    .split_once(' ')
                    .map_or("", |(_, arguments)| arguments);
                format!("Exec={} {arguments}\n", quote(exec))
            }
            Some(("Icon", _)) => format!("Icon={icon}\n"),
            _ => format!("{line}\n"),
        })
        .collect()
}

/// `path` as an Exec argument: quoted, with the quoting's escapes escaped again as the
/// entry's strings require, and field codes' `%` doubled.
fn quote(path: &Path) -> String {
    let mut quoted = String::from("\"");
    for character in path.to_string_lossy().chars() {
        match character {
            '"' | '`' | '$' => quoted.push_str("\\\\"),
            '\\' => quoted.push_str("\\\\\\"),
            '%' => quoted.push('%'),
            _ => {}
        }
        quoted.push(character);
    }
    quoted + "\""
}

/// Writes whole, so the shell watching the folder never reads half an entry.
fn write_entry(path: &Path, text: &str) -> io::Result<()> {
    fs::create_dir_all(path.parent().ok_or(io::ErrorKind::NotFound)?)?;
    let partial = path.with_extension("partial");
    fs::write(&partial, text)?;
    fs::rename(partial, path)
}

fn pixels() -> image::RgbaImage {
    let mut decoder = png::Decoder::new(io::Cursor::new(ICON));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().expect("The icon is a PNG");
    let mut rgba = vec![
        0;
        reader
            .output_buffer_size()
            .expect("The icon fits in memory")
    ];
    let frame = reader.next_frame(&mut rgba).expect("The icon is a PNG");
    image::RgbaImage::from_raw(frame.width, frame.height, rgba).expect("The icon is RGBA")
}

fn scaled(icon: &image::RgbaImage, side: u32) -> image::RgbaImage {
    image::imageops::resize(icon, side, side, image::imageops::FilterType::Lanczos3)
}

/// For X11, which takes the icon from the window.
pub fn window_icon() -> Option<Icon> {
    let icon = scaled(&pixels(), 128);
    Icon::from_rgba(icon.into_raw(), 128, 128).ok()
}

struct Wayland;

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Wayland {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &wayland_client::QueueHandle<Self>,
    ) {
    }
}

wayland_client::delegate_noop!(Wayland: ignore wl_shm::WlShm);
wayland_client::delegate_noop!(Wayland: wl_shm_pool::WlShmPool);
wayland_client::delegate_noop!(Wayland: ignore wl_buffer::WlBuffer);
wayland_client::delegate_noop!(Wayland: ignore XdgToplevelIconManagerV1);
wayland_client::delegate_noop!(Wayland: XdgToplevelIconV1);
wayland_client::delegate_noop!(Wayland: XdgActivationV1);

fn display(handle: &impl HasDisplayHandle) -> Option<NonNull<c_void>> {
    match handle.display_handle().ok()?.as_raw() {
        RawDisplayHandle::Wayland(wayland) => Some(wayland.display),
        _ => None,
    }
}

/// A queue of this process's own on winit's Wayland connection.
fn connect(display: NonNull<c_void>) -> Option<(Connection, GlobalList, EventQueue<Wayland>)> {
    // Safety: winit's display outlives the window and event loop that lend it.
    let backend = unsafe { Backend::from_foreign_display(display.as_ptr().cast()) };
    let connection = Connection::from_backend(backend);
    let (globals, queue) = registry_queue_init(&connection).ok()?;
    Some((connection, globals, queue))
}

fn toplevel_icons(display: NonNull<c_void>) -> Option<bool> {
    let (_, globals, _) = connect(display)?;
    let name = XdgToplevelIconManagerV1::interface().name;
    Some(
        globals
            .contents()
            .with_list(|list| list.iter().any(|global| global.interface == name)),
    )
}

/// Gives the window its icon where the compositor takes one through xdg-toplevel-icon, as KWin
/// does.
pub fn set_toplevel_icon(window: &Window) -> Option<()> {
    let toplevel = window.xdg_toplevel()?;
    let (connection, globals, mut queue) = connect(display(window)?)?;
    let handle = queue.handle();
    let manager: XdgToplevelIconManagerV1 = globals.bind(&handle, 1..=1, ()).ok()?;
    let shm: wl_shm::WlShm = globals.bind(&handle, 1..=1, ()).ok()?;
    // Safety: winit's xdg_toplevel lives as long as the window.
    let id = unsafe { ObjectId::from_ptr(XdgToplevel::interface(), toplevel.as_ptr().cast()) };
    let toplevel = XdgToplevel::from_id(&connection, id.ok()?).ok()?;
    let icon = pixels();
    // wl_shm's ARGB8888 is premultiplied, little-endian BGRA.
    let sides = &SIZES[..SIZES.len() - 1];
    let mut bytes = Vec::new();
    for &side in sides {
        for pixel in scaled(&icon, side).pixels() {
            let [red, green, blue, alpha] = pixel.0;
            let premultiply = |channel: u8| (channel as u32 * alpha as u32 / 255) as u8;
            bytes.extend([
                premultiply(blue),
                premultiply(green),
                premultiply(red),
                alpha,
            ]);
        }
    }
    // Safety: memfd_create returns a new descriptor this File then owns. A system call, as
    // glibc wraps it only from 2.27.
    let fd = unsafe {
        libc::syscall(
            libc::SYS_memfd_create,
            c"snowbound-icon".as_ptr(),
            libc::MFD_CLOEXEC,
        )
    } as i32;
    if fd < 0 {
        return None;
    }
    let mut file = unsafe { fs::File::from_raw_fd(fd) };
    file.write_all(&bytes).ok()?;
    let pool = shm.create_pool(file.as_fd(), bytes.len() as i32, &handle, ());
    let icon = manager.create_icon(&handle, ());
    let mut offset = 0;
    let buffers: Vec<_> = sides
        .iter()
        .map(|&side| {
            let stride = side as i32 * 4;
            let buffer = pool.create_buffer(
                offset,
                side as i32,
                side as i32,
                stride,
                wl_shm::Format::Argb8888,
                &handle,
                (),
            );
            offset += stride * side as i32;
            icon.add_buffer(&buffer, 1);
            buffer
        })
        .collect();
    manager.set_icon(&toplevel, Some(&icon));
    // The toplevel keeps its icon; the buffers must outlive the icon object.
    icon.destroy();
    for buffer in buffers {
        buffer.destroy();
    }
    pool.destroy();
    manager.destroy();
    queue.roundtrip(&mut Wayland).ok()?;
    Some(())
}

/// The token the launcher gave this launch for its window to take the focus with: an
/// xdg-activation token on Wayland, a startup notification ID on X11.
pub fn activation_token() -> Option<String> {
    std::env::var("XDG_ACTIVATION_TOKEN")
        .or_else(|_| std::env::var("DESKTOP_STARTUP_ID"))
        .ok()
        .filter(|token| !token.is_empty())
}

/// Brings the window forward with the token a later launch handed over.
pub fn activate(window: &Window, token: &str) {
    match window.window_handle().map(|handle| handle.as_raw()) {
        Ok(RawWindowHandle::Wayland(handle)) => {
            activate_wayland(window, handle.surface, token);
        }
        Ok(RawWindowHandle::Xlib(handle)) => {
            if let Err(error) = activate_x11(handle.window as u32, token) {
                eprintln!("Cannot activate the window: {error}");
            }
        }
        _ => {}
    }
}

fn activate_wayland(window: &Window, surface: NonNull<c_void>, token: &str) -> Option<()> {
    let (connection, globals, mut queue) = connect(display(window)?)?;
    let activation: XdgActivationV1 = globals.bind(&queue.handle(), 1..=1, ()).ok()?;
    // Safety: winit's surface lives as long as the window.
    let id = unsafe { ObjectId::from_ptr(WlSurface::interface(), surface.as_ptr().cast()) };
    let surface = WlSurface::from_id(&connection, id.ok()?).ok()?;
    activation.activate(token.into(), &surface);
    activation.destroy();
    queue.roundtrip(&mut Wayland).ok()?;
    Some(())
}

/// Asks for the focus at the launch's time, which the window manager weighs against the
/// user's latest input, then ends the launch's startup notification.
fn activate_x11(window: u32, token: &str) -> Result<(), Box<dyn std::error::Error>> {
    use x11rb::{
        connection::Connection as _,
        protocol::xproto::{ClientMessageEvent, ConnectionExt as _, EventMask},
    };
    let (connection, screen) = x11rb::connect(None)?;
    let root = connection.setup().roots[screen].root;
    let atom = |name: &str| -> Result<u32, Box<dyn std::error::Error>> {
        Ok(connection
            .intern_atom(false, name.as_bytes())?
            .reply()?
            .atom)
    };
    let time = token
        .rsplit_once("_TIME")
        .and_then(|(_, time)| time.parse().ok())
        .unwrap_or(x11rb::CURRENT_TIME);
    let active =
        ClientMessageEvent::new(32, window, atom("_NET_ACTIVE_WINDOW")?, [1, time, 0, 0, 0]);
    let mask = EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY;
    connection.send_event(false, root, mask, active)?;
    let quoted = token.replace('\\', "\\\\").replace('"', "\\\"");
    let mut kind = atom("_NET_STARTUP_INFO_BEGIN")?;
    let more = atom("_NET_STARTUP_INFO")?;
    for chunk in format!("remove: ID=\"{quoted}\"\0").as_bytes().chunks(20) {
        let mut data = [0; 20];
        data[..chunk.len()].copy_from_slice(chunk);
        let message = ClientMessageEvent::new(8, window, kind, data);
        connection.send_event(false, root, EventMask::PROPERTY_CHANGE, message)?;
        kind = more;
    }
    connection.flush()?;
    Ok(())
}

/// Copies this executable to `~/.local/bin` unless it runs from there, adds Snowbound to the
/// app menu and to the apps opening OneNote's files, and says where Uninstall is.
pub fn install() {
    match try_install() {
        Ok(()) => {
            *INSTALLED.lock().unwrap() = Installed::Here;
            crate::platform::inform(
                "Snowbound is in your app menu",
                "To uninstall it, open Options from the notebook menu. Uninstall is under Updates.",
            );
        }
        Err(error) => crate::platform::alert("Couldn't install Snowbound", &error.to_string()),
    }
}

fn try_install() -> io::Result<()> {
    let (data, binary) = data_home().zip(binary()).ok_or(io::ErrorKind::NotFound)?;
    let running = crate::loader::executable()?;
    if fs::canonicalize(&running)? != fs::canonicalize(&binary).unwrap_or_default() {
        fs::create_dir_all(binary.parent().ok_or(io::ErrorKind::NotFound)?)?;
        let partial = binary.with_extension("partial");
        fs::copy(&running, &partial)?;
        fs::rename(partial, &binary)?;
    }
    let icon = pixels();
    for side in SIZES {
        let path = theme_icon(&data, side);
        fs::create_dir_all(path.parent().ok_or(io::ErrorKind::NotFound)?)?;
        if side == icon.width() {
            fs::write(path, ICON)?;
            continue;
        }
        let mut encoder = png::Encoder::new(fs::File::create(path)?, side, side);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()?
            .write_image_data(&scaled(&icon, side))?;
    }
    let types = mime_types(&data);
    fs::create_dir_all(types.parent().ok_or(io::ErrorKind::NotFound)?)?;
    fs::write(types, MIME_TYPES)?;
    // Replaces this run's hidden entry, which exiting then leaves alone.
    write_entry(&entry(&data), &entry_text(&binary, APP_ID))?;
    let _ = runtime_icon().map(fs::remove_file);
    refresh_caches(&data);
    Ok(())
}

/// After confirming, removes what Install wrote; notebooks stay where they are.
pub fn uninstall(proxy: &winit::event_loop::EventLoopProxy<crate::UserEvent>) {
    let Some(binary) = binary() else {
        return;
    };
    let detail = format!(
        "Snowbound leaves the app menu and {} is deleted. Your notebooks stay where they are.",
        binary.display()
    );
    let reply = crate::Reply::new(proxy, |_, ()| {
        remove();
        Ok(())
    });
    crate::platform::confirm(
        "Uninstall Snowbound?",
        &detail,
        "Cancel",
        "Uninstall",
        reply,
    );
}

fn remove() {
    let (Some(data), Some(binary)) = (data_home(), binary()) else {
        return;
    };
    let mut failed = None;
    let icons = SIZES.map(|side| theme_icon(&data, side));
    if let Some(staged) = crate::update::staging(&binary)
        && let Err(error) = fs::remove_dir_all(staged)
        && error.kind() != io::ErrorKind::NotFound
    {
        failed.get_or_insert(error);
    }
    for path in [entry(&data), mime_types(&data), binary]
        .iter()
        .chain(&icons)
    {
        if let Err(error) = fs::remove_file(path)
            && error.kind() != io::ErrorKind::NotFound
        {
            failed.get_or_insert(error);
        }
    }
    // Install made the size folders; each goes once empty, up to hicolor.
    for icon in &icons {
        for folder in icon.ancestors().skip(1).take(3) {
            let _ = fs::remove_dir(folder);
        }
    }
    refresh_caches(&data);
    match failed {
        Some(error) => crate::platform::alert("Couldn't uninstall Snowbound", &error.to_string()),
        None => *INSTALLED.lock().unwrap() = Installed::No,
    }
}

/// Brings the icon theme's cache, the file types and the apps opening them up to date, where
/// the desktop has the tools.
fn refresh_caches(data: &Path) {
    for (tool, folder) in [
        ("gtk-update-icon-cache", "icons/hicolor"),
        ("update-mime-database", "mime"),
        ("update-desktop-database", "applications"),
    ] {
        let _ = Command::new(tool)
            .arg(data.join(folder))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exec_paths_are_quoted_then_escaped_for_the_entry() {
        assert_eq!(
            quote(Path::new("/home/a b/snowbound")),
            r#""/home/a b/snowbound""#
        );
        assert_eq!(quote(Path::new(r#"/x/$"`\%y"#)), r#""/x/\\$\\"\\`\\\\%%y""#);
    }

    #[test]
    fn only_the_hidden_entry_names_an_owner() {
        let installed = entry_text(Path::new("/bin/snowbound"), APP_ID);
        assert!(installed.contains("\nExec=\"/bin/snowbound\" %F\n"));
        assert!(installed.contains(&format!("\nIcon={APP_ID}\n")));
        assert_eq!(portable_owner(&installed), None);
        let hidden = installed + &format!("NoDisplay=true\n{PORTABLE}=42\n");
        assert_eq!(portable_owner(&hidden), Some(42));
    }

    #[test]
    fn the_icon_is_square_and_as_large_as_the_largest_size() {
        let icon = pixels();
        assert_eq!([icon.width(), icon.height()], [SIZES[7]; 2]);
    }
}
