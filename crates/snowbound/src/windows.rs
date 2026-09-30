//! Windows 7 to 11. Where the desktop composes windows, the toolbar's row is the title bar:
//! over Aero glass on 7 and Mica on 11, beside the system's own caption buttons, and on 8
//! and 10 with caption buttons drawn as theirs, over 10's acrylic. Dialogs are the
//! system's, passwords live in the Credential Manager, and dates follow the user's locale.
//! Anything newer than Windows 7 is looked up at run time, so one executable starts on all
//! of them.

use canvas::date::DateField;
use std::{
    ffi::c_void,
    path::PathBuf,
    sync::{
        OnceLock,
        atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering},
    },
    time::Duration,
};
use ui::{Flags, Spec, Ui, px};
use windows_sys::Win32::{
    Foundation::{FILETIME, HWND, LPARAM, LRESULT, POINT, RECT, SYSTEMTIME, WPARAM},
    Graphics::{Dwm, Gdi},
    System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW},
    UI::{Controls::MARGINS, WindowsAndMessaging as wm},
};
use windows_sys::core::BOOL;
use winit::{
    error::EventLoopError,
    event_loop::{EventLoop, EventLoopProxy},
    platform::windows::{IconExtWindows, WindowAttributesExtWindows},
    raw_window_handle::{HasWindowHandle, RawWindowHandle},
    window::{Icon, Theme, Window, WindowAttributes},
};

/// The title bar's leading margin.
pub const LEADING: f32 = 8.0;
/// The margin past the caption buttons, which the row's last gap already makes; the
/// buttons slide over it to the window's corner.
pub const TRAILING: f32 = GAP;
const GAP: f32 = 6.0;
/// A drawn caption button's width, as Windows 10 draws its own.
const CAPTION_BUTTON: f32 = 46.0;
/// How far inside the window's edges a press resizes it, at 96 dpi.
const EDGE: f32 = 8.0;

static QUIT: OnceLock<EventLoopProxy<crate::UserEvent>> = OnceLock::new();
/// The window, which dialogs belong to.
static WINDOW: AtomicIsize = AtomicIsize::new(0);
/// The window procedure the frame's own passes the rest to.
static PREVIOUS: AtomicIsize = AtomicIsize::new(0);
/// Whether the desktop composes windows, which Windows 7's basic and classic themes don't.
static COMPOSED: AtomicBool = AtomicBool::new(false);
/// The window's scale factor, as `f32` bits, for hit-testing outside a frame.
static SCALE: AtomicU32 = AtomicU32::new(0x3f80_0000);
/// The caption button under the pointer, by its hit-test code: as the system reports it
/// over Mica, where the system runs them, and as the row saw it for the close button,
/// whose glyph turns white on red.
static CAPTION_HOVERED: AtomicU32 = AtomicU32::new(0);

/// The Windows release: its major and minor version and its build.
fn version() -> (u32, u32, u32) {
    static VERSION: OnceLock<(u32, u32, u32)> = OnceLock::new();
    *VERSION.get_or_init(|| {
        // GetVersionEx answers as the manifest allows; ntdll's RtlGetVersion never lies.
        #[repr(C)]
        struct Info {
            size: u32,
            major: u32,
            minor: u32,
            build: u32,
            platform: u32,
            service_pack: [u16; 128],
        }
        let mut info = Info {
            size: size_of::<Info>() as u32,
            major: 6,
            minor: 1,
            build: 7601,
            platform: 0,
            service_pack: [0; 128],
        };
        if let Some(get) =
            function::<unsafe extern "system" fn(*mut Info) -> i32>("ntdll.dll", c"RtlGetVersion")
        {
            unsafe { get(&mut info) };
        }
        (info.major, info.minor, info.build)
    })
}

/// Windows 11, whose windows have rounded corners and Mica.
fn eleven() -> bool {
    version().0 >= 10 && version().2 >= 22000
}

/// Who draws the caption buttons.
#[derive(Clone, Copy, PartialEq)]
enum Caption {
    /// The system, in its own title bar: Windows 7 while the desktop doesn't compose.
    System,
    /// The system, over the glass or Mica the row shows through: Windows 7 composing, 11.
    Glass,
    /// The row, as the system draws them: Windows 8 and 10, whose frames are opaque.
    Drawn,
}

fn caption() -> Caption {
    if eleven() || (version() < (6, 2, 0) && COMPOSED.load(Ordering::Relaxed)) {
        Caption::Glass
    } else if version() < (6, 2, 0) {
        Caption::System
    } else {
        Caption::Drawn
    }
}

/// Function `name` of `library`, of signature `F`, where this Windows has it.
fn function<F: Copy>(library: &str, name: &std::ffi::CStr) -> Option<F> {
    let library = wide(library);
    unsafe {
        let mut module = GetModuleHandleW(library.as_ptr());
        if module.is_null() {
            module = LoadLibraryW(library.as_ptr());
        }
        let address = GetProcAddress(module, name.as_ptr().cast())?;
        Some(std::mem::transmute_copy(&address))
    }
}

/// `text` as a NUL-terminated UTF-16 string.
fn wide(text: impl AsRef<std::ffi::OsStr>) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    text.as_ref().encode_wide().chain([0]).collect()
}

/// The UTF-16 string at `text` up to its first NUL.
fn narrow(text: &[u16]) -> String {
    let end = text
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(text.len());
    String::from_utf16_lossy(&text[..end])
}

fn hwnd(window: &Window) -> HWND {
    match window.window_handle().expect("Live window").as_raw() {
        RawWindowHandle::Win32(handle) => handle.hwnd.get() as HWND,
        _ => unreachable!(),
    }
}

/// The window dialogs belong to, once there is one.
fn owner() -> HWND {
    WINDOW.load(Ordering::Relaxed) as HWND
}

fn composed() -> bool {
    let mut on: BOOL = 0;
    unsafe { Dwm::DwmIsCompositionEnabled(&mut on) >= 0 && on != 0 }
}

pub fn event_loop(_headless: bool) -> Result<EventLoop<crate::UserEvent>, EventLoopError> {
    let event_loop = EventLoop::with_user_event().build()?;
    QUIT.set(event_loop.create_proxy())
        .expect("Only one application event loop is created");
    COMPOSED.store(composed(), Ordering::Relaxed);
    Ok(event_loop)
}

/// The executable's icon, and where the row draws the caption buttons, no system frame:
/// the desktop draws the window's shadow.
pub fn window_attributes() -> WindowAttributes {
    let icon = |size| Icon::from_resource(1, Some(winit::dpi::PhysicalSize::new(size, size)));
    Window::default_attributes()
        .with_window_icon(icon(16).ok())
        .with_taskbar_icon(icon(32).ok())
        .with_decorations(caption() != Caption::Drawn)
        .with_undecorated_shadow(true)
}

/// Takes the window's frame messages ahead of winit's: over glass or Mica, the caption
/// leaves the non-client area for the row, beside the system's caption buttons; with drawn
/// buttons, the window's edges resize it.
pub fn install_title_bar(window: &Window) {
    let hwnd = hwnd(window);
    WINDOW.store(hwnd as isize, Ordering::Relaxed);
    SCALE.store((window.scale_factor() as f32).to_bits(), Ordering::Relaxed);
    let procedure = frame_procedure as *const () as isize;
    PREVIOUS.store(
        unsafe { wm::SetWindowLongPtrW(hwnd, wm::GWLP_WNDPROC, procedure) },
        Ordering::Relaxed,
    );
    reframe(hwnd);
}

/// Has the system measure the window's frame again.
fn reframe(hwnd: HWND) {
    let flags = wm::SWP_FRAMECHANGED | wm::SWP_NOMOVE | wm::SWP_NOSIZE | wm::SWP_NOZORDER;
    unsafe { wm::SetWindowPos(hwnd, std::ptr::null_mut(), 0, 0, 0, 0, flags) };
}

fn set_attribute<T>(hwnd: HWND, attribute: u32, value: &T) -> bool {
    unsafe {
        Dwm::DwmSetWindowAttribute(
            hwnd,
            attribute,
            (value as *const T).cast(),
            size_of::<T>() as u32,
        ) >= 0
    }
}

/// The system's frame under the whole window, which the row and the rest of the chrome
/// show through.
fn extend_frame(hwnd: HWND) {
    let sheet = MARGINS {
        cxLeftWidth: -1,
        cxRightWidth: -1,
        cyTopHeight: -1,
        cyBottomHeight: -1,
    };
    unsafe { Dwm::DwmExtendFrameIntoClientArea(hwnd, &sheet) };
}

unsafe extern "system" fn frame_procedure(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let previous = |message, wparam, lparam| unsafe {
        let procedure: wm::WNDPROC = std::mem::transmute(PREVIOUS.load(Ordering::Relaxed));
        wm::CallWindowProcW(procedure, hwnd, message, wparam, lparam)
    };
    match message {
        // Accent colours and transparency effects reach the window only as settings.
        wm::WM_SETTINGCHANGE | wm::WM_DWMCOLORIZATIONCOLORCHANGED => {
            if let Some(proxy) = QUIT.get() {
                let _ = proxy.send_event(crate::UserEvent::Appearance);
            }
        }
        wm::WM_DWMCOMPOSITIONCHANGED => {
            COMPOSED.store(composed(), Ordering::Relaxed);
            reframe(hwnd);
        }
        _ => {}
    }
    let result = match caption() {
        Caption::System => None,
        Caption::Glass => glass_frame(hwnd, message, wparam, lparam, &previous),
        Caption::Drawn => drawn_frame(hwnd, message, wparam, lparam, &previous),
    };
    result.unwrap_or_else(|| previous(message, wparam, lparam))
}

type Procedure<'a> = &'a dyn Fn(u32, WPARAM, LPARAM) -> LRESULT;

/// The frame over glass or Mica: the system's frame keeps its sides and bottom, the caption
/// goes, and the system draws and hit-tests its caption buttons over the row.
fn glass_frame(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    previous: Procedure<'_>,
) -> Option<LRESULT> {
    if eleven() {
        light_caption(hwnd, message, wparam);
    }
    let mut result = 0;
    if unsafe { Dwm::DwmDefWindowProc(hwnd, message, wparam, lparam, &mut result) } != 0 {
        return Some(result);
    }
    let maximized = unsafe { wm::IsZoomed(hwnd) } != 0;
    match message {
        wm::WM_NCCALCSIZE if wparam != 0 => {
            let params = unsafe { &mut *(lparam as *mut wm::NCCALCSIZE_PARAMS) };
            let top = params.rgrc[0].top;
            let result = previous(message, wparam, lparam);
            // A maximized window's frame lies past the screen's edge.
            params.rgrc[0].top = top + if maximized { frame_size() } else { 0 };
            Some(result)
        }
        wm::WM_NCHITTEST => {
            let hit = previous(message, wparam, lparam);
            let top = client_point(hwnd, lparam).is_some_and(|point| point.y < frame_size());
            Some(if hit == wm::HTCLIENT as LRESULT && top && !maximized {
                wm::HTTOP as LRESULT
            } else {
                hit
            })
        }
        wm::WM_ACTIVATE | wm::WM_DWMCOMPOSITIONCHANGED => {
            extend_frame(hwnd);
            None
        }
        _ => None,
    }
}

/// Follows the pointer over the system's caption buttons, which the row draws on Windows 11.
fn light_caption(hwnd: HWND, message: u32, wparam: WPARAM) {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse as input;
    let over = match message {
        wm::WM_NCMOUSEMOVE => match wparam as u32 {
            hit @ (wm::HTMINBUTTON | wm::HTMAXBUTTON | wm::HTCLOSE) => hit,
            _ => 0,
        },
        wm::WM_NCMOUSELEAVE => 0,
        _ => return,
    };
    if CAPTION_HOVERED.swap(over, Ordering::Relaxed) == over {
        return;
    }
    if over != 0 {
        let mut track = input::TRACKMOUSEEVENT {
            cbSize: size_of::<input::TRACKMOUSEEVENT>() as u32,
            dwFlags: input::TME_LEAVE | input::TME_NONCLIENT,
            hwndTrack: hwnd,
            dwHoverTime: 0,
        };
        unsafe { input::TrackMouseEvent(&mut track) };
    }
    unsafe { Gdi::InvalidateRect(hwnd, std::ptr::null(), 0) };
}

/// The system frame's thickness at the top, where the caption went.
fn frame_size() -> i32 {
    unsafe { wm::GetSystemMetrics(wm::SM_CYFRAME) + wm::GetSystemMetrics(wm::SM_CXPADDEDBORDER) }
}

/// The screen point in a hit-test's `lparam` in the window's client coordinates.
fn client_point(hwnd: HWND, lparam: LPARAM) -> Option<POINT> {
    let mut point = POINT {
        x: (lparam & 0xffff) as i16 as i32,
        y: ((lparam >> 16) & 0xffff) as i16 as i32,
    };
    (unsafe { Gdi::ScreenToClient(hwnd, &mut point) } != 0).then_some(point)
}

/// The frame with drawn caption buttons, where winit has made the whole window its client
/// area: its edges resize it.
fn drawn_frame(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    previous: Procedure<'_>,
) -> Option<LRESULT> {
    match message {
        // -1 keeps the system from painting a caption the window no longer has.
        wm::WM_NCACTIVATE => Some(previous(message, wparam, -1)),
        wm::WM_NCHITTEST => {
            let point = client_point(hwnd, lparam)?;
            let style = unsafe { wm::GetWindowLongW(hwnd, wm::GWL_STYLE) } as u32;
            // Maximized or full screen, the window has no edges.
            if unsafe { wm::IsZoomed(hwnd) } != 0 || style & wm::WS_CAPTION != wm::WS_CAPTION {
                return None;
            }
            let mut client = RECT::default();
            unsafe { wm::GetClientRect(hwnd, &mut client) };
            let edge = (EDGE * f32::from_bits(SCALE.load(Ordering::Relaxed))).round() as i32;
            let hit = match [
                point.y < edge,
                point.y >= client.bottom - edge,
                point.x < edge,
                point.x >= client.right - edge,
            ] {
                [true, _, true, _] => wm::HTTOPLEFT,
                [true, _, _, true] => wm::HTTOPRIGHT,
                [_, true, true, _] => wm::HTBOTTOMLEFT,
                [_, true, _, true] => wm::HTBOTTOMRIGHT,
                [true, ..] => wm::HTTOP,
                [_, true, ..] => wm::HTBOTTOM,
                [.., true, _] => wm::HTLEFT,
                [.., true] => wm::HTRIGHT,
                _ => return None,
            };
            Some(hit as LRESULT)
        }
        // Windows 10's acrylic lags a window being dragged or resized; plain blur doesn't.
        wm::WM_ENTERSIZEMOVE if ACRYLIC.load(Ordering::Relaxed) => {
            accent(hwnd, Accent::Blur);
            None
        }
        wm::WM_EXITSIZEMOVE if ACRYLIC.load(Ordering::Relaxed) => {
            accent(hwnd, Accent::Acrylic(ACRYLIC_TINT.load(Ordering::Relaxed)));
            None
        }
        _ => None,
    }
}

/// Where the row is not the title bar: Windows 7 with its basic or classic theme, whose
/// desktop draws no glass and whose title bars the system draws.
pub fn system_titlebar(_: &Window) -> bool {
    caption() == Caption::System
}

/// Windows 11 rounds a window's corners unless it is maximized; the desktop clips them.
pub fn corner_radius(window: &Window) -> f32 {
    if eleven() && !window.is_maximized() && window.fullscreen().is_none() {
        8.0
    } else {
        0.0
    }
}

pub fn cuts_corners() -> bool {
    false
}

/// Whether Windows 10 shows acrylic under the window, which a drag swaps for plain blur,
/// and the tint it shows it with.
static ACRYLIC: AtomicBool = AtomicBool::new(false);
static ACRYLIC_TINT: AtomicU32 = AtomicU32::new(0);

enum Accent {
    Blur,
    /// Acrylic tinted with an `0xAABBGGRR` colour.
    Acrylic(u32),
}

/// Windows 10's accent under the window, through the undocumented
/// SetWindowCompositionAttribute that its own shell uses.
fn accent(hwnd: HWND, accent: Accent) -> bool {
    #[repr(C)]
    struct Policy {
        state: u32,
        flags: u32,
        tint: u32,
        animation: u32,
    }
    #[repr(C)]
    struct Data {
        attribute: u32,
        data: *mut c_void,
        size: usize,
    }
    type Set = unsafe extern "system" fn(HWND, *mut Data) -> BOOL;
    let Some(set) = function::<Set>("user32.dll", c"SetWindowCompositionAttribute") else {
        return false;
    };
    // ACCENT_ENABLE_BLURBEHIND and ACCENT_ENABLE_ACRYLICBLURBEHIND; flag 2 takes the tint.
    let mut policy = match accent {
        Accent::Blur => Policy {
            state: 3,
            flags: 0,
            tint: 0,
            animation: 0,
        },
        Accent::Acrylic(tint) => Policy {
            state: 4,
            flags: 2,
            tint,
            animation: 0,
        },
    };
    // WCA_ACCENT_POLICY.
    let mut data = Data {
        attribute: 19,
        data: (&mut policy as *mut Policy).cast(),
        size: size_of::<Policy>(),
    };
    unsafe { set(hwnd, &mut data) != 0 }
}

/// Whether Personalization's transparency effects are on, as acrylic and Mica need.
fn transparency() -> bool {
    registry_dword(
        r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize",
        "EnableTransparency",
    ) != Some(0)
}

/// Lays the system's material under the window, which shows through the app's transparent
/// pixels: Aero glass on Windows 7 while the desktop composes, acrylic on Windows 10 (blur
/// before its April 2018 update) and Mica on 11. Windows 8 has none.
pub fn install_backdrop(window: &Window) -> bool {
    let hwnd = hwnd(window);
    let (major, _, build) = version();
    match caption() {
        Caption::System => false,
        Caption::Glass if !eleven() => {
            extend_frame(hwnd);
            // Blurring behind the whole window has the desktop take its pixels' alpha.
            let blur = Dwm::DWM_BLURBEHIND {
                dwFlags: Dwm::DWM_BB_ENABLE,
                fEnable: 1,
                hRgnBlur: std::ptr::null_mut(),
                fTransitionOnMaximized: 0,
            };
            unsafe { Dwm::DwmEnableBlurBehindWindow(hwnd, &blur) };
            true
        }
        _ if major < 10 || !transparency() => false,
        Caption::Glass => {
            extend_frame(hwnd);
            if build >= 22621 {
                // DWMWA_SYSTEMBACKDROP_TYPE, DWMSBT_MAINWINDOW.
                set_attribute(hwnd, 38, &2u32)
            } else {
                // Windows 11's first release names Mica by an undocumented attribute.
                set_attribute(hwnd, 1029, &1u32)
            }
        }
        Caption::Drawn if build >= 17134 => {
            // The shell's own acrylic tints: Windows 10's light and dark flyouts.
            let dark = appearance(window) == Theme::Dark;
            let tint = if dark { 0xcc20_2020 } else { 0xccf3_f3f3 };
            ACRYLIC_TINT.store(tint, Ordering::Relaxed);
            let shown = accent(hwnd, Accent::Acrylic(tint));
            ACRYLIC.store(shown, Ordering::Relaxed);
            shown
        }
        Caption::Drawn => accent(hwnd, Accent::Blur),
    }
}

/// The title bar's fills, focused and not, where no material lies under it: Windows 10
/// and 11's, or the accent colour where Personalization shows it on title bars.
pub fn titlebar(appearance: Theme) -> Option<[[f32; 4]; 2]> {
    if version().0 < 10 {
        return None;
    }
    let dwm = r"Software\Microsoft\Windows\DWM";
    if registry_dword(dwm, "ColorPrevalence") == Some(1)
        && let Some(color) = registry_dword(dwm, "AccentColor")
    {
        let [red, green, blue, _] = color.to_le_bytes();
        let inactive = if appearance == Theme::Dark {
            0x2b
        } else {
            0xff
        };
        return Some([
            draw::srgb(red, green, blue),
            draw::srgb(inactive, inactive, inactive),
        ]);
    }
    Some(match appearance {
        Theme::Dark => [draw::srgb(0x20, 0x20, 0x20), draw::srgb(0x2b, 0x2b, 0x2b)],
        Theme::Light => [draw::srgb(0xff, 0xff, 0xff); 2],
    })
}

/// None: the kit's own menus.
pub fn menu(_: Theme) -> Option<ui::Menu> {
    None
}

const MINIMIZE: &[&str] = &[include_str!("../assets/windows/caption-minimize.svg")];
const MAXIMIZE: &[&str] = &[include_str!("../assets/windows/caption-maximize.svg")];
const RESTORE: &[&str] = &[include_str!("../assets/windows/caption-restore.svg")];
const CLOSE: &[&str] = &[include_str!("../assets/windows/caption-close.svg")];

/// The caption buttons at the row's trailing end, flush with the window's corner. Over glass
/// and Mica the system hit-tests and runs its own, which brings Windows 11's snap layouts:
/// 7 draws them over the glass and the row leaves them room, and 11 doesn't draw them over
/// the DirectComposition surface, so the row draws them as 11 does, lit as the system
/// reports the pointer. On 8 and 10 the row draws and runs them as 10 does.
pub fn window_controls(ui: &mut Ui, window: &Window) {
    let scale = window.scale_factor() as f32;
    SCALE.store(scale.to_bits(), Ordering::Relaxed);
    let system = caption() == Caption::Glass;
    let mut bounds = RECT::default();
    // DWMWA_CAPTION_BUTTON_BOUNDS.
    let found = system
        && unsafe {
            Dwm::DwmGetWindowAttribute(
                hwnd(window),
                5,
                (&mut bounds as *mut RECT).cast(),
                size_of::<RECT>() as u32,
            )
        } >= 0;
    let size = if found {
        [bounds.right - bounds.left, bounds.bottom - bounds.top].map(|side| side as f32 / scale)
    } else {
        [3.0 * CAPTION_BUTTON, crate::TITLE]
    };
    let offset = [GAP, -(crate::TITLE - ui::shell::TOOL) / 2.0];
    if system && !eleven() {
        let size = [px(size[0]), px(1.0)];
        ui.leaf(
            "caption",
            Spec {
                size,
                offset: [GAP, 0.0],
                ..Spec::default()
            },
        );
        return;
    }
    ui.open(
        "caption",
        Spec {
            size: [px(size[0]), px(size[1])],
            offset,
            ..Spec::default()
        },
    );
    let text = ui.theme.text;
    // Windows 11's subtle fill and red, and Windows 10's grey and red.
    let (hover, red) = if eleven() {
        (
            [text[0], text[1], text[2], 0.06],
            draw::srgb(0xc4, 0x2b, 0x1c),
        )
    } else {
        (
            [text[0], text[1], text[2], 0.1],
            draw::srgb(0xe8, 0x11, 0x23),
        )
    };
    let lit = CAPTION_HOVERED.load(Ordering::Relaxed);
    let maximized = window.is_maximized();
    let buttons = [
        ("minimize", "Minimize", MINIMIZE, wm::HTMINBUTTON),
        (
            "maximize",
            if maximized { "Restore" } else { "Maximize" },
            if maximized { RESTORE } else { MAXIMIZE },
            wm::HTMAXBUTTON,
        ),
        ("close", "Close", CLOSE, wm::HTCLOSE),
    ];
    for (id, name, icon, hit) in buttons {
        let fill = if hit == wm::HTCLOSE { red } else { hover };
        let spec = Spec {
            size: [px(size[0] / 3.0), px(size[1])],
            icon: Some(icon),
            color: Some(if hit == wm::HTCLOSE && lit == hit {
                [1.0; 4]
            } else {
                text
            }),
            center: true,
            ..Spec::default()
        };
        if system {
            let fill = Some(if lit == hit { fill } else { [0.0; 4] });
            ui.leaf(id, Spec { fill, ..spec });
            continue;
        }
        let response = ui.leaf(
            id,
            Spec {
                flags: Flags::CLICKABLE,
                hover_fill: Some(fill),
                role: Some(accesskit::Role::Button),
                ..spec
            },
        );
        let over = if response.hovered { hit } else { 0 };
        if hit == wm::HTCLOSE && CAPTION_HOVERED.swap(over, Ordering::Relaxed) != over {
            window.request_redraw();
        }
        if let Some(node) = ui.access(ui.id(id)) {
            node.set_label(name);
        }
        if response.clicked {
            match id {
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
    ui.close();
}

/// The frame's edges take resizing presses.
pub fn resize_direction(_: &Window, _: [f32; 2]) -> Option<winit::window::ResizeDirection> {
    None
}

pub fn zoom(window: &Window) {
    window.set_maximized(!window.is_maximized());
}

/// Winit delivers typed text with its key events on Windows.
pub fn install_text_input(_: &Window) {}

pub fn move_cursor() -> winit::window::CursorIcon {
    winit::window::CursorIcon::Move
}

/// No system border lies under the chrome.
pub fn cover_border_line(_: &mut Ui, _: f32) {}

pub fn with_pool<R>(run: impl FnOnce() -> R) -> R {
    run()
}

/// The interface keeps its own font, which fontique finds as Segoe UI.
pub fn system_interface(_: &mut Ui) {}

/// Resizing takes the window's edges.
pub fn resize_grip(_: &mut Ui, _: &Window, _: [f32; 2]) {}

/// Options opens from the sidebar's footer and Ctrl+Comma; OneNote has a ribbon, not a
/// menu bar, and the toolbar and the command palette stand in for it.
pub fn install_menu() {}

pub fn update_menu(_: impl FnOnce() -> Vec<crate::commands::Status>) {}

pub fn update_tag_menu(_: &[canvas::editor::NoteTag]) {}

pub fn clear_marked_text(_: &Window) {}

#[cfg(feature = "wgpu")]
pub fn configure_presentation(_: &wgpu::Surface<'_>) {}

#[cfg(feature = "wgpu")]
pub fn commit_presentation(_: &Window) {}

/// winit reports no pen pressure here, so every stroke keeps its pen's width.
pub fn pen_pressure() -> Option<f32> {
    None
}

pub fn double_click_interval() -> Duration {
    let millis = unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime() };
    Duration::from_millis(u64::from(millis))
}

/// The apps' colour mode from Personalization, which Windows 10 added; light before it.
pub fn appearance(window: &Window) -> Theme {
    window.theme().unwrap_or(Theme::Light)
}

pub struct Clipboard(arboard::Clipboard);

impl Clipboard {
    pub fn new(_: &Window) -> Result<Self, arboard::Error> {
        arboard::Clipboard::new().map(Self)
    }

    pub fn set_text(&mut self, text: String) -> Result<(), arboard::Error> {
        self.0.set_text(text)
    }

    pub fn get_text(&mut self) -> Result<String, arboard::Error> {
        self.0.get_text()
    }
}

/// The DWORD `value` under `key` in the user's registry hive.
fn registry_dword(key: &str, value: &str) -> Option<u32> {
    use windows_sys::Win32::System::Registry;
    let mut data = 0u32;
    let mut size = size_of::<u32>() as u32;
    let result = unsafe {
        Registry::RegGetValueW(
            Registry::HKEY_CURRENT_USER,
            wide(key).as_ptr(),
            wide(value).as_ptr(),
            Registry::RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&mut data as *mut u32).cast(),
            &mut size,
        )
    };
    (result == 0).then_some(data)
}

/// Windows names no document of a window's.
pub fn represent(_: &Window, _: Option<&std::path::Path>) {}

pub const SHOW_FILE: &str = "Show in Explorer";

/// Opens an Explorer window with `file` selected.
pub fn show_file(file: &std::path::Path) {
    use std::os::windows::process::CommandExt;
    let mut argument = std::ffi::OsString::from("/select,\"");
    argument.push(file);
    argument.push("\"");
    if let Err(error) = std::process::Command::new("explorer.exe")
        .raw_arg(argument)
        .spawn()
    {
        eprintln!("Cannot show {}: {error}", file.display());
    }
}

/// Opens `target`, a folder, a file or a link's URL, with the shell's handler for it.
pub fn reveal(target: impl AsRef<std::ffi::OsStr>) {
    let target = target.as_ref();
    let opened = unsafe {
        windows_sys::Win32::UI::Shell::ShellExecuteW(
            owner(),
            wide("open").as_ptr(),
            wide(target).as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            wm::SW_SHOWNORMAL,
        )
    };
    // ShellExecute's success is a pseudo-handle above 32.
    if opened as isize <= 32 {
        eprintln!("Cannot open {}", target.display());
    }
}

/// Opens a copy of an attachment with the application registered for it.
pub fn open_file(path: &std::path::Path) {
    reveal(path);
}

/// Opens Character Map, whose picks the user copies in.
pub fn character_palette() {
    if let Err(error) = std::process::Command::new("charmap.exe").spawn() {
        eprintln!("Cannot open Character Map: {error}");
    }
}

/// No shell icon lookup yet; the page draws a blank page for the file.
pub fn file_icon(_: &std::path::Path) -> Option<Vec<u8>> {
    None
}

/// winit reports drops without a position; the file goes to the caret.
pub fn drop_point(_: &Window) -> Option<[f32; 2]> {
    None
}

pub fn cache_dir() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("Snowbound"))
}

pub fn settings_dir() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("APPDATA")?).join("Snowbound"))
}

/// None: Windows reaches a share by its UNC path with the share modes and byte-range
/// locks OneNote takes, so a notebook there opens as a folder, as OneNote opens it.
/// Snowbound's own client serves Open Notebook from Server….
pub fn smb_mount(_: &std::path::Path) -> Option<crate::library::Mount> {
    None
}

/// The Credential Manager's name for `mount`'s server, as Snowbound keeps it.
fn credential_target(mount: &crate::library::Mount) -> Vec<u16> {
    wide(format!("Snowbound/smb/{}", mount.host()))
}

/// The password the Credential Manager keeps for `mount`'s server, or what the user types
/// into the system's sign-in dialog when it keeps none.
pub fn smb_login(mount: &crate::library::Mount) -> Result<crate::library::Login, String> {
    use windows_sys::Win32::Security::Credentials as cred;
    let target = credential_target(mount);
    let mut found: *mut cred::CREDENTIALW = std::ptr::null_mut();
    if unsafe { cred::CredReadW(target.as_ptr(), cred::CRED_TYPE_GENERIC, 0, &mut found) } != 0 {
        let login = unsafe {
            let found = &*found;
            let secret = std::slice::from_raw_parts(
                found.CredentialBlob.cast::<u16>(),
                found.CredentialBlobSize as usize / 2,
            );
            let user = if found.UserName.is_null() {
                String::new()
            } else {
                let length = (0..).take_while(|&at| *found.UserName.add(at) != 0).count();
                String::from_utf16_lossy(std::slice::from_raw_parts(found.UserName, length))
            };
            (user, String::from_utf16_lossy(secret))
        };
        unsafe { cred::CredFree(found.cast()) };
        let (user, password) = login;
        let user = mount
            .user
            .clone()
            .filter(|user| !user.is_empty())
            .unwrap_or(user);
        return Ok(crate::library::Login {
            user,
            password,
            domain: mount.domain.clone(),
        });
    }
    let caption = wide("Snowbound");
    let message = wide(format!("Sign in to {}", mount.server));
    let info = cred::CREDUI_INFOW {
        cbSize: size_of::<cred::CREDUI_INFOW>() as u32,
        hwndParent: owner(),
        pszMessageText: message.as_ptr(),
        pszCaptionText: caption.as_ptr(),
        hbmBanner: std::ptr::null_mut(),
    };
    let mut user = [0u16; cred::CREDUI_MAX_USERNAME_LENGTH as usize + 1];
    for (slot, unit) in user
        .iter_mut()
        .zip(mount.user.clone().unwrap_or_default().encode_utf16())
    {
        *slot = unit;
    }
    let mut password = [0u16; 257];
    let mut save: BOOL = 0;
    let server = wide(&mount.server);
    let result = unsafe {
        cred::CredUIPromptForCredentialsW(
            &info,
            server.as_ptr(),
            std::ptr::null_mut(),
            0,
            user.as_mut_ptr(),
            user.len() as u32,
            password.as_mut_ptr(),
            password.len() as u32,
            &mut save,
            cred::CREDUI_FLAGS_GENERIC_CREDENTIALS
                | cred::CREDUI_FLAGS_DO_NOT_PERSIST
                | cred::CREDUI_FLAGS_ALWAYS_SHOW_UI,
        )
    };
    let login = crate::library::Login {
        user: narrow(&user),
        password: narrow(&password),
        domain: mount.domain.clone(),
    };
    password.fill(0);
    match result {
        0 => Ok(login),
        _ => Err("Signing in was canceled".to_owned()),
    }
}

/// What the sign-in offers for keeping a password, which the Credential Manager keeps.
pub fn remember_label() -> Option<&'static str> {
    Some("Remember my credentials")
}

/// Keeps `login`'s password for `mount`'s server in the Credential Manager, where
/// `smb_login` looks for it.
pub fn save_login(
    mount: &crate::library::Mount,
    login: &crate::library::Login,
) -> Result<(), String> {
    use windows_sys::Win32::Security::Credentials as cred;
    let target = credential_target(mount);
    let mut user = wide(&login.user);
    let secret: Vec<u16> = login.password.encode_utf16().collect();
    let credential = cred::CREDENTIALW {
        Flags: 0,
        Type: cred::CRED_TYPE_GENERIC,
        TargetName: target.as_ptr().cast_mut(),
        Comment: std::ptr::null_mut(),
        LastWritten: FILETIME::default(),
        CredentialBlobSize: (secret.len() * 2) as u32,
        CredentialBlob: secret.as_ptr().cast::<u8>().cast_mut(),
        Persist: cred::CRED_PERSIST_LOCAL_MACHINE,
        AttributeCount: 0,
        Attributes: std::ptr::null_mut(),
        TargetAlias: std::ptr::null_mut(),
        UserName: user.as_mut_ptr(),
    };
    if unsafe { cred::CredWriteW(&credential, 0) } != 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error().to_string())
    }
}

/// The account's display name, as Office takes its user name, or its login where the
/// directory has none.
pub fn user_name() -> String {
    use windows_sys::Win32::Security::Authentication::Identity::{GetUserNameExW, NameDisplay};
    let mut name = [0u16; 256];
    let mut length = name.len() as u32;
    if unsafe { GetUserNameExW(NameDisplay, name.as_mut_ptr(), &mut length) } {
        let name = narrow(&name);
        if !name.trim().is_empty() {
            return name;
        }
    }
    std::env::var("USERNAME").unwrap_or_default()
}

/// The keyboard layout's language, as OneNote tags pasted text.
pub fn input_language() -> String {
    use windows_sys::Win32::{
        Globalization::LCIDToLocaleName, UI::Input::KeyboardAndMouse::GetKeyboardLayout,
    };
    let layout = unsafe { GetKeyboardLayout(0) } as usize;
    let language = (layout & 0xffff) as u32;
    let mut name = [0u16; 85];
    let length = unsafe { LCIDToLocaleName(language, name.as_mut_ptr(), name.len() as i32, 0) };
    if length > 0 {
        narrow(&name)
    } else {
        String::new()
    }
}

/// FILETIME as local time.
fn local_time(filetime: u64) -> SYSTEMTIME {
    use windows_sys::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
    let time = FILETIME {
        dwLowDateTime: filetime as u32,
        dwHighDateTime: (filetime >> 32) as u32,
    };
    let [mut universal, mut local] = [SYSTEMTIME::default(); 2];
    unsafe {
        FileTimeToSystemTime(&time, &mut universal);
        SystemTimeToTzSpecificLocalTime(std::ptr::null(), &universal, &mut local);
    }
    local
}

/// Local time as FILETIME.
fn filetime(local: &SYSTEMTIME) -> Option<u64> {
    use windows_sys::Win32::System::Time::{SystemTimeToFileTime, TzSpecificLocalTimeToSystemTime};
    let mut universal = SYSTEMTIME::default();
    let mut time = FILETIME::default();
    unsafe {
        (TzSpecificLocalTimeToSystemTime(std::ptr::null(), local, &mut universal) != 0
            && SystemTimeToFileTime(&universal, &mut time) != 0)
            .then(|| u64::from(time.dwHighDateTime) << 32 | u64::from(time.dwLowDateTime))
    }
}

/// `time` in the user's `flags` date format.
fn date_format(time: &SYSTEMTIME, flags: u32) -> String {
    use windows_sys::Win32::Globalization::{GetDateFormatW, LOCALE_USER_DEFAULT};
    let mut text = [0u16; 128];
    let length = unsafe {
        GetDateFormatW(
            LOCALE_USER_DEFAULT,
            flags,
            time,
            std::ptr::null(),
            text.as_mut_ptr(),
            text.len() as i32,
        )
    };
    narrow(&text[..length.max(0) as usize])
}

/// `time` in the user's time format without seconds.
fn time_format(time: &SYSTEMTIME) -> String {
    use windows_sys::Win32::Globalization::{GetTimeFormatW, LOCALE_USER_DEFAULT, TIME_NOSECONDS};
    let mut text = [0u16; 64];
    let length = unsafe {
        GetTimeFormatW(
            LOCALE_USER_DEFAULT,
            TIME_NOSECONDS,
            time,
            std::ptr::null(),
            text.as_mut_ptr(),
            text.len() as i32,
        )
    };
    narrow(&text[..length.max(0) as usize])
}

/// How far local time is ahead of UTC at Unix time `unix`, in seconds.
pub fn utc_offset(unix: i64) -> i64 {
    let ticks = (unix + 11_644_473_600).max(0) as u64 * 10_000_000;
    filetime(&local_time(ticks)).map_or(0, |local| (ticks as i64 - local as i64) / -10_000_000)
}

/// FILETIME as the locale's short date, as OneNote labels a conflict page.
pub fn short_date(filetime: u64) -> String {
    date_format(
        &local_time(filetime),
        windows_sys::Win32::Globalization::DATE_SHORTDATE,
    )
}

fn date_labels(time: &SYSTEMTIME) -> [String; 2] {
    [
        date_format(time, windows_sys::Win32::Globalization::DATE_LONGDATE),
        time_format(time),
    ]
}

/// FILETIME as a new page's title shows it: the long date and the short time, which is
/// how OneNote 2010 writes them from the same settings.
pub fn date_text(filetime: u64) -> [String; 2] {
    date_labels(&local_time(filetime))
}

/// Asks for the page's date or time with the system's date and time picker.
pub fn edit_date(
    timestamp: u64,
    field: DateField,
    title: &str,
) -> Result<Option<(u64, [String; 2])>, &'static str> {
    let before = local_time(timestamp);
    let Some(chosen) = picker::pick(&before, field, title) else {
        return Ok(None);
    };
    let mut time = before;
    match field {
        DateField::Date => {
            (time.wYear, time.wMonth, time.wDay) = (chosen.wYear, chosen.wMonth, chosen.wDay);
        }
        DateField::Time => {
            (time.wHour, time.wMinute) = (chosen.wHour, chosen.wMinute);
        }
    }
    let updated = filetime(&time).ok_or(crate::DATE_OUT_OF_RANGE)?;
    // FILETIME's sub-second ticks stay as they were.
    let updated = updated / 10_000_000 * 10_000_000 + timestamp % 10_000_000;
    Ok(Some((updated, date_labels(&local_time(updated)))))
}

/// Asks for a notebook's table of contents or a section file, titled `title`.
pub fn pick_notebook(title: &str) -> Option<PathBuf> {
    pick(
        title,
        &[("OneNote notebooks and sections", "*.onetoc2;*.one")],
        None,
        false,
    )
}

/// Asks for a file to insert, one of `types` (extensions) unless empty, titled `title`.
pub fn pick_file(title: &str, types: &[&str]) -> Option<PathBuf> {
    let patterns = types
        .iter()
        .map(|kind| format!("*.{kind}"))
        .collect::<Vec<_>>()
        .join(";");
    let filters = if types.is_empty() {
        vec![("All files", "*.*")]
    } else {
        vec![("Supported files", patterns.as_str()), ("All files", "*.*")]
    };
    pick(title, &filters, None, false)
}

/// Asks where to put something named `name` by default; the dialog names its own button.
pub fn pick_new(
    title: &str,
    name: &str,
    _action: &str,
    folder: Option<&std::path::Path>,
) -> Option<PathBuf> {
    pick(title, &[("All files", "*.*")], Some((name, folder)), true)
}

/// The common file dialog: an open dialog over `filters` (name and `;`-separated
/// patterns), or with `new`, a save dialog suggesting a name in a folder.
fn pick(
    title: &str,
    filters: &[(&str, &str)],
    new: Option<(&str, Option<&std::path::Path>)>,
    save: bool,
) -> Option<PathBuf> {
    use windows_sys::Win32::UI::Controls::Dialogs as dialogs;
    let mut filter: Vec<u16> = Vec::new();
    for (name, patterns) in filters {
        filter.extend(name.encode_utf16().chain([0]));
        filter.extend(patterns.encode_utf16().chain([0]));
    }
    filter.push(0);
    let mut file = vec![0u16; 32 * 1024];
    if let Some((name, _)) = new {
        for (slot, unit) in file.iter_mut().zip(name.encode_utf16()) {
            *slot = unit;
        }
    }
    let folder = new.and_then(|(_, folder)| folder).map(wide);
    let title = wide(title);
    let mut dialog = dialogs::OPENFILENAMEW {
        lStructSize: size_of::<dialogs::OPENFILENAMEW>() as u32,
        hwndOwner: owner(),
        lpstrFilter: filter.as_ptr(),
        lpstrFile: file.as_mut_ptr(),
        nMaxFile: file.len() as u32,
        lpstrTitle: title.as_ptr(),
        lpstrInitialDir: folder
            .as_ref()
            .map_or(std::ptr::null(), |folder| folder.as_ptr()),
        Flags: dialogs::OFN_EXPLORER
            | dialogs::OFN_NOCHANGEDIR
            | if save {
                dialogs::OFN_OVERWRITEPROMPT
            } else {
                dialogs::OFN_FILEMUSTEXIST
            },
        ..unsafe { std::mem::zeroed() }
    };
    let chosen = unsafe {
        if save {
            dialogs::GetSaveFileNameW(&mut dialog)
        } else {
            dialogs::GetOpenFileNameW(&mut dialog)
        }
    };
    if chosen == 0 {
        // Zero where the user cancelled.
        let error = unsafe { dialogs::CommDlgExtendedError() };
        if error != 0 {
            eprintln!("The file dialog failed: {error:#x}");
        }
        return None;
    }
    Some(PathBuf::from(narrow(&file)))
}

/// Tells the user something they asked for could not be done: `message`, then what to do.
pub fn alert(message: &str, detail: &str) {
    let text = wide(format!("{message}\n\n{detail}"));
    let caption = wide("Snowbound");
    let style = wm::MB_OK | wm::MB_ICONWARNING;
    unsafe { wm::MessageBoxW(owner(), text.as_ptr(), caption.as_ptr(), style) };
}

/// Asks whether to go ahead with `action`, offering `cancel` too: a task dialog with those
/// buttons where the common controls have one, as from Windows Vista, else OK and Cancel.
pub fn confirm(message: &str, detail: &str, cancel: &str, action: &str) -> bool {
    use windows_sys::Win32::UI::Controls as controls;
    type Indirect = unsafe extern "system" fn(
        *const controls::TASKDIALOGCONFIG,
        *mut i32,
        *mut i32,
        *mut BOOL,
    ) -> i32;
    let [title, instruction, content, cancel_text, action_text] =
        ["Snowbound", message, detail, cancel, action].map(wide);
    if let Some(indirect) = function::<Indirect>("comctl32.dll", c"TaskDialogIndirect") {
        const ACTION: i32 = 100;
        let buttons = [
            controls::TASKDIALOG_BUTTON {
                nButtonID: ACTION,
                pszButtonText: action_text.as_ptr(),
            },
            controls::TASKDIALOG_BUTTON {
                nButtonID: wm::IDCANCEL,
                pszButtonText: cancel_text.as_ptr(),
            },
        ];
        let mut config: controls::TASKDIALOGCONFIG = unsafe { std::mem::zeroed() };
        config.cbSize = size_of::<controls::TASKDIALOGCONFIG>() as u32;
        config.hwndParent = owner();
        config.dwFlags = controls::TDF_ALLOW_DIALOG_CANCELLATION;
        config.pszWindowTitle = title.as_ptr();
        config.pszMainInstruction = instruction.as_ptr();
        config.pszContent = content.as_ptr();
        config.cButtons = buttons.len() as u32;
        config.pButtons = buttons.as_ptr();
        config.nDefaultButton = wm::IDCANCEL;
        let mut pressed = 0;
        let shown = unsafe {
            indirect(
                &config,
                &mut pressed,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if shown >= 0 {
            return pressed == ACTION;
        }
    }
    let text = wide(format!("{message}\n\n{detail}"));
    let answer = unsafe {
        wm::MessageBoxW(
            owner(),
            text.as_ptr(),
            title.as_ptr(),
            wm::MB_OKCANCEL | wm::MB_ICONQUESTION | wm::MB_DEFBUTTON2,
        )
    };
    answer == wm::IDOK
}

/// The common controls' date and time picker in a dialog of its own.
mod picker {
    use super::{DateField, SYSTEMTIME, owner, wm};
    use std::cell::Cell;
    use windows_sys::Win32::{
        Foundation::{HWND, LPARAM, WPARAM},
        UI::Controls as controls,
    };

    const PICKER: i32 = 100;

    thread_local! {
        static TIME: Cell<SYSTEMTIME> = Cell::new(SYSTEMTIME::default());
    }

    /// The dialog, as a template of 16-bit words: its frame, then the picker, OK and Cancel.
    fn template(title: &str, style: u32) -> Vec<u16> {
        let mut words = Vec::new();
        let dword = |words: &mut Vec<u16>, value: u32| {
            words.extend([value as u16, (value >> 16) as u16]);
        };
        let text = |words: &mut Vec<u16>, text: &str| {
            words.extend(text.encode_utf16().chain([0]));
        };
        dword(
            &mut words,
            wm::DS_MODALFRAME as u32
                | wm::DS_CENTER as u32
                | wm::DS_SETFONT as u32
                | wm::WS_POPUP
                | wm::WS_CAPTION
                | wm::WS_SYSMENU,
        );
        dword(&mut words, 0);
        // Three controls in a dialog of 160 by 62 dialog units.
        words.extend([3, 0, 0, 160, 62]);
        // No menu, the default class, the title, then 9-point Segoe UI.
        words.extend([0, 0]);
        text(&mut words, title);
        words.push(9);
        text(&mut words, "Segoe UI");
        let item = |words: &mut Vec<u16>,
                    style: u32,
                    [x, y, width, height]: [u16; 4],
                    id: i32,
                    class: &[u16],
                    title: &str| {
            if words.len() % 2 == 1 {
                words.push(0);
            }
            dword(words, style | wm::WS_CHILD | wm::WS_VISIBLE);
            dword(words, 0);
            words.extend([x, y, width, height, id as u16]);
            words.extend(class);
            text(words, title);
            words.push(0);
        };
        let class: Vec<u16> = "SysDateTimePick32".encode_utf16().chain([0]).collect();
        item(
            &mut words,
            style | wm::WS_TABSTOP,
            [7, 7, 146, 14],
            PICKER,
            &class,
            "",
        );
        // 0xFFFF 0x0080 is the button class.
        item(
            &mut words,
            wm::BS_DEFPUSHBUTTON as u32 | wm::WS_TABSTOP,
            [49, 38, 50, 14],
            wm::IDOK,
            &[0xffff, 0x0080],
            "OK",
        );
        item(
            &mut words,
            wm::BS_PUSHBUTTON as u32 | wm::WS_TABSTOP,
            [103, 38, 50, 14],
            wm::IDCANCEL,
            &[0xffff, 0x0080],
            "Cancel",
        );
        words
    }

    unsafe extern "system" fn procedure(
        dialog: HWND,
        message: u32,
        wparam: WPARAM,
        _: LPARAM,
    ) -> isize {
        let picker = unsafe { wm::GetDlgItem(dialog, PICKER) };
        match message {
            wm::WM_INITDIALOG => {
                let time = TIME.get();
                unsafe {
                    wm::SendMessageW(
                        picker,
                        controls::DTM_SETSYSTEMTIME,
                        controls::GDT_VALID as WPARAM,
                        (&time as *const SYSTEMTIME) as LPARAM,
                    )
                };
                1
            }
            wm::WM_COMMAND => {
                let id = (wparam & 0xffff) as i32;
                if id == wm::IDOK {
                    let mut time = SYSTEMTIME::default();
                    unsafe {
                        wm::SendMessageW(
                            picker,
                            controls::DTM_GETSYSTEMTIME,
                            0,
                            (&mut time as *mut SYSTEMTIME) as LPARAM,
                        )
                    };
                    TIME.set(time);
                }
                if id == wm::IDOK || id == wm::IDCANCEL {
                    unsafe { wm::EndDialog(dialog, id as isize) };
                    return 1;
                }
                0
            }
            _ => 0,
        }
    }

    /// The date or time the user picks, starting from `time`; none when cancelled.
    pub(super) fn pick(time: &SYSTEMTIME, field: DateField, title: &str) -> Option<SYSTEMTIME> {
        let classes = controls::INITCOMMONCONTROLSEX {
            dwSize: size_of::<controls::INITCOMMONCONTROLSEX>() as u32,
            dwICC: controls::ICC_DATE_CLASSES,
        };
        unsafe { controls::InitCommonControlsEx(&classes) };
        let style = match field {
            DateField::Date => controls::DTS_LONGDATEFORMAT,
            DateField::Time => controls::DTS_TIMEFORMAT,
        };
        TIME.set(*time);
        let template = template(title, style);
        let chosen = unsafe {
            wm::DialogBoxIndirectParamW(
                std::ptr::null_mut(),
                template.as_ptr().cast(),
                owner(),
                Some(procedure),
                0,
            )
        };
        (chosen == wm::IDOK as isize).then(|| TIME.get())
    }
}
