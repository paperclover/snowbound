use crate::commands;
use canvas::date::DateField;
use objc2::{
    ClassType, DeclaredClass,
    declare::ClassBuilder,
    declare_class, msg_send, msg_send_id, mutability,
    rc::{Allocated, Retained},
    runtime::{AnyClass, AnyObject, Sel},
    sel,
};
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSAlertSecondButtonReturn, NSApplication, NSColor,
    NSColorSpace, NSDatePicker, NSDatePickerElementFlags, NSDatePickerStyle, NSEvent,
    NSEventSubtype, NSEventType, NSMenu, NSMenuItem,
};
use objc2_foundation::{
    MainThreadMarker, NSAttributedString, NSCalendar, NSCalendarUnit, NSDate, NSDateFormatter,
    NSDateFormatterStyle, NSPoint, NSRange, NSRect, NSString,
};
use std::{
    cell::{Cell, RefCell},
    sync::OnceLock,
};
use winit::{
    error::EventLoopError,
    event_loop::{EventLoop, EventLoopProxy},
    platform::macos::WindowAttributesExtMacOS,
    raw_window_handle::{HasWindowHandle, RawWindowHandle},
    window::{Window, WindowAttributes},
};

/// Runs `run` inside an autorelease pool. 10.6 has none outside NSApplication's run loop,
/// and quitting releases the windows after it returns.
pub fn with_pool<R>(run: impl FnOnce() -> R) -> R {
    objc2::rc::autoreleasepool(|_| run())
}

pub use crate::aqua::{cover_border_line, move_cursor, resize_grip, system_interface};

static QUIT: OnceLock<EventLoopProxy<crate::UserEvent>> = OnceLock::new();
static INPUT_CLASS: OnceLock<&'static AnyClass> = OnceLock::new();
static BACKDROP_CLASS: OnceLock<&'static AnyClass> = OnceLock::new();
static DELEGATE_CLASS: OnceLock<&'static AnyClass> = OnceLock::new();
thread_local! {
    static IN_KEY_DOWN: Cell<bool> = const { Cell::new(false) };
    static STATUSES: RefCell<Vec<commands::Status>> = const { RefCell::new(Vec::new()) };
    static TAGS_MENU: RefCell<Option<Retained<NSMenu>>> = const { RefCell::new(None) };
}

unsafe extern "C" fn key_down(view: &AnyObject, _: Sel, event: &NSEvent) {
    let previous = IN_KEY_DOWN.replace(true);
    unsafe {
        let superclass = INPUT_CLASS.get().unwrap().superclass().unwrap();
        let _: () = msg_send![super(view, superclass), keyDown: event];
    }
    IN_KEY_DOWN.set(previous);
}

unsafe extern "C" fn insert_text(view: &AnyObject, _: Sel, text: &AnyObject, range: NSRange) {
    unsafe {
        let marked: bool = msg_send![view, hasMarkedText];
        if IN_KEY_DOWN.get() || marked {
            let superclass = INPUT_CLASS.get().unwrap().superclass().unwrap();
            let _: () =
                msg_send![super(view, superclass), insertText: text replacementRange: range];
        } else {
            // NSTextInputClient supplies either NSString or NSAttributedString.
            let attributed: bool = msg_send![text, isKindOfClass: NSAttributedString::class()];
            let string: Retained<NSString> = if attributed {
                msg_send_id![text, string]
            } else {
                msg_send_id![text, copy]
            };
            if let Some(proxy) = QUIT.get() {
                let _ = proxy.send_event(crate::UserEvent::InsertText(string.to_string()));
            }
        }
    }
}

fn ns_window(window: &Window) -> Retained<AnyObject> {
    let RawWindowHandle::AppKit(handle) =
        window.window_handle().expect("Live AppKit window").as_raw()
    else {
        unreachable!()
    };
    unsafe {
        let view = &*handle.ns_view.as_ptr().cast::<AnyObject>();
        msg_send_id![view, window]
    }
}

/// Room the traffic lights take at the title bar's leading edge.
pub const LEADING: f32 = 78.0;
/// The title bar's trailing margin, the gap the traffic lights leave before the toolbar.
pub const TRAILING: f32 = 12.0;
/// How far AppKit rounds a window's corners.
pub const CORNER_RADIUS: f32 = 10.0;

/// How far the window's corners round: 10.6 rounds a textured window's bottom corners
/// by about 3.5 pixels.
pub fn corner_radius(_: &Window) -> f32 {
    if crate::aqua::before_lion() {
        3.5
    } else {
        CORNER_RADIUS
    }
}

/// AppKit clips the window's corners itself, except where the OpenGL surface covers
/// 10.6's textured window, whose rounded bottom corners the app leaves transparent.
pub fn cuts_corners() -> bool {
    crate::aqua::before_lion()
}

/// A transparent title bar over the content, where the window draws its own.
pub fn window_attributes() -> WindowAttributes {
    Window::default_attributes()
        .with_titlebar_transparent(true)
        .with_fullsize_content_view(true)
}

/// Makes the title bar a unified compact toolbar's, as tall as the toolbar's row, which the
/// app draws: an empty `NSToolbar` and a hidden title. AppKit places the traffic lights in
/// it. The title is still set, for the Window menu, Mission Control and VoiceOver.
pub fn install_title_bar(window: &Window) {
    MainThreadMarker::new().expect("Windows belong to the main thread");
    let window = ns_window(window);
    let responds =
        |selector: Sel| -> bool { unsafe { msg_send![&window, respondsToSelector: selector] } };
    // Before 10.10 the title bar keeps its own line above the row.
    if crate::aqua::before_lion() || !responds(sel!(setTitleVisibility:)) {
        return;
    }
    unsafe {
        let class = AnyClass::get("NSToolbar").expect("AppKit is linked");
        let toolbar: Allocated<AnyObject> = msg_send_id![class, alloc];
        let toolbar: Retained<AnyObject> =
            msg_send_id![toolbar, initWithIdentifier: &*NSString::from_str("Snowbound")];
        let _: () = msg_send![&toolbar, setShowsBaselineSeparator: false];
        let _: () = msg_send![&window, setToolbar: &*toolbar];
        // NSWindowTitleHidden.
        let _: () = msg_send![&window, setTitleVisibility: 1isize];
        if responds(sel!(setToolbarStyle:)) {
            // NSWindowToolbarStyleUnifiedCompact and NSTitlebarSeparatorStyleNone.
            let _: () = msg_send![&window, setToolbarStyle: 4isize];
            let _: () = msg_send![&window, setTitlebarSeparatorStyle: 1isize];
        }
    }
}

/// The centres of the close, minimize and zoom buttons where AppKit placed them, in points
/// from the content's top left corner.
pub fn traffic_lights(window: &Window) -> [[f32; 2]; 3] {
    let window = ns_window(window);
    unsafe {
        let content: Retained<AnyObject> = msg_send_id![&window, contentView];
        let content: NSRect = msg_send![&content, frame];
        // NSWindowCloseButton, NSWindowMiniaturizeButton and NSWindowZoomButton, in the
        // window's coordinates, which rise from its bottom left corner.
        [0usize, 1, 2].map(|kind| {
            let button: Retained<AnyObject> = msg_send_id![&window, standardWindowButton: kind];
            let bounds: NSRect = msg_send![&button, bounds];
            let frame: NSRect =
                msg_send![&button, convertRect: bounds, toView: std::ptr::null::<AnyObject>()];
            [
                (frame.origin.x + frame.size.width / 2.0) as f32,
                (content.size.height - frame.origin.y - frame.size.height / 2.0) as f32,
            ]
        })
    }
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

/// AppKit draws the traffic lights.
pub fn window_controls(_: &mut ui::Ui, _: &Window) {}

/// Whether AppKit draws the title bar: before 10.10 content can't extend under it, and
/// otherwise the app draws it around the traffic lights.
pub fn system_titlebar(window: &Window) -> bool {
    if crate::aqua::before_lion() {
        return true;
    }
    let window = ns_window(window);
    unsafe {
        let frame: NSRect = msg_send![&window, frame];
        let content: NSRect = msg_send![&window, contentRectForFrameRect: frame];
        content.size.height < frame.size.height
    }
}

unsafe extern "C" fn no_document_drag(
    _: &AnyObject,
    _: Sel,
    _: &AnyObject,
    _: &AnyObject,
    _: NSPoint,
    _: &AnyObject,
) -> objc2::runtime::Bool {
    objc2::runtime::Bool::NO
}

/// Names `file` as the window's document: the title shows its icon, and Command-clicking the
/// title lists the folders it lies in. Dragging the icon, which could move the file out of its
/// notebook, is off.
pub fn represent(window: &Window, file: Option<&std::path::Path>) {
    let window = ns_window(window);
    unsafe {
        let delegate: Option<Retained<AnyObject>> = msg_send_id![&window, delegate];
        if let Some(delegate) = delegate {
            let class = DELEGATE_CLASS.get_or_init(|| {
                let mut class = ClassBuilder::new("SnowboundWindowDelegate", delegate.class())
                    .expect("Unique window delegate class");
                class.add_method(
                    sel!(window:shouldDragDocumentWithEvent:from:withPasteboard:),
                    no_document_drag as unsafe extern "C" fn(_, _, _, _, _, _) -> _,
                );
                class.register()
            });
            if delegate.class() != *class {
                assert_eq!(class.superclass(), Some(delegate.class()));
                assert_eq!(class.instance_size(), delegate.class().instance_size());
                // The subclass adds no ivars, so the existing allocation remains valid.
                AnyObject::set_class(&delegate, class);
            }
        }
        let url = file.map(|file| {
            objc2_foundation::NSURL::fileURLWithPath(&NSString::from_str(&file.to_string_lossy()))
        });
        let _: () = msg_send![&window, setRepresentedURL: url.as_deref()];
    }
}

/// None: the kit's own menus stand in for AppKit's.
pub fn menu(_: winit::window::Theme) -> Option<ui::Menu> {
    None
}

/// Nothing the backdrop leaves to colour.
pub fn titlebar(_: winit::window::Theme) -> Option<[[f32; 4]; 2]> {
    None
}

unsafe extern "C" fn hit_nothing(_: &AnyObject, _: Sel, _: NSPoint) -> *mut AnyObject {
    std::ptr::null_mut()
}

/// Lays the title bar's material under the window's content, where it shows through the
/// app's transparent pixels as AppKit's title bars and toolbars show it: tinted by the
/// desktop, and following the window's appearance and whether it is key. Presses pass
/// through it to the view it lies in.
pub fn install_backdrop(window: &Window) -> bool {
    MainThreadMarker::new().expect("Views belong to the main thread");
    // Before 10.10 AppKit has no materials; 10.6's textured window stands in, its gradient
    // running through the toolbar and the tab row to the notebook's frame.
    let Some(effect) = AnyClass::get("NSVisualEffectView") else {
        return crate::aqua::textured(window, crate::TITLE + crate::TAB_ROW);
    };
    let RawWindowHandle::AppKit(handle) =
        window.window_handle().expect("Live AppKit window").as_raw()
    else {
        unreachable!()
    };
    unsafe {
        let view = &*handle.ns_view.as_ptr().cast::<AnyObject>();
        let class = BACKDROP_CLASS.get_or_init(|| {
            let mut class =
                ClassBuilder::new("SnowboundBackdrop", effect).expect("Unique backdrop class");
            class.add_method(
                sel!(hitTest:),
                hit_nothing as unsafe extern "C" fn(_, _, _) -> _,
            );
            class.register()
        });
        let bounds: NSRect = msg_send![view, bounds];
        let backdrop: Allocated<AnyObject> = msg_send_id![*class, alloc];
        let backdrop: Retained<AnyObject> = msg_send_id![backdrop, initWithFrame: bounds];
        // NSVisualEffectMaterialTitlebar, NSVisualEffectBlendingModeWithinWindow (which
        // AppKit's own title bar measures as, over the window's desktop-tinted background)
        // and NSVisualEffectStateFollowsWindowActiveState.
        let _: () = msg_send![&backdrop, setMaterial: 3isize];
        let _: () = msg_send![&backdrop, setBlendingMode: 1isize];
        let _: () = msg_send![&backdrop, setState: 0isize];
        // NSViewWidthSizable | NSViewHeightSizable
        let _: () = msg_send![&backdrop, setAutoresizingMask: 18usize];
        let _: () = msg_send![view, addSubview: &*backdrop];
    }
    true
}

/// AppKit's window frame takes resizing presses.
pub fn resize_direction(_: &Window, _: [f32; 2]) -> Option<winit::window::ResizeDirection> {
    None
}

pub const SHOW_FILE: &str = "Show in Finder";

/// Opens a Finder window with `file` selected.
pub fn show_file(file: &std::path::Path) {
    unsafe {
        let url =
            objc2_foundation::NSURL::fileURLWithPath(&NSString::from_str(&file.to_string_lossy()));
        objc2_app_kit::NSWorkspace::sharedWorkspace()
            .activateFileViewerSelectingURLs(&objc2_foundation::NSArray::from_vec(vec![url]));
    }
}

pub fn cache_dir() -> Option<std::path::PathBuf> {
    Some(std::path::PathBuf::from(std::env::var_os("HOME")?).join("Library/Caches/snowbound"))
}

/// The share holding `path` where macOS mounted it from an SMB server.
pub fn smb_mount(path: &std::path::Path) -> Option<crate::library::Mount> {
    use std::os::unix::ffi::OsStrExt;
    let name = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut mount: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(name.as_ptr(), &mut mount) } != 0 {
        return None;
    }
    let text = |chars: &[libc::c_char]| {
        unsafe { std::ffi::CStr::from_ptr(chars.as_ptr()) }
            .to_string_lossy()
            .into_owned()
    };
    if text(&mount.f_fstypename) != "smbfs" {
        return None;
    }
    let within = path.strip_prefix(text(&mount.f_mntonname)).ok()?;
    crate::library::Mount::parse(&text(&mount.f_mntfromname), &within.to_string_lossy(), "")
}

#[allow(non_camel_case_types)]
type DNSServiceResolveReply = extern "C" fn(
    service: *mut std::ffi::c_void,
    flags: u32,
    interface: u32,
    error: i32,
    name: *const libc::c_char,
    host: *const libc::c_char,
    port: u16,
    txt_length: u16,
    txt: *const u8,
    context: *mut std::ffi::c_void,
);

unsafe extern "C" {
    fn DNSServiceResolve(
        service: *mut *mut std::ffi::c_void,
        flags: u32,
        interface: u32,
        name: *const libc::c_char,
        kind: *const libc::c_char,
        domain: *const libc::c_char,
        reply: DNSServiceResolveReply,
        context: *mut std::ffi::c_void,
    ) -> i32;
    fn DNSServiceRefSockFD(service: *mut std::ffi::c_void) -> i32;
    fn DNSServiceProcessResult(service: *mut std::ffi::c_void) -> i32;
    fn DNSServiceRefDeallocate(service: *mut std::ffi::c_void);
}

/// The `host:port` the SMB service Bonjour names `instance` answers at.
pub fn bonjour_endpoint(instance: &str) -> Option<String> {
    extern "C" fn resolved(
        _: *mut std::ffi::c_void,
        _: u32,
        _: u32,
        error: i32,
        _: *const libc::c_char,
        host: *const libc::c_char,
        port: u16,
        _: u16,
        _: *const u8,
        context: *mut std::ffi::c_void,
    ) {
        if error != 0 || host.is_null() {
            return;
        }
        let host = unsafe { std::ffi::CStr::from_ptr(host) }.to_string_lossy();
        let found = format!("{}:{}", host.trim_end_matches('.'), u16::from_be(port));
        unsafe { *context.cast::<Option<String>>() = Some(found) };
    }
    let name = std::ffi::CString::new(instance).ok()?;
    let mut service = std::ptr::null_mut();
    let mut found: Option<String> = None;
    let status = unsafe {
        DNSServiceResolve(
            &mut service,
            0,
            0,
            name.as_ptr(),
            c"_smb._tcp".as_ptr(),
            c"local.".as_ptr(),
            resolved,
            (&raw mut found).cast(),
        )
    };
    if status != 0 {
        return None;
    }
    let mut ready = libc::pollfd {
        fd: unsafe { DNSServiceRefSockFD(service) },
        events: libc::POLLIN,
        revents: 0,
    };
    // As long as the Finder waits to connect.
    if unsafe { libc::poll(&mut ready, 1, 5000) } == 1 {
        unsafe { DNSServiceProcessResult(service) };
    }
    unsafe { DNSServiceRefDeallocate(service) };
    found
}

#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    fn SecKeychainFindInternetPassword(
        keychain: *const std::ffi::c_void,
        server_length: u32,
        server: *const u8,
        domain_length: u32,
        domain: *const u8,
        account_length: u32,
        account: *const u8,
        path_length: u32,
        path: *const u8,
        port: u16,
        protocol: u32,
        authentication: u32,
        password_length: *mut u32,
        password: *mut *mut std::ffi::c_void,
        item: *mut *const std::ffi::c_void,
    ) -> i32;
    fn SecKeychainItemFreeContent(
        list: *const std::ffi::c_void,
        data: *mut std::ffi::c_void,
    ) -> i32;
    fn SecKeychainAddInternetPassword(
        keychain: *const std::ffi::c_void,
        server_length: u32,
        server: *const u8,
        domain_length: u32,
        domain: *const u8,
        account_length: u32,
        account: *const u8,
        path_length: u32,
        path: *const u8,
        port: u16,
        protocol: u32,
        authentication: u32,
        password_length: u32,
        password: *const u8,
        item: *mut *const std::ffi::c_void,
    ) -> i32;
    fn SecKeychainItemModifyAttributesAndData(
        item: *const std::ffi::c_void,
        attributes: *const std::ffi::c_void,
        length: u32,
        data: *const u8,
    ) -> i32;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(object: *const std::ffi::c_void);
}

/// kSecProtocolTypeSMB, as the Finder keeps an SMB server's passwords.
const SMB: u32 = u32::from_be_bytes(*b"smb ");
/// errSecDuplicateItem.
const DUPLICATE: i32 = -25299;

/// What the sign-in offers for keeping a password, which the keychain keeps.
pub fn remember_label() -> Option<&'static str> {
    Some("Remember this password in my keychain")
}

/// Keeps `login`'s password for `mount`'s server in the default keychain, as the Finder keeps
/// one, replacing any kept for the account before.
pub fn save_login(
    mount: &crate::library::Mount,
    login: &crate::library::Login,
) -> Result<(), String> {
    save_login_in(std::ptr::null(), mount, login)
}

fn save_login_in(
    keychain: *const std::ffi::c_void,
    mount: &crate::library::Mount,
    login: &crate::library::Login,
) -> Result<(), String> {
    let (server, user, password) = (mount.host(), &login.user, &login.password);
    let port = match mount.host() == mount.server {
        true => 0,
        false => mount.server[server.len() + 1..].parse().unwrap_or(0),
    };
    let status = unsafe {
        SecKeychainAddInternetPassword(
            keychain,
            server.len() as u32,
            server.as_ptr(),
            0,
            std::ptr::null(),
            user.len() as u32,
            user.as_ptr(),
            0,
            std::ptr::null(),
            port,
            SMB,
            u32::from_be_bytes(*b"dflt"),
            password.len() as u32,
            password.as_ptr(),
            std::ptr::null_mut(),
        )
    };
    let status = match status {
        DUPLICATE => {
            let mut item = std::ptr::null();
            let found = unsafe {
                SecKeychainFindInternetPassword(
                    keychain,
                    server.len() as u32,
                    server.as_ptr(),
                    0,
                    std::ptr::null(),
                    user.len() as u32,
                    user.as_ptr(),
                    0,
                    std::ptr::null(),
                    port,
                    SMB,
                    0,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    &mut item,
                )
            };
            if found != 0 {
                found
            } else {
                let status = unsafe {
                    SecKeychainItemModifyAttributesAndData(
                        item,
                        std::ptr::null(),
                        password.len() as u32,
                        password.as_ptr(),
                    )
                };
                unsafe { CFRelease(item) };
                status
            }
        }
        status => status,
    };
    match status {
        0 => Ok(()),
        status => Err(format!("The keychain didn't keep the password ({status})")),
    }
}

/// The password the keychain keeps for `mount`'s account on its server, as macOS saved it
/// when the share was mounted or Snowbound when it signed in; the system asks the user to
/// allow the app to read it.
pub fn smb_login(mount: &crate::library::Mount) -> Result<crate::library::Login, String> {
    smb_login_in(std::ptr::null(), mount)
}

fn smb_login_in(
    keychain: *const std::ffi::c_void,
    mount: &crate::library::Mount,
) -> Result<crate::library::Login, String> {
    let Some(user) = &mount.user else {
        return Ok(crate::library::Login::guest(mount));
    };
    let server = mount.host();
    let (mut length, mut data) = (0, std::ptr::null_mut());
    let status = unsafe {
        SecKeychainFindInternetPassword(
            keychain,
            server.len() as u32,
            server.as_ptr(),
            0,
            std::ptr::null(),
            user.len() as u32,
            user.as_ptr(),
            0,
            std::ptr::null(),
            0,
            SMB,
            0,
            &mut length,
            &mut data,
            std::ptr::null_mut(),
        )
    };
    if status != 0 {
        return Err(format!(
            "Enter the password for \u{201c}{user}\u{201d} on \u{201c}{server}\u{201d}."
        ));
    }
    let password = unsafe { std::slice::from_raw_parts(data.cast::<u8>(), length as usize) };
    let password = String::from_utf8_lossy(password).into_owned();
    unsafe { SecKeychainItemFreeContent(std::ptr::null(), data) };
    Ok(crate::library::Login {
        user: user.clone(),
        password,
        domain: mount.domain.clone(),
    })
}

pub fn settings_dir() -> Option<std::path::PathBuf> {
    Some(
        std::path::PathBuf::from(std::env::var_os("HOME")?)
            .join("Library/Application Support/Snowbound"),
    )
}

/// Asks for a notebook folder or a notebook file with the system's open panel, titled
/// `title`.
pub fn pick_notebook(title: &str) -> Option<std::path::PathBuf> {
    let mtm = MainThreadMarker::new().expect("Panels belong to the main thread");
    unsafe {
        let panel = objc2_app_kit::NSOpenPanel::openPanel(mtm);
        panel.setCanChooseDirectories(true);
        panel.setCanChooseFiles(true);
        panel.setCanCreateDirectories(true);
        panel.setTitle(Some(&NSString::from_str(title)));
        panel.setPrompt(Some(&NSString::from_str("Open")));
        if panel.runModal() != objc2_app_kit::NSModalResponseOK {
            return None;
        }
        let path = panel.URLs().firstObject()?.path()?;
        Some(path.to_string().into())
    }
}

/// Asks where to `action` something named `name` by default, with the system's save panel,
/// starting in `folder` where given.
pub fn pick_new(
    title: &str,
    name: &str,
    action: &str,
    folder: Option<&std::path::Path>,
) -> Option<std::path::PathBuf> {
    let mtm = MainThreadMarker::new().expect("Panels belong to the main thread");
    unsafe {
        let panel = objc2_app_kit::NSSavePanel::savePanel(mtm);
        if let Some(folder) = folder.and_then(std::path::Path::to_str) {
            panel.setDirectoryURL(Some(&objc2_foundation::NSURL::fileURLWithPath(
                &NSString::from_str(folder),
            )));
        }
        panel.setCanCreateDirectories(true);
        panel.setTitle(Some(&NSString::from_str(title)));
        panel.setPrompt(Some(&NSString::from_str(action)));
        panel.setNameFieldStringValue(&NSString::from_str(name));
        if panel.runModal() != objc2_app_kit::NSModalResponseOK {
            return None;
        }
        Some(panel.URL()?.path()?.to_string().into())
    }
}

/// Asks for a file to insert, one of `types` (extensions) unless empty, with the system's
/// open panel titled `title`.
pub fn pick_file(title: &str, types: &[&str]) -> Option<std::path::PathBuf> {
    let mtm = MainThreadMarker::new().expect("Panels belong to the main thread");
    unsafe {
        let panel = objc2_app_kit::NSOpenPanel::openPanel(mtm);
        if !types.is_empty() {
            let types = types.iter().map(|kind| NSString::from_str(kind)).collect();
            #[allow(deprecated, reason = "Allowed content types need macOS 11")]
            panel.setAllowedFileTypes(Some(&objc2_foundation::NSArray::from_vec(types)));
        }
        panel.setTitle(Some(&NSString::from_str(title)));
        panel.setPrompt(Some(&NSString::from_str("Insert")));
        if panel.runModal() != objc2_app_kit::NSModalResponseOK {
            return None;
        }
        let path = panel.URLs().firstObject()?.path()?;
        Some(path.to_string().into())
    }
}

/// The icon Finder shows for the file at `path`, as a 32 pixel PNG like the one OneNote
/// 2010 stores with an attachment.
pub fn file_icon(path: &std::path::Path) -> Option<Vec<u8>> {
    MainThreadMarker::new().expect("AppKit draws on the main thread");
    let class = |name| AnyClass::get(name).expect("AppKit is linked");
    unsafe {
        let workspace: Retained<AnyObject> = msg_send_id![class("NSWorkspace"), sharedWorkspace];
        let icon: Retained<AnyObject> =
            msg_send_id![&workspace, iconForFile: &*NSString::from_str(path.to_str()?)];
        let bitmap: Allocated<AnyObject> = msg_send_id![class("NSBitmapImageRep"), alloc];
        let bitmap: Option<Retained<AnyObject>> = msg_send_id![
            bitmap,
            initWithBitmapDataPlanes: std::ptr::null_mut::<*mut u8>(),
            pixelsWide: 32isize,
            pixelsHigh: 32isize,
            bitsPerSample: 8isize,
            samplesPerPixel: 4isize,
            hasAlpha: true,
            isPlanar: false,
            colorSpaceName: &*NSString::from_str("NSDeviceRGBColorSpace"),
            bytesPerRow: 0isize,
            bitsPerPixel: 0isize
        ];
        let bitmap = bitmap?;
        let graphics = class("NSGraphicsContext");
        let context: Option<Retained<AnyObject>> =
            msg_send_id![graphics, graphicsContextWithBitmapImageRep: &*bitmap];
        let _: () = msg_send![graphics, saveGraphicsState];
        let _: () = msg_send![graphics, setCurrentContext: context.as_deref()];
        let rect = NSRect::new(
            NSPoint::new(0.0, 0.0),
            objc2_foundation::NSSize::new(32.0, 32.0),
        );
        // NSCompositingOperationSourceOver.
        let _: () = msg_send![&icon, drawInRect: rect, fromRect: NSRect::ZERO, operation: 2usize, fraction: 1.0f64];
        let _: () = msg_send![graphics, restoreGraphicsState];
        let properties: Retained<AnyObject> = msg_send_id![class("NSDictionary"), dictionary];
        // NSBitmapImageFileTypePNG.
        let data: Option<Retained<AnyObject>> =
            msg_send_id![&bitmap, representationUsingType: 4usize, properties: &*properties];
        let data = data?;
        let length: usize = msg_send![&data, length];
        let bytes: *const std::ffi::c_void = msg_send![&data, bytes];
        Some(std::slice::from_raw_parts(bytes.cast::<u8>(), length).to_vec())
    }
}

/// Where the pointer is in the window, in points from the content's top left corner, for a
/// drop, which AppKit reports without a position.
pub fn drop_point(window: &Window) -> Option<[f32; 2]> {
    let window = ns_window(window);
    unsafe {
        let point: NSPoint = msg_send![&window, mouseLocationOutsideOfEventStream];
        let content: Retained<AnyObject> = msg_send_id![&window, contentView];
        let frame: NSRect = msg_send![&content, frame];
        Some([point.x as f32, (frame.size.height - point.y) as f32])
    }
}

/// Opens a copy of an attachment with its application, quarantined as a download is, so
/// Gatekeeper checks it where OneNote 2010 warns before opening one.
pub fn open_file(path: &std::path::Path) {
    use std::os::unix::ffi::OsStrExt;
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let value = format!("0081;{seconds:08x};Snowbound;");
    if let Ok(file) = std::ffi::CString::new(path.as_os_str().as_bytes()) {
        unsafe {
            libc::setxattr(
                file.as_ptr(),
                c"com.apple.quarantine".as_ptr(),
                value.as_ptr().cast(),
                value.len(),
                0,
                0,
            );
        }
    }
    reveal(path);
}

/// Lets the user drag out part of the screen, then sends it as a PNG.
pub fn clip_screen(proxy: EventLoopProxy<crate::UserEvent>) {
    std::thread::spawn(move || {
        let path = std::env::temp_dir().join(format!("snowbound-clip-{}.png", std::process::id()));
        let taken = std::process::Command::new("/usr/sbin/screencapture")
            .arg("-i")
            .arg(&path)
            .status()
            .is_ok_and(|status| status.success());
        // Escape leaves no file.
        if taken && let Ok(bytes) = std::fs::read(&path) {
            let _ = std::fs::remove_file(&path);
            let _ = proxy.send_event(crate::UserEvent::Picture(bytes));
        }
    });
}

/// Shows the system's character palette, whose picks arrive as inserted text.
pub fn character_palette() {
    let mtm = MainThreadMarker::new().expect("AppKit belongs to the main thread");
    NSApplication::sharedApplication(mtm).orderFrontCharacterPalette(None);
}

/// Tells the user something they asked for could not be done: `message`, then what to do.
pub fn alert(message: &str, detail: &str) {
    let mtm = MainThreadMarker::new().expect("Window events run on the main thread");
    unsafe {
        let alert = NSAlert::new(mtm);
        alert.setMessageText(&NSString::from_str(message));
        alert.setInformativeText(&NSString::from_str(detail));
        alert.runModal();
    }
}

pub fn appearance(window: &Window) -> winit::window::Theme {
    window.theme().unwrap_or(winit::window::Theme::Dark)
}

/// Zooms the window once the current event is handled: AppKit's zoom animation runs its
/// own loop, and started from inside winit's handler it would hold every resize until the
/// end, stretching the last frame instead of drawing each step.
pub fn zoom(window: &Window) {
    let window = ns_window(window);
    unsafe {
        let _: () = msg_send![
            &window,
            performSelector: sel!(zoom:),
            withObject: std::ptr::null::<AnyObject>(),
            afterDelay: 0.0f64
        ];
    }
}

/// Install before AccessKit subclasses the same view, preserving its restoration chain.
pub fn install_text_input(window: &Window) {
    MainThreadMarker::new().expect("Text input belongs to the main thread");
    let RawWindowHandle::AppKit(handle) =
        window.window_handle().expect("Live AppKit window").as_raw()
    else {
        unreachable!()
    };
    unsafe {
        let view = &*handle.ns_view.as_ptr().cast::<AnyObject>();
        let class = INPUT_CLASS.get_or_init(|| {
            let mut class = ClassBuilder::new("SnowboundTextInputView", view.class())
                .expect("Unique text input class");
            class.add_method(sel!(keyDown:), key_down as unsafe extern "C" fn(_, _, _));
            class.add_method(
                sel!(insertText:replacementRange:),
                insert_text as unsafe extern "C" fn(_, _, _, _),
            );
            if crate::aqua::before_lion() {
                class.add_method(
                    sel!(mouseDownCanMoveWindow),
                    crate::aqua::no_window_drags as extern "C" fn(_, _) -> _,
                );
            }
            class.register()
        });
        assert_eq!(class.superclass(), Some(view.class()));
        assert_eq!(class.instance_size(), view.class().instance_size());
        // The subclass adds no ivars, so the existing allocation remains valid.
        AnyObject::set_class(view, class);
    }
}

/// A tablet pen's pressure in the event AppKit is delivering, from 0 to 1; none for a mouse
/// or trackpad. winit reports only a trackpad's Force Touch, so the pen's comes from here.
pub fn pen_pressure() -> Option<f32> {
    let mtm = MainThreadMarker::new()?;
    let event = NSApplication::sharedApplication(mtm).currentEvent()?;
    let tablet = matches!(
        unsafe { event.r#type() },
        NSEventType::LeftMouseDown | NSEventType::LeftMouseDragged | NSEventType::LeftMouseUp
    ) && unsafe { event.subtype() } == NSEventSubtype::TabletPoint;
    tablet.then(|| unsafe { event.pressure() })
}

pub fn double_click_interval() -> std::time::Duration {
    std::time::Duration::from_secs_f64(unsafe { NSEvent::doubleClickInterval() })
}

pub fn edit_date(
    timestamp: u64,
    field: DateField,
    title: &str,
) -> Result<Option<(u64, [String; 2])>, &'static str> {
    let mtm = MainThreadMarker::new().expect("Date controls belong to the main thread");
    unsafe {
        let calendar = NSCalendar::currentCalendar();
        let picker = NSDatePicker::new(mtm);
        picker.setDatePickerStyle(NSDatePickerStyle::TextFieldAndStepper);
        picker.setDatePickerElements(match field {
            DateField::Date => NSDatePickerElementFlags::NSDatePickerElementFlagYearMonthDay,
            DateField::Time => NSDatePickerElementFlags::NSDatePickerElementFlagHourMinute,
        });
        picker.setPresentsCalendarOverlay(true);
        picker.setCalendar(Some(&calendar));
        picker.setTimeZone(Some(&calendar.timeZone()));
        picker.setDateValue(&NSDate::dateWithTimeIntervalSince1970(
            (timestamp / 10_000_000) as f64 - 11_644_473_600.0,
        ));
        picker.setMinDate(Some(&NSDate::dateWithTimeIntervalSince1970(
            -11_644_473_600.0,
        )));
        let label = NSString::from_str(canvas::interaction::DATE_LABELS[field as usize]);
        let _: () = msg_send![&picker, setAccessibilityLabel: &*label];
        picker.sizeToFit();
        let alert = NSAlert::new(mtm);
        alert.setMessageText(&NSString::from_str(title));
        alert.setAccessoryView(Some(&picker));
        alert.addButtonWithTitle(&NSString::from_str("Apply"));
        alert.addButtonWithTitle(&NSString::from_str("Cancel"));
        alert.layout();
        alert.window().makeFirstResponder(Some(&picker));
        if alert.runModal() != NSAlertFirstButtonReturn {
            return Ok(None);
        }
        alert.window().makeFirstResponder(None);
        merge_date(timestamp, &picker.dateValue(), field, &calendar).map(Some)
    }
}

fn merge_date(
    timestamp: u64,
    selected: &NSDate,
    field: DateField,
    calendar: &NSCalendar,
) -> Result<(u64, [String; 2]), &'static str> {
    unsafe {
        let original = NSDate::dateWithTimeIntervalSince1970(
            (timestamp / 10_000_000) as f64 - 11_644_473_600.0,
        );
        let units = NSCalendarUnit::Era
            | NSCalendarUnit::Year
            | NSCalendarUnit::Month
            | NSCalendarUnit::Day
            | NSCalendarUnit::Hour
            | NSCalendarUnit::Minute
            | NSCalendarUnit::Second;
        let components = calendar.components_fromDate(units, &original);
        let changed = calendar.components_fromDate(units, selected);
        match field {
            DateField::Date => {
                components.setEra(changed.era());
                components.setYear(changed.year());
                components.setMonth(changed.month());
                components.setDay(changed.day());
            }
            DateField::Time => {
                components.setHour(changed.hour());
                components.setMinute(changed.minute());
            }
        }
        let date = calendar
            .dateFromComponents(&components)
            .ok_or(crate::DATE_UNCHOSEN)?;
        let seconds = date.timeIntervalSince1970() + 11_644_473_600.0;
        let outside_range = crate::DATE_OUT_OF_RANGE;
        if !seconds.is_finite() || seconds < 0.0 || seconds > (u64::MAX / 10_000_000) as f64 {
            return Err(outside_range);
        }
        // NSDate cannot retain FILETIME's subsecond precision at this epoch distance.
        let updated = (seconds.round() as u64)
            .checked_mul(10_000_000)
            .and_then(|value| value.checked_add(timestamp % 10_000_000))
            .ok_or(outside_range)?;
        Ok((updated, labels(&date, calendar)))
    }
}

/// `date` as a page's title shows it: the long date and the short time.
fn labels(date: &NSDate, calendar: &NSCalendar) -> [String; 2] {
    unsafe {
        let formatter = NSDateFormatter::new();
        formatter.setCalendar(Some(calendar));
        formatter.setTimeZone(Some(&calendar.timeZone()));
        formatter.setDateStyle(NSDateFormatterStyle::NSDateFormatterFullStyle);
        let date_text = formatter.stringFromDate(date).to_string();
        formatter.setDateStyle(NSDateFormatterStyle::NSDateFormatterNoStyle);
        formatter.setTimeStyle(NSDateFormatterStyle::NSDateFormatterShortStyle);
        [date_text, formatter.stringFromDate(date).to_string()]
    }
}

/// FILETIME as a new page's title shows it: the long date and the short time.
pub fn date_text(filetime: u64) -> [String; 2] {
    unsafe {
        let date = NSDate::dateWithTimeIntervalSince1970(
            (filetime / 10_000_000) as f64 - 11_644_473_600.0,
        );
        labels(&date, &NSCalendar::currentCalendar())
    }
}

/// The account's full name, which OneNote's author fields take from Office's user name.
pub fn user_name() -> String {
    unsafe { objc2_foundation::NSFullUserName().as_ref() }.to_string()
}

/// FILETIME as the system's short date, as OneNote labels a conflict page.
pub fn short_date(filetime: u64) -> String {
    unsafe {
        let date = NSDate::dateWithTimeIntervalSince1970(
            (filetime / 10_000_000) as f64 - 11_644_473_600.0,
        );
        let formatter = NSDateFormatter::new();
        formatter.setDateStyle(NSDateFormatterStyle::NSDateFormatterShortStyle);
        formatter.setTimeStyle(NSDateFormatterStyle::NSDateFormatterNoStyle);
        formatter.stringFromDate(&date).to_string()
    }
}

#[cfg(test)]
mod date_tests {
    use super::*;
    use objc2_foundation::{NSCalendarIdentifierGregorian, NSTimeZone};

    #[test]
    fn calendar_changes_preserve_hidden_components_and_filetime_fraction() {
        unsafe {
            let calendar = NSCalendar::initWithCalendarIdentifier(
                NSCalendar::alloc(),
                NSCalendarIdentifierGregorian,
            )
            .unwrap();
            calendar.setTimeZone(
                &NSTimeZone::timeZoneWithName(&NSString::from_str("America/Los_Angeles")).unwrap(),
            );
            for (original, selected, field, expected) in [
                (
                    1_788_786_864_u64,
                    1_789_023_599.0,
                    DateField::Date,
                    1_788_959_664_u64,
                ),
                (1_788_786_864, 978_413_702.0, DateField::Time, 1_788_842_124),
                (
                    1_709_259_645,
                    1_740_772_800.0,
                    DateField::Date,
                    1_740_795_645,
                ),
                (
                    1_772_879_424,
                    1_772_996_400.0,
                    DateField::Date,
                    1_772_965_824,
                ),
            ] {
                let ticks = (original + 11_644_473_600) * 10_000_000 + 9_123_456;
                let (updated, text) = merge_date(
                    ticks,
                    &NSDate::dateWithTimeIntervalSince1970(selected),
                    field,
                    &calendar,
                )
                .unwrap();
                assert_eq!(
                    updated,
                    (expected + 11_644_473_600) * 10_000_000 + 9_123_456
                );
                assert!(text.iter().all(|value| !value.is_empty()));
            }
        }
    }
}

#[cfg(feature = "wgpu")]
pub fn configure_presentation(surface: &wgpu::Surface<'_>) {
    MainThreadMarker::new().expect("Layer presentation belongs to the main thread");
    // Retain wgpu's surface ownership while synchronizing presentation with AppKit resize transactions.
    if let Some(surface) = unsafe { surface.as_hal::<wgpu::hal::api::Metal>() } {
        let layer = surface.render_layer().lock();
        layer.setPresentsWithTransaction(true);
        // Over the backdrop's layer, which AppKit orders after it.
        layer.setZPosition(1.0);
    }
}

/// Commits a frame presented with the transaction: winit redraws after Core Animation's
/// commit observer, so otherwise the frame waits for the next event. A live resize leaves
/// the commit to AppKit, which pairs the frame with the window's new size.
#[cfg(feature = "wgpu")]
pub fn commit_presentation(window: &Window) {
    let resizing: bool = unsafe { msg_send![&ns_window(window), inLiveResize] };
    if !resizing {
        let class = AnyClass::get("CATransaction").expect("QuartzCore is linked");
        let _: () = unsafe { msg_send![class, flush] };
    }
}

/// The insertion point's colour and selected text's fill with and without keyboard focus,
/// linear RGBA, in `window`'s appearance.
pub fn text_colors(window: &Window) -> [[f32; 4]; 3] {
    let class = AnyClass::get("NSAppearance").expect("AppKit is linked");
    unsafe {
        let appearance: Retained<AnyObject> = msg_send_id![&ns_window(window), effectiveAppearance];
        // System colours resolve in the thread's appearance, which outside drawing does
        // not follow the window's.
        let previous: Option<Retained<AnyObject>> = msg_send_id![class, currentAppearance];
        let _: () = msg_send![class, setCurrentAppearance: &*appearance];
        let space = NSColorSpace::sRGBColorSpace();
        let colors = [
            NSColor::textInsertionPointColor(),
            NSColor::selectedTextBackgroundColor(),
            NSColor::unemphasizedSelectedTextBackgroundColor(),
        ]
        .map(|color| {
            let color = color
                .colorUsingColorSpace(&space)
                .expect("System text colors convert to sRGB");
            let [r, g, b] = [
                color.redComponent(),
                color.greenComponent(),
                color.blueComponent(),
            ]
            .map(|value| {
                let value = value as f32;
                if value <= 0.04045 {
                    value / 12.92
                } else {
                    ((value + 0.055) / 1.055).powf(2.4)
                }
            });
            [r, g, b, color.alphaComponent() as f32]
        });
        let _: () = msg_send![class, setCurrentAppearance: previous.as_deref()];
        colors
    }
}

declare_class!(
    struct CanvasApplication;

    unsafe impl ClassType for CanvasApplication {
        type Super = NSApplication;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "SnowboundApplication";
    }

    impl DeclaredClass for CanvasApplication {
        type Ivars = ();
    }

    unsafe impl CanvasApplication {
        #[method_id(init)]
        fn init(this: Allocated<Self>) -> Retained<Self> {
            let this = this.set_ivars(());
            unsafe { msg_send_id![super(this), init] }
        }

        #[method(terminate:)]
        fn terminate(&self, _sender: Option<&AnyObject>) {
            if let Some(proxy) = QUIT.get() { let _ = proxy.send_event(crate::UserEvent::Quit); }
        }

        #[method(choose:)]
        fn choose(&self, sender: &NSMenuItem) {
            let tag = usize::try_from(unsafe { sender.tag() }).ok();
            if let (Some(proxy), Some(choice)) = (QUIT.get(), tag.and_then(|tag| commands::choices().nth(tag))) {
                let _ = proxy.send_event(crate::UserEvent::Choose(choice));
            }
        }

        #[method(validateMenuItem:)]
        fn validate_menu_item(&self, item: &NSMenuItem) -> objc2::runtime::Bool {
            if unsafe { item.action() } != Some(sel!(choose:)) {
                // AppKit's own items validate as NSApplication does.
                return objc2::runtime::Bool::new(
                    !NSApplication::class().responds_to(sel!(validateMenuItem:))
                        || unsafe { msg_send![super(self), validateMenuItem: item] },
                );
            }
            let status = usize::try_from(unsafe { item.tag() }).ok().and_then(|tag| {
                STATUSES.with_borrow(|statuses| statuses.get(tag).copied())
            }).unwrap_or_default();
            unsafe { item.setState(isize::from(status.checked == Some(true))) };
            objc2::runtime::Bool::new(status.enabled)
        }

        // NSAlert consumes Escape before it reaches cancelOperation:, and 10.6's date
        // picker consumes Return before it reaches the default button.
        #[method(sendEvent:)]
        fn send_event(&self, event: &NSEvent) {
            unsafe {
                let key = (event.r#type() == NSEventType::KeyDown)
                    .then(|| event.charactersIgnoringModifiers())
                    .flatten()
                    .map(|text| text.to_string());
                let modal = self.modalWindow();
                match (key.as_deref(), &modal) {
                    (Some("\u{1b}"), Some(_)) => self.stopModal(),
                    (Some("\r" | "\u{3}"), Some(modal)) if {
                        let responder: *mut AnyObject = msg_send![modal, firstResponder];
                        !responder.is_null()
                            && msg_send![responder, isKindOfClass: NSDatePicker::class()]
                    } => {
                        let cell: *mut AnyObject = msg_send![modal, defaultButtonCell];
                        let _: () = msg_send![cell, performClick: std::ptr::null::<AnyObject>()];
                    }
                    _ => {
                        let _: () = msg_send![super(self), sendEvent: event];
                    }
                }
            }
        }
    }
);

/// A `headless` application takes no Dock icon and never activates.
pub fn event_loop(headless: bool) -> Result<EventLoop<crate::UserEvent>, EventLoopError> {
    use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
    MainThreadMarker::new().expect("AppKit must start on the main thread");
    // Create the application's own subclass before Winit obtains the singleton.
    let _: Retained<CanvasApplication> =
        unsafe { msg_send_id![CanvasApplication::class(), sharedApplication] };
    let mut builder = EventLoop::with_user_event();
    builder.with_default_menu(false);
    if headless {
        builder
            .with_activation_policy(ActivationPolicy::Prohibited)
            .with_activate_ignoring_other_apps(false);
    }
    let event_loop = builder.build()?;
    QUIT.set(event_loop.create_proxy())
        .expect("Only one application event loop is created");
    Ok(event_loop)
}

/// Replaces Winit's application menu with the menu bar, whose items the table validates.
pub fn install_menu() {
    let mtm = MainThreadMarker::new().expect("Menus belong to the main thread");
    let app = NSApplication::sharedApplication(mtm);
    let menus = crate::menubar::build(mtm, &app);
    app.setMainMenu(Some(&menus.bar));
    // The bar holds these for as long as the application runs.
    unsafe {
        app.setServicesMenu(Some(&menus.services));
        app.setWindowsMenu(Some(&menus.window));
        app.setHelpMenu(Some(&menus.help));
    }
    TAGS_MENU.set(Some(menus.tags));
}

/// Lists `tags` in the menu bar's Tags menu.
pub fn update_tag_menu(tags: &[canvas::editor::NoteTag]) {
    let mtm = MainThreadMarker::new().expect("Menus belong to the main thread");
    let app = NSApplication::sharedApplication(mtm);
    TAGS_MENU.with_borrow(|menu| {
        if let Some(menu) = menu {
            crate::menubar::tags(mtm, &app, menu, tags);
        }
    });
}

/// The table's statuses, in `commands::choices` order, which the menu bar's items show.
pub fn update_menu(statuses: impl FnOnce() -> Vec<commands::Status>) {
    STATUSES.set(statuses());
}

/// Opens `target`, a folder or a link's URL, as Finder would.
pub fn reveal(target: impl AsRef<std::ffi::OsStr>) {
    let target = target.as_ref();
    if let Err(error) = std::process::Command::new("open").arg(target).spawn() {
        eprintln!("Cannot open {}: {error}", target.display());
    }
}

/// Asks whether to go ahead with `action`, offering `cancel` first.
pub fn confirm(message: &str, detail: &str, cancel: &str, action: &str) -> bool {
    let mtm = MainThreadMarker::new().expect("Window events run on the main thread");
    // The alert and its strings stay on AppKit's main thread for the modal call.
    unsafe {
        let alert = NSAlert::new(mtm);
        alert.setMessageText(&NSString::from_str(message));
        alert.setInformativeText(&NSString::from_str(detail));
        alert.addButtonWithTitle(&NSString::from_str(cancel));
        alert.addButtonWithTitle(&NSString::from_str(action));
        alert.runModal() == NSAlertSecondButtonReturn
    }
}

/// End AppKit preedit after a canvas action commits or cancels the composition.
pub fn clear_marked_text(window: &Window) {
    MainThreadMarker::new().expect("Text input belongs to the main thread");
    let RawWindowHandle::AppKit(handle) =
        window.window_handle().expect("Live AppKit window").as_raw()
    else {
        unreachable!()
    };
    // Winit owns this view and implements the NSTextInputClient selectors.
    unsafe {
        let view = &*handle.ns_view.as_ptr().cast::<AnyObject>();
        let marked: bool = msg_send![view, hasMarkedText];
        if marked {
            let _: () = msg_send![view, unmarkText];
        }
    }
}

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn TISCopyCurrentKeyboardInputSource() -> *mut AnyObject;
    fn TISGetInputSourceProperty<'a>(
        source: &'a AnyObject,
        key: &NSString,
    ) -> Option<&'a AnyObject>;
    static kTISPropertyInputSourceLanguages: &'static NSString;
}

/// The current keyboard input source's primary language as a BCP-47 tag, empty if it has none.
pub fn input_language() -> String {
    MainThreadMarker::new().expect("Text Input Sources belong to the main thread");
    // Input sources are CFTypes, released as Objective-C objects; languages is an NSArray.
    unsafe {
        let Some(source) = Retained::from_raw(TISCopyCurrentKeyboardInputSource()) else {
            return String::new();
        };
        let language: Option<Retained<NSString>> =
            TISGetInputSourceProperty(&source, kTISPropertyInputSourceLanguages)
                .and_then(|languages| msg_send_id![languages, firstObject]);
        language.map_or_else(String::new, |language| language.to_string())
    }
}

#[cfg(test)]
mod tests {
    use crate::library::{Login, Mount};

    #[link(name = "Security", kind = "framework")]
    unsafe extern "C" {
        fn SecKeychainCreate(
            path: *const std::ffi::c_char,
            password_length: u32,
            password: *const u8,
            prompt: bool,
            access: *const std::ffi::c_void,
            keychain: *mut *const std::ffi::c_void,
        ) -> i32;
        fn SecKeychainDelete(keychain: *const std::ffi::c_void) -> i32;
    }

    /// A kept password reads back for the server whatever its port, and keeping another
    /// replaces it; in a keychain of the test's own, never the user's.
    #[test]
    #[ignore = "a rebuilt test binary may make the system ask on screen to allow keychain access"]
    fn logins_kept_in_a_keychain_read_back() {
        let path = std::env::temp_dir().join(format!("snowbound-{}.keychain", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let name = std::ffi::CString::new(path.to_string_lossy().as_bytes()).unwrap();
        let mut keychain = std::ptr::null();
        let lock = b"test keychain";
        let status = unsafe {
            SecKeychainCreate(
                name.as_ptr(),
                lock.len() as u32,
                lock.as_ptr(),
                false,
                std::ptr::null(),
                &mut keychain,
            )
        };
        assert_eq!(status, 0);
        let mount = Mount::from_address("smb://amy@nas.local:1445/notes").unwrap();
        let login = |password: &str| Login {
            user: "amy".into(),
            password: password.into(),
            domain: String::new(),
        };
        assert!(super::smb_login_in(keychain, &mount).is_err());
        super::save_login_in(keychain, &mount, &login("first")).unwrap();
        super::save_login_in(keychain, &mount, &login("second")).unwrap();
        let read = super::smb_login_in(keychain, &mount);
        unsafe { SecKeychainDelete(keychain) };
        let _ = std::fs::remove_file(&path);
        assert_eq!(read.unwrap().password, "second");
    }

    /// A server the Finder mounted by its Bonjour service resolves to where it answers.
    #[test]
    #[ignore = "requires SNOWBOUND_TEST_BONJOUR naming an SMB service on this network"]
    fn bonjour_services_resolve_to_their_host() {
        let instance = std::env::var("SNOWBOUND_TEST_BONJOUR").unwrap();
        let endpoint = super::bonjour_endpoint(&instance).unwrap();
        assert!(endpoint.contains(".local:"), "{endpoint}");
        assert_eq!(super::bonjour_endpoint("snowbound-no-such-service"), None);
    }
}
