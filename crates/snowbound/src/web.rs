//! The browser: one canvas, its input through `web/glue.js`, and notebooks in the files
//! `notebook::fs` keeps, which the glue loads from IndexedDB and writes back. The window and
//! event loop are stand-ins with the calls `State` makes of winit's; menus are the kit's own,
//! as on Linux; the browser's chords stay the browser's. See arc/platforms.md.

use crate::{State, UserEvent, commands, page, settings};
use canvas::date::DateField;
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    error::Error,
    marker::PhantomData,
    ops::RangeInclusive,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use ui::Ui;
use wasm_bindgen::prelude::*;
use web_time::Instant;
use winit::{
    dpi::{PhysicalPosition, PhysicalSize, Size},
    event::{Ime, MouseButton},
    event_loop::EventLoopClosed,
    keyboard::{Key, ModifiersState, NamedKey},
    window::{CursorIcon, ResizeDirection, Theme, WindowAttributes},
};

#[wasm_bindgen(module = "/web/glue.js")]
extern "C" {
    fn attach(module: JsValue);
    /// The files the browser kept: `[path, bytes or null for a folder, modified]`.
    #[wasm_bindgen(js_name = loadFiles, catch)]
    async fn load_files() -> Result<js_sys::Array, JsValue>;
    #[wasm_bindgen(js_name = requestFrame)]
    fn request_frame();
    #[wasm_bindgen(js_name = wakeIn)]
    fn wake_in(ms: f64);
    #[wasm_bindgen(js_name = setCursor)]
    fn set_cursor(name: &str);
    #[wasm_bindgen(js_name = setTitle)]
    fn set_title(title: &str);
    #[wasm_bindgen(js_name = placeInput)]
    fn place_input(x: f32, y: f32, height: f32, text: bool);
    #[wasm_bindgen(js_name = writeClipboard)]
    fn write_clipboard(text: &str);
    /// Hands `bytes` to the browser to save as `name`, of MIME type `kind`.
    pub fn download(name: &str, bytes: &[u8], kind: &str);
    #[wasm_bindgen(js_name = pickFiles)]
    fn pick_files(purpose: &str, accept: &str);
    /// Asks for a notebook: a folder of the user's where the browser can be given one
    /// (`mounted`), else files to copy in (`files`).
    #[wasm_bindgen(js_name = pickNotebook)]
    fn pick_notebook_files();
    /// The folders of the user's the browser was given before and may still reach:
    /// `[root, files]`, the files as `loadFiles` gives them.
    #[wasm_bindgen(js_name = loadFolders, catch)]
    async fn load_folders() -> Result<js_sys::Array, JsValue>;
    #[wasm_bindgen(js_name = dateText)]
    fn date_strings(ms: f64) -> Vec<String>;
    #[wasm_bindgen(js_name = shortDate)]
    fn short_date_string(ms: f64) -> String;
    #[wasm_bindgen(js_name = askConfirm)]
    fn ask_confirm(message: &str) -> bool;
    #[wasm_bindgen(js_name = askText)]
    fn ask_text(message: &str, value: &str) -> Option<String>;
    #[wasm_bindgen(js_name = tell)]
    fn tell(message: &str);
    #[wasm_bindgen(js_name = openLink)]
    fn open_link(url: &str);
    /// Writes changes out: `[path]` removed, `[path, null]` a folder, and `[path, length,
    /// [[offset, bytes], ...]]` a file's new length and the ranges that changed; with a
    /// fourth `true`, a section committed under a folder of the user's, written only where
    /// nothing else wrote the file since it was read (`refreshed` answers).
    /// Applies an accessibility tree update to the DOM mirror (`mirrored`).
    #[wasm_bindgen(js_name = mirrorTree)]
    fn mirror_tree(update: &js_sys::Object);
    /// Fetches `fonts/{name}` for `font_arrived`.
    #[wasm_bindgen(js_name = fetchFont)]
    fn fetch_font(name: &str);
    #[wasm_bindgen(js_name = storeFiles)]
    fn store_files(changes: js_sys::Array);
}

/// Where the browser keeps its notebooks, which a first visit makes one in.
const NOTEBOOKS: &str = "/Notebooks";
const CACHE: &str = "/Cache";
/// The metric-compatible faces `index.html` fetched, which only this page load holds.
const FONTS: &str = "/Fonts";
const SETTINGS: &str = "/Settings";
/// Where folders of the user's given to the browser are mirrored (`glue.js` mounts them).
const FOLDERS: &str = "/Folders";
/// Files chosen to insert, open or paste, kept where the page links them from.
const CHOSEN: &str = "/Chosen";
/// Faces for what the bundled ones lack, beside them in `fonts/`, fetched the first time a
/// page holds a character in their ranges: Noto's, by script.
const FALLBACKS: [(&str, &[RangeInclusive<u32>]); 6] = [
    (
        "NotoSansArabic.ttf",
        &[
            0x600..=0x6ff,
            0x750..=0x77f,
            0x8a0..=0x8ff,
            0xfb50..=0xfdff,
            0xfe70..=0xfeff,
        ],
    ),
    ("NotoSansHebrew.ttf", &[0x590..=0x5ff, 0xfb1d..=0xfb4f]),
    ("NotoSansDevanagari.ttf", &[0x900..=0x97f, 0xa8e0..=0xa8ff]),
    ("NotoSansThai.ttf", &[0xe00..=0xe7f]),
    (
        "NotoSansSymbols2-Regular.otf",
        &[
            0x2190..=0x21ff,
            0x2300..=0x23ff,
            0x25a0..=0x27bf,
            0x2b00..=0x2bff,
        ],
    ),
    (
        "NotoSansCJK.otf",
        &[
            0x2e80..=0x9fff,
            0xac00..=0xd7af,
            0xf900..=0xfaff,
            0xff00..=0xffef,
        ],
    ),
];
/// How long changed files wait to be written out, so a burst of edits writes once.
const STORE_AFTER: Duration = Duration::from_millis(500);

/// What the page's input brought, waiting for the next frame.
enum Input {
    Ui(ui::Event),
    /// Zooms the page by the factor about a point in the window.
    Pinch(f32, [f32; 2]),
    /// The canvas's device pixels and device pixels per CSS pixel.
    Resize([u32; 2], f32),
    Focus(bool),
}

/// The browser window's state as the canvas reports it.
struct Host {
    size: PhysicalSize<u32>,
    ratio: f64,
    focused: bool,
    title: String,
    theme: Theme,
    redraw: bool,
}

thread_local! {
    static HOST: RefCell<Host> = const {
        RefCell::new(Host {
            size: PhysicalSize::new(1, 1),
            ratio: 1.0,
            focused: true,
            title: String::new(),
            theme: Theme::Light,
            redraw: true,
        })
    };
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
    static INPUT: RefCell<Vec<Input>> = const { RefCell::new(Vec::new()) };
    static EVENTS: RefCell<VecDeque<UserEvent>> = const { RefCell::new(VecDeque::new()) };
    /// Work deferred to after the frame, as threads run it elsewhere.
    static LATER: RefCell<Vec<Box<dyn FnOnce()>>> = const { RefCell::new(Vec::new()) };
    /// The fallback faces asked for, by bit, and the page whose text was last looked through.
    static FETCHED: Cell<u32> = const { Cell::new(0) };
    static LOOKED: Cell<Option<onestore::ExGuid>> = const { Cell::new(None) };
    /// Text arrived from the keyboard, input methods or the clipboard since the last look.
    static TYPED: Cell<bool> = const { Cell::new(false) };
    /// A screen reader asked for the accessibility mirror.
    static ACCESSIBLE: Cell<bool> = const { Cell::new(false) };
    /// Input waits a frame for what a chord opened.
    static HOLDING: Cell<bool> = const { Cell::new(false) };
    /// Modifier bits as the page's input last reported them.
    static HELD: Cell<u8> = const { Cell::new(0) };
    /// Whether Command, not Control, takes the editing chords.
    static MAC: Cell<bool> = const { Cell::new(false) };
    static PASTED: RefCell<Pasted> = RefCell::new(Pasted::default());
    /// When changed files are next written out.
    static STORE_DUE: Cell<Option<Instant>> = const { Cell::new(None) };
    static LANGUAGE: RefCell<String> = const { RefCell::new(String::new()) };
}

/// The page's canvas, which frames are drawn into.
pub fn canvas() -> web_sys::HtmlCanvasElement {
    use wasm_bindgen::JsCast;
    web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.get_element_by_id("page"))
        .and_then(|canvas| canvas.dyn_into().ok())
        .expect("index.html has a canvas called page")
}

fn host<T>(act: impl FnOnce(&mut Host) -> T) -> T {
    HOST.with_borrow_mut(act)
}

fn report(error: impl std::fmt::Display) {
    web_sys::console::error_1(&error.to_string().into());
}

/// Runs `work` once the frame under way is done, as a thread would run it beside the frame.
pub fn defer(work: impl FnOnce() + 'static) {
    LATER.with_borrow_mut(|later| later.push(Box::new(work)));
    wake_in(0.0);
}

/// The event loop `State` opens its window in.
pub struct ActiveEventLoop;

impl ActiveEventLoop {
    pub fn create_window(&self, attributes: WindowAttributes) -> Result<Window, Box<dyn Error>> {
        let window = Window(());
        window.set_title(&attributes.title);
        Ok(window)
    }
}

/// The page's canvas as a window.
pub struct Window(());

impl Window {
    pub fn request_redraw(&self) {
        host(|host| host.redraw = true);
        request_frame();
    }

    pub fn scale_factor(&self) -> f64 {
        host(|host| host.ratio)
    }

    pub fn inner_size(&self) -> PhysicalSize<u32> {
        host(|host| host.size)
    }

    pub fn request_inner_size(&self, _: impl Into<Size>) -> Option<PhysicalSize<u32>> {
        None
    }

    pub fn title(&self) -> String {
        host(|host| host.title.clone())
    }

    pub fn set_title(&self, title: &str) {
        set_title(title);
        host(|host| host.title = title.to_owned());
    }

    /// The colour scheme Options chooses is `State`'s; the page follows the browser's.
    pub fn set_theme(&self, _: Option<Theme>) {}

    pub fn has_focus(&self) -> bool {
        host(|host| host.focused)
    }

    pub fn focus_window(&self) {}

    pub fn set_visible(&self, _: bool) {}

    pub fn set_minimized(&self, _: bool) {}

    /// The browser moves and sizes its own window.
    pub fn drag_window(&self) -> Result<(), ()> {
        Err(())
    }

    pub fn drag_resize_window(&self, _: ResizeDirection) -> Result<(), ()> {
        Err(())
    }

    pub fn set_cursor(&self, cursor: impl Into<winit::window::Cursor>) {
        set_cursor(match cursor.into() {
            winit::window::Cursor::Icon(icon) => icon.name(),
            winit::window::Cursor::Custom(_) => "default",
        });
    }

    pub fn set_ime_allowed(&self, _: bool) {}

    /// Puts the text area at the caret, so an input method's candidates show beside it.
    pub fn set_ime_cursor_area(
        &self,
        position: impl Into<winit::dpi::Position>,
        size: impl Into<Size>,
    ) {
        let ratio = self.scale_factor();
        let position: PhysicalPosition<f64> = position.into().to_physical(ratio);
        let size: PhysicalSize<f64> = size.into().to_physical(ratio);
        place_input(
            (position.x / ratio) as f32,
            (position.y / ratio) as f32,
            ((size.height / ratio) as f32).max(1.0),
            true,
        );
    }

    pub fn pre_present_notify(&self) {}
}

/// Sends events to the page's loop, from wherever `State` hands it.
pub struct EventLoopProxy<T>(PhantomData<fn(T)>);

impl<T> Clone for EventLoopProxy<T> {
    fn clone(&self) -> Self {
        Self(PhantomData)
    }
}

impl EventLoopProxy<UserEvent> {
    pub fn send_event(&self, event: UserEvent) -> Result<(), EventLoopClosed<UserEvent>> {
        EVENTS.with_borrow_mut(|events| events.push_back(event));
        request_frame();
        Ok(())
    }
}

/// Assistive technology's view of the window: a DOM of ARIA roles the glue keeps beside the
/// canvas once a screen reader asks for it (`accessibility`), as AccessKit has no adapter
/// for the web.
pub struct AccessAdapter;

impl AccessAdapter {
    pub fn with_event_loop_proxy(
        _: &ActiveEventLoop,
        _: &Window,
        _: EventLoopProxy<UserEvent>,
    ) -> Self {
        Self
    }

    pub fn update_if_active(&mut self, update: impl FnOnce() -> accesskit::TreeUpdate) {
        if ACCESSIBLE.get() {
            mirror_tree(&mirrored(&update()));
        }
    }
}

/// `update` as the glue mirrors it: the tree, its focus and root where it has one, and each
/// node as `[id, role, name, value, children, bounds, grafted tree, disabled, toggled]`.
fn mirrored(update: &accesskit::TreeUpdate) -> js_sys::Object {
    let tree = |id: accesskit::TreeId| JsValue::from_str(&id.0.to_string());
    let nodes: js_sys::Array = update
        .nodes
        .iter()
        .map(|(id, node)| {
            let children: js_sys::Array = node
                .children()
                .iter()
                .map(|child| JsValue::from_f64(child.0 as f64))
                .collect();
            let bounds = node.bounds().map_or(JsValue::NULL, |rect| {
                js_sys::Array::of4(
                    &rect.x0.into(),
                    &rect.y0.into(),
                    &rect.x1.into(),
                    &rect.y1.into(),
                )
                .into()
            });
            let text = |text: Option<&str>| text.map_or(JsValue::NULL, JsValue::from_str);
            [
                JsValue::from_f64(id.0 as f64),
                JsValue::from_str(&format!("{:?}", node.role())),
                text(node.label()),
                text(node.value()),
                children.into(),
                bounds,
                node.tree_id().map_or(JsValue::NULL, tree),
                node.is_disabled().into(),
                node.toggled().map_or(JsValue::NULL, |toggled| {
                    (toggled == accesskit::Toggled::True).into()
                }),
            ]
            .into_iter()
            .collect::<js_sys::Array>()
        })
        .collect();
    let mirrored = js_sys::Object::new();
    let set = |key: &str, value: &JsValue| {
        let _ = js_sys::Reflect::set(&mirrored, &key.into(), value);
    };
    set("tree", &tree(update.tree_id));
    set("focus", &JsValue::from_f64(update.focus.0 as f64));
    set(
        "root",
        &update
            .tree
            .as_ref()
            .map_or(JsValue::NULL, |info| JsValue::from_f64(info.root.0 as f64)),
    );
    set("nodes", &nodes);
    mirrored
}

/// A screen reader turned the mirror on, or off: the whole tree is sent afresh.
#[wasm_bindgen]
pub fn accessibility(on: bool) {
    ACCESSIBLE.set(on);
    send(UserEvent::Then(Box::new(|state| {
        state.deactivate_accessibility();
        state.update_accessibility(true)
    })));
}

/// Assistive technology acted on node `node` of tree `tree`: 0 clicks it, 1 focuses it.
#[wasm_bindgen]
pub fn access(tree: String, node: f64, action: u8) {
    let Ok(tree) = tree.parse() else {
        return;
    };
    let request = accesskit::ActionRequest {
        action: match action {
            1 => accesskit::Action::Focus,
            _ => accesskit::Action::Click,
        },
        target_tree: accesskit::TreeId(tree),
        target_node: accesskit::NodeId(node as u64),
        data: None,
    };
    send(UserEvent::Then(Box::new(move |state| {
        state.access_action(request)
    })));
}

/// The browser's clipboard: text out through `navigator.clipboard`, and in through the page's
/// paste event, which the glue keeps here before the paste runs.
pub struct Clipboard;

#[derive(Default)]
struct Pasted {
    text: Option<String>,
    html: Option<String>,
    files: Vec<PathBuf>,
}

impl Clipboard {
    pub fn new(_: &Window) -> Result<Self, Box<dyn Error>> {
        Ok(Self)
    }

    pub fn set_text(&mut self, text: String) -> Result<(), Box<dyn Error>> {
        write_clipboard(&text);
        Ok(())
    }

    pub fn get_text(&mut self) -> Result<String, Box<dyn Error>> {
        PASTED
            .with_borrow(|pasted| pasted.text.clone())
            .ok_or_else(|| "Nothing is on the clipboard".into())
    }

    pub fn get_files(&mut self) -> Vec<PathBuf> {
        PASTED.with_borrow(|pasted| pasted.files.clone())
    }

    pub fn get_html(&mut self) -> Option<String> {
        PASTED.with_borrow(|pasted| pasted.html.clone())
    }

    pub fn get_picture(&mut self) -> Option<Vec<u8>> {
        None
    }
}

pub const LEADING: f32 = 8.0;
pub const TRAILING: f32 = 8.0;

pub fn window_attributes() -> WindowAttributes {
    WindowAttributes::default()
}

pub fn install_title_bar(_: &Window) {}

pub fn install_text_input(_: &Window) {}

pub fn install_backdrop(_: &Window) -> bool {
    false
}

pub fn install_menu() {}

pub fn update_menu(_: impl FnOnce() -> Vec<commands::Status>) {}

pub fn update_tag_menu(_: &[canvas::editor::NoteTag]) {}

/// The browser draws the tab's title; the toolbar's row is the page's top.
pub fn system_titlebar(_: &Window) -> bool {
    true
}

pub fn represent(_: &Window, _: Option<&Path>) {}

pub fn titlebar(_: Theme) -> Option<[[f32; 4]; 2]> {
    None
}

pub fn over_backdrop(theme: ui::Theme, _: Theme) -> ui::Theme {
    theme.over_backdrop()
}

/// The kit's own menus.
pub fn menu(_: Theme) -> Option<ui::Menu> {
    None
}

pub fn follow_appearance(_: &Window, _: Theme) {}

pub fn appearance(_: &Window) -> Theme {
    host(|host| host.theme)
}

pub fn corner_radius(_: &Window) -> f32 {
    0.0
}

pub fn cuts_corners() -> bool {
    false
}

pub fn window_controls(_: &mut Ui, _: &Window) {}

pub fn resize_direction(_: &Window, _: [f32; 2]) -> Option<ResizeDirection> {
    None
}

pub fn set_min_size(_: &Window, _: [f32; 2]) {}

pub fn zoom(_: &Window) {}

pub fn move_cursor() -> CursorIcon {
    CursorIcon::Move
}

pub fn cover_border_line(_: &mut Ui, _: f32) {}

/// The interface in Carlito as Calibri, the page's default face, as the browser has no
/// system font to offer.
pub fn system_interface(ui: &mut Ui) {
    ui.set_system_font("Calibri");
}

pub fn resize_grip(_: &mut Ui, _: &Window, _: [f32; 2]) {}

pub fn clear_marked_text(_: &Window) {}

pub fn configure_presentation(_: &wgpu::Surface<'_>) {}

pub fn commit_presentation(_: &Window) {}

pub fn double_click_interval() -> Duration {
    Duration::from_millis(500)
}

/// Pages fall back through the fonts the browser module carries.
pub fn symbol_fonts() -> Vec<String> {
    Vec::new()
}

pub fn file_icon(_: &Path) -> Option<Vec<u8>> {
    None
}

/// The browser has no file manager to show a file in.
pub const SHOW_FILE: &str = "Download";

/// Downloads `file`, the nearest the browser comes to showing it.
pub fn show_file(file: &Path) {
    open_file(file);
}

/// Downloads a copy of the file at `path` from the browser's files.
pub fn open_file(path: &Path) {
    match notebook::fs::read(path) {
        Ok(bytes) => download(
            &crate::library::file_name(path),
            &bytes,
            "application/octet-stream",
        ),
        Err(error) => alert("Couldn't open the file", &error.to_string()),
    }
}

/// Opens a link's URL in a new tab; a folder of the browser's files has nothing to show.
pub fn reveal(target: impl AsRef<std::ffi::OsStr>) {
    let target = target.as_ref().to_string_lossy();
    if target.contains("://") || target.starts_with("mailto:") {
        open_link(&target);
    }
}

pub fn cache_dir() -> Option<PathBuf> {
    Some(CACHE.into())
}

pub fn settings_dir() -> Option<PathBuf> {
    Some(SETTINGS.into())
}

pub fn documents_dir() -> Option<PathBuf> {
    Some(NOTEBOOKS.into())
}

/// The browser names no user; edits are stored under the name Options sets.
pub fn user_name() -> String {
    String::new()
}

pub fn input_language() -> String {
    LANGUAGE.with_borrow(Clone::clone)
}

/// Milliseconds since 1970 at FILETIME `filetime`.
fn unix_ms(filetime: u64) -> f64 {
    (filetime / 10_000) as f64 - 11_644_473_600_000.0
}

/// Seconds east of UTC at `unix` seconds since 1970, as the browser's zone has it.
pub fn utc_offset(unix: i64) -> i64 {
    -(js_sys::Date::new(&(unix as f64 * 1e3).into()).get_timezone_offset() as i64) * 60
}

pub fn short_date(filetime: u64) -> String {
    short_date_string(unix_ms(filetime))
}

pub fn date_text(filetime: u64) -> [String; 2] {
    let mut text = date_strings(unix_ms(filetime)).into_iter();
    [
        text.next().unwrap_or_default(),
        text.next().unwrap_or_default(),
    ]
}

/// Asks for the page's date as `YYYY-MM-DD`, or its time as `HH:MM`.
pub fn edit_date(
    timestamp: u64,
    field: DateField,
    title: &str,
) -> Result<Option<(u64, [String; 2])>, &'static str> {
    let ms = unix_ms(timestamp);
    let date = js_sys::Date::new(&ms.into());
    let current = match field {
        DateField::Date => format!(
            "{:04}-{:02}-{:02}",
            date.get_full_year(),
            date.get_month() + 1,
            date.get_date()
        ),
        DateField::Time => format!("{:02}:{:02}", date.get_hours(), date.get_minutes()),
    };
    let Some(answer) = ask_text(title, &current) else {
        return Ok(None);
    };
    let numbers: Vec<u32> = answer
        .split(|c: char| !c.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.parse().ok())
        .collect();
    match (field, numbers.as_slice()) {
        (DateField::Date, &[year, month, day])
            if (1..=12).contains(&month) && (1..=31).contains(&day) =>
        {
            date.set_full_year_with_month_date(year, month as i32 - 1, day as i32);
        }
        (DateField::Time, &[hour, minute]) if hour < 24 && minute < 60 => {
            date.set_hours(hour);
            date.set_minutes(minute);
        }
        _ => return Err(crate::DATE_UNCHOSEN),
    }
    let seconds = (date.get_time() / 1e3).floor();
    let updated = ((seconds as i64 + 11_644_473_600) as u64)
        .checked_mul(10_000_000)
        .and_then(|ticks| ticks.checked_add(timestamp % 10_000_000))
        .ok_or(crate::DATE_OUT_OF_RANGE)?;
    Ok(Some((updated, date_text(updated))))
}

/// Asks for a file to insert; once chosen it goes to the caret, as a dropped file does.
pub fn pick_file(_: &str, types: &[&str]) -> Option<PathBuf> {
    let accept: Vec<String> = types.iter().map(|kind| format!(".{kind}")).collect();
    pick_files("place", &accept.join(","));
    None
}

/// Asks for notebooks, sections or packages to open; they open once read.
pub fn pick_notebook(_: &str) -> Option<PathBuf> {
    pick_notebook_files();
    None
}

/// A new notebook goes in the browser's notebooks, under a name not yet taken; anything else
/// asked a place goes there too, then downloads.
pub fn pick_new(title: &str, name: &str, _: &str, _: Option<&Path>) -> Option<PathBuf> {
    let name = ask_text(title, name)?;
    let name = name.trim().replace(['/', '\\'], " ");
    let folder = Path::new(NOTEBOOKS);
    let _ = notebook::fs::create_dir_all(folder);
    let mut path = folder.join(&name);
    let mut number = 2;
    while notebook::fs::metadata(&path).is_ok() {
        path = folder.join(format!("{name} {number}"));
        number += 1;
    }
    Some(path)
}

pub fn confirm(message: &str, detail: &str, _: &str, _: &str) -> bool {
    ask_confirm(&format!("{message}\n\n{detail}"))
}

pub fn alert(message: &str, detail: &str) {
    tell(&format!("{message}\n\n{detail}"));
}

pub fn inform(message: &str, detail: &str) {
    alert(message, detail);
}

/// What the sync popup says of a notebook in a folder of the user's, which the browser
/// writes without the locks OneNote takes.
pub fn lock_notice(location: &str) -> Option<String> {
    location.starts_with(&format!("{FOLDERS}/")).then(|| {
        "This folder isn’t locked while you edit. Edit each section in one app at a time.".into()
    })
}

/// Servers are reached through a relay in a later phase; none is mounted here.
pub fn smb_mount(_: &Path) -> Option<crate::library::Mount> {
    None
}

pub fn smb_login(_: &crate::library::Mount) -> Result<crate::library::Login, String> {
    Err("Servers aren't reachable from the browser yet.".into())
}

pub fn remember_label() -> Option<&'static str> {
    None
}

pub fn save_login(_: &crate::library::Mount, _: &crate::library::Login) -> Result<(), String> {
    Err("The browser keeps no passwords.".into())
}

/// The embedded SMB client's face, until a WebSocket relay carries SMB to the browser:
/// every server is out of reach.
pub mod smb {
    use std::{convert::Infallible, fmt, io, time::Duration};

    fn unreachable() -> io::Error {
        io::Error::new(io::ErrorKind::Unsupported, Refusal::Unreachable)
    }

    #[derive(Default)]
    pub struct Credentials<'a> {
        pub username: &'a str,
        pub password: &'a str,
        pub domain: &'a str,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct DirectoryEntry {
        pub name: String,
        pub size: u64,
        pub modified: u64,
        pub attributes: u32,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Refusal {
        Unreachable,
        Smb1,
        SignIn,
        NoShare,
        Denied,
        NoFolder,
        Other,
    }

    impl fmt::Display for Refusal {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("Servers aren't reachable from the browser yet")
        }
    }

    impl std::error::Error for Refusal {}

    impl Refusal {
        pub fn of(_: &io::Error) -> Self {
            Self::Unreachable
        }
    }

    pub struct Client(Infallible);

    impl Client {
        pub fn connect(_: &str, _: &str, _: Credentials, _: Duration) -> io::Result<Self> {
            Err(unreachable())
        }

        pub fn read_dir(&self, _: &str, _: usize) -> io::Result<Vec<DirectoryEntry>> {
            match self.0 {}
        }
    }

    pub fn shares(_: &str, _: Credentials, _: Duration) -> io::Result<Vec<String>> {
        Err(unreachable())
    }
}

/// Starts Snowbound on the page's canvas, `index.html` having fetched `fonts` and, where it
/// could, the spelling dictionary's affix and word files. `module` is the module's own
/// exports, which the glue calls.
#[wasm_bindgen]
pub async fn start(
    module: JsValue,
    fonts: Vec<js_sys::Uint8Array>,
    dictionary: Vec<js_sys::Uint8Array>,
) -> Result<(), JsValue> {
    std::panic::set_hook(Box::new(|info| report(info)));
    let window = web_sys::window().ok_or("No window")?;
    let navigator = window.navigator();
    MAC.set(navigator.platform().is_ok_and(|platform| {
        ["Mac", "iPhone", "iPad"]
            .iter()
            .any(|name| platform.contains(name))
    }));
    LANGUAGE.with_borrow_mut(|language| *language = navigator.language().unwrap_or_default());
    let dark = window
        .match_media("(prefers-color-scheme: dark)")
        .ok()
        .flatten()
        .is_some_and(|query| query.matches());
    let ratio = window.device_pixel_ratio();
    let canvas = canvas();
    host(|host| {
        host.theme = if dark { Theme::Dark } else { Theme::Light };
        host.size = PhysicalSize::new(
            ((f64::from(canvas.client_width()) * ratio) as u32).max(1),
            ((f64::from(canvas.client_height()) * ratio) as u32).max(1),
        );
        host.ratio = ratio;
    });
    canvas.set_width(host(|host| host.size.width));
    canvas.set_height(host(|host| host.size.height));
    restore(load_files().await?);
    if let [affix, words] = dictionary.as_slice() {
        notebook::fs::restore("/Dictionaries", notebook::fs::Saved::Directory);
        for (path, file) in [(crate::spell::AFFIX, affix), (crate::spell::WORDS, words)] {
            notebook::fs::restore(path, notebook::fs::Saved::File(file.to_vec(), 0.0));
        }
    }
    for folder in load_folders().await?.iter() {
        let folder = js_sys::Array::from(&folder);
        let root = folder.get(0).as_string().unwrap_or_default();
        notebook::fs::mount(&root);
        restore(folder.get(1).into());
    }
    let state = open(fonts)
        .await
        .map_err(|error| JsValue::from_str(&error.to_string()))?;
    STATE.with_borrow_mut(|slot| *slot = Some(state));
    attach(module);
    request_frame();
    Ok(())
}

/// Puts back the files IndexedDB kept: `[path, bytes or null for a folder, modified]`.
fn restore(files: js_sys::Array) {
    for entry in files.iter() {
        let entry = js_sys::Array::from(&entry);
        let Some(path) = entry.get(0).as_string() else {
            continue;
        };
        let bytes = entry.get(1);
        let saved = if bytes.is_null() {
            notebook::fs::Saved::Directory
        } else {
            notebook::fs::Saved::File(
                js_sys::Uint8Array::new(&bytes).to_vec(),
                entry.get(2).as_f64().unwrap_or(0.0),
            )
        };
        notebook::fs::restore(path, saved);
    }
}

/// Writes the files changed since the last time out to the browser's storage.
fn store() {
    use notebook::fs::Change;
    STORE_DUE.set(None);
    let changes = js_sys::Array::new();
    for (path, change) in notebook::fs::changes() {
        let path = JsValue::from_str(&path.to_string_lossy());
        let entry = match change {
            Change::Removed => js_sys::Array::of1(&path),
            Change::Directory => js_sys::Array::of2(&path, &JsValue::NULL),
            Change::File { length, ranges } => js_sys::Array::of3(
                &path,
                &(length as f64).into(),
                &ranges
                    .into_iter()
                    .map(|(offset, bytes)| {
                        js_sys::Array::of2(
                            &(offset as f64).into(),
                            &js_sys::Uint8Array::from(bytes.as_slice()),
                        )
                    })
                    .collect::<js_sys::Array>(),
            ),
        };
        changes.push(&entry);
    }
    for (path, image) in notebook::fs::committed() {
        changes.push(&js_sys::Array::of4(
            &JsValue::from_str(&path.to_string_lossy()),
            &(image.len() as f64).into(),
            &js_sys::Array::of1(&js_sys::Array::of2(
                &0.into(),
                &js_sys::Uint8Array::from(image.as_slice()),
            )),
            &true.into(),
        ));
    }
    if changes.length() > 0 {
        store_files(changes);
    }
}

/// The window, as `launch` opens it with the notebooks the settings list, making the
/// browser's first notebook on a first visit.
async fn open(fonts: Vec<js_sys::Uint8Array>) -> Result<State, Box<dyn Error>> {
    let cache = PathBuf::from(CACHE);
    for folder in [NOTEBOOKS, CACHE, SETTINGS, CHOSEN] {
        notebook::fs::create_dir_all(folder)?;
    }
    notebook::fs::restore(FONTS, notebook::fs::Saved::Directory);
    let file = settings::default_path();
    let saved = file
        .as_deref()
        .map(settings::Settings::load)
        .unwrap_or_default();
    let first = saved.notebooks.is_empty();
    let input = crate::Input::Notebooks {
        locations: saved.notebooks.clone(),
        current: saved.current.clone(),
    };
    let launch = settings::Launch { file, saved, cache };
    // Where `State` reads them from, kept out of what IndexedDB stores.
    let substitutes: Vec<PathBuf> = fonts
        .iter()
        .enumerate()
        .map(|(number, font)| {
            let path = PathBuf::from(format!("{FONTS}/{number}.ttf"));
            notebook::fs::restore(&path, notebook::fs::Saved::File(font.to_vec(), 0.0));
            path
        })
        .collect();
    let mut state = State::new(
        &ActiveEventLoop,
        EventLoopProxy(PhantomData),
        input,
        &substitutes,
        true,
        launch,
    )
    .await?;
    // The first web build's sections, moved by the glue, or else a notebook of the browser's own.
    if first {
        let [moved, own] =
            ["Web Notebook", "My Notebook"].map(|name| Path::new(NOTEBOOKS).join(name));
        match [&moved, &own]
            .into_iter()
            .find(|root| notebook::fs::metadata(root).is_ok())
        {
            Some(root) => state.open_path(root),
            None => state.create_notebook(own.clone())?,
        }
    }
    Ok(state)
}

fn queue(input: Input) {
    INPUT.with_borrow_mut(|queue| queue.push(input));
    request_frame();
}

/// Whether the page takes the key `key` with modifier bits `held` (Shift, Control, Alt, Meta
/// from the lowest), rather than leaving it to the browser: the browser keeps its own tab
/// and window chords, and pastes through its paste event.
#[wasm_bindgen]
pub fn takes(key: &str, held: u8) -> bool {
    let command = held & if MAC.get() { 8 } else { 2 } != 0;
    if !command {
        return true;
    }
    !matches!(
        key.to_ascii_lowercase().as_str(),
        "v" | "w" | "t" | "n" | "q" | "r" | "l" | "tab"
    )
}

/// Reports modifier bits `held` where they changed.
fn follow_modifiers(held: u8) {
    if HELD.replace(held) == held {
        return;
    }
    let [shift, control, alt, meta] = [1, 2, 4, 8].map(|bit| held & bit != 0);
    let mac = MAC.get();
    let mut state = ModifiersState::empty();
    state.set(ModifiersState::SHIFT, shift);
    state.set(ModifiersState::ALT, alt);
    // The command table's chords are the PC's here; on a Mac, Command is Control.
    state.set(ModifiersState::CONTROL, if mac { meta } else { control });
    state.set(ModifiersState::SUPER, if mac { control } else { meta });
    queue(Input::Ui(ui::Event::Modifiers(state)));
}

/// Pointer `kind` (moved, pressed, released, left) at `x`, `y` in CSS pixels.
#[wasm_bindgen]
pub fn pointer(kind: u8, x: f32, y: f32, button: i16, pressure: f32, held: u8) {
    follow_modifiers(held);
    queue(Input::Ui(ui::Event::Pressure(
        (!pressure.is_nan()).then_some(pressure),
    )));
    let button = match button {
        1 => MouseButton::Middle,
        2 => MouseButton::Right,
        _ => MouseButton::Left,
    };
    queue(Input::Ui(match kind {
        0 => ui::Event::PointerMoved([x, y]),
        1 | 2 => ui::Event::Button {
            button,
            pressed: kind == 1,
            at: Instant::now(),
        },
        _ => ui::Event::PointerLeft,
    }));
}

#[wasm_bindgen]
pub fn key(key: &str, text: Option<String>, held: u8) {
    follow_modifiers(held);
    TYPED.set(TYPED.get() || text.is_some());
    queue(Input::Ui(ui::Event::Key {
        key: logical_key(key),
        text,
    }));
}

#[wasm_bindgen]
pub fn modifiers(held: u8) {
    follow_modifiers(held);
}

#[wasm_bindgen]
pub fn compose(text: String) {
    let end = text.len();
    queue(Input::Ui(ui::Event::Ime(Ime::Preedit(
        text,
        Some((end, end)),
    ))));
}

#[wasm_bindgen]
pub fn commit(text: String) {
    TYPED.set(true);
    queue(Input::Ui(ui::Event::Ime(Ime::Commit(text))));
}

/// The page's paste event: its text, HTML and files, kept for the paste it runs.
#[wasm_bindgen]
pub fn paste(text: Option<String>, html: Option<String>, files: js_sys::Array) {
    let files = keep(files, CHOSEN);
    TYPED.set(true);
    PASTED.with_borrow_mut(|pasted| *pasted = Pasted { text, html, files });
    send(UserEvent::Choose(commands::Choice::Command(
        commands::Id::Paste,
    )));
}

#[wasm_bindgen]
pub fn wheel(x: f32, y: f32) {
    queue(Input::Ui(ui::Event::Wheel([x, y])));
}

#[wasm_bindgen]
pub fn pinch(factor: f32, x: f32, y: f32) {
    queue(Input::Pinch(factor, [x, y]));
}

#[wasm_bindgen]
pub fn resize(width: u32, height: u32, ratio: f32) {
    queue(Input::Resize([width, height], ratio));
}

#[wasm_bindgen]
pub fn focus(focused: bool) {
    queue(Input::Focus(focused));
}

#[wasm_bindgen]
pub fn appearance_changed(dark: bool) {
    host(|host| host.theme = if dark { Theme::Dark } else { Theme::Light });
    send(UserEvent::Appearance);
}

/// Files chosen or dropped, as `[name, bytes]` pairs, for `purpose`: `open` opens them as
/// notebooks, sections or packages, `place` puts them at the caret.
#[wasm_bindgen]
pub fn files(purpose: &str, files: js_sys::Array) {
    match purpose {
        "open" => {
            let paths = keep(files, NOTEBOOKS);
            send(UserEvent::Open(paths));
        }
        _ => {
            for path in keep(files, CHOSEN) {
                send(UserEvent::Then(Box::new(move |state| {
                    state.place_file(&path, None)
                })));
            }
        }
    }
}

/// A folder of the user's given to the browser, mirrored at `root` with `files` as
/// `loadFiles` gives them, opened as a notebook.
#[wasm_bindgen]
pub fn mounted(root: String, files: js_sys::Array) {
    notebook::fs::mount(&root);
    restore(files);
    send(UserEvent::Open(vec![root.into()]));
}

/// A path under a folder of the user's as it now stands there, after Snowbound wrote it or
/// after something else did: a file's bytes, null for a folder, undefined where it went. The
/// sections' synchronization takes it up.
#[wasm_bindgen]
pub fn refreshed(path: String, bytes: JsValue, modified: f64) {
    notebook::fs::restore(
        path,
        if bytes.is_undefined() {
            notebook::fs::Saved::Gone
        } else if bytes.is_null() {
            notebook::fs::Saved::Directory
        } else {
            notebook::fs::Saved::File(js_sys::Uint8Array::new(&bytes).to_vec(), modified)
        },
    );
    send(UserEvent::Then(Box::new(|state| {
        state.publish_now(Duration::ZERO);
        for background in state
            .notebooks
            .iter()
            .filter_map(|library| library.background.as_ref())
        {
            background.wake();
        }
        Ok(())
    })));
}

/// Writes `[name, bytes]` pairs into `folder` under names not yet taken, returning the paths.
fn keep(files: js_sys::Array, folder: &str) -> Vec<PathBuf> {
    files
        .iter()
        .filter_map(|pair| {
            let pair = js_sys::Array::from(&pair);
            let name = pair.get(0).as_string()?;
            let bytes = js_sys::Uint8Array::new(&pair.get(1)).to_vec();
            let name = Path::new(&name).file_name()?.to_owned();
            let mut path = Path::new(folder).join(&name);
            let (stem, extension) = (
                Path::new(&name).file_stem()?.to_string_lossy().into_owned(),
                Path::new(&name)
                    .extension()
                    .map(|extension| format!(".{}", extension.to_string_lossy()))
                    .unwrap_or_default(),
            );
            let mut number = 2;
            while notebook::fs::metadata(&path).is_ok() {
                path = Path::new(folder).join(format!("{stem} {number}{extension}"));
                number += 1;
            }
            notebook::fs::write(&path, bytes)
                .inspect_err(|error| report(error))
                .ok()?;
            Some(path)
        })
        .collect()
}

fn send(event: UserEvent) {
    EVENTS.with_borrow_mut(|events| events.push_back(event));
    request_frame();
}

/// Writes every changed file out now, as the page goes away.
#[wasm_bindgen]
pub fn flush() {
    STATE.with(|state| {
        if let Ok(mut state) = state.try_borrow_mut()
            && let Some(state) = state.as_mut()
            && let Err(error) = state.persist()
        {
            report(error);
        }
    });
    store();
}

#[wasm_bindgen]
pub fn frame() {
    STATE.with(|state| {
        let Ok(mut state) = state.try_borrow_mut() else {
            return request_frame();
        };
        let Some(state) = state.as_mut() else {
            return;
        };
        if let Err(error) = turn(state) {
            report(error);
        }
    });
    for work in LATER.take() {
        work();
    }
}

/// One turn of the loop: the page's input, the events sent to the loop, a frame where one
/// was asked for, then when to come back.
fn turn(state: &mut State) -> Result<(), Box<dyn Error>> {
    // Input after a chord waits for the frame that runs it and the one that builds what it
    // opened, as the palette takes the keys typed next.
    let mut inputs = match HOLDING.take() {
        true => Vec::new(),
        false => INPUT.take(),
    }
    .into_iter();
    while state.commands.is_empty()
        && let Some(input) = inputs.next()
    {
        match input {
            Input::Ui(event) => state.input(event),
            Input::Pinch(factor, point) => {
                state.input(ui::Event::PointerMoved(point));
                state.pinch(factor)?;
            }
            Input::Resize(size, ratio) => {
                let ratio = f64::from(ratio);
                let changed = host(|host| {
                    let changed = host.ratio != ratio;
                    host.size = PhysicalSize::new(size[0].max(1), size[1].max(1));
                    host.ratio = ratio;
                    changed
                });
                if changed {
                    state.renderer.clear_glyph_cache();
                }
                state.surface.size = size.map(|side| side.max(1));
                state.surface.configure(&state.renderer);
                host(|host| host.redraw = true);
            }
            Input::Focus(focused) => {
                state.ui.window_focused = focused;
                host(|host| host.focused = focused);
                if !focused {
                    state.publish_now(Duration::ZERO);
                }
                host(|host| host.redraw = true);
            }
        }
    }
    let waiting: Vec<Input> = inputs.collect();
    if !waiting.is_empty() {
        INPUT.with_borrow_mut(|queue| queue.splice(0..0, waiting).for_each(drop));
        HOLDING.set(true);
    }
    if HOLDING.get() || !INPUT.with_borrow(Vec::is_empty) {
        host(|host| host.redraw = true);
        request_frame();
    }
    while let Some(event) = EVENTS.with_borrow_mut(VecDeque::pop_front) {
        state.user_event(event);
    }
    if host(|host| std::mem::take(&mut host.redraw)) {
        state.frame()?;
    }
    let space = state.session.as_ref().map(|session| session.space);
    if TYPED.take() || LOOKED.replace(space) != space {
        fetch_fallbacks(&crate::page_text(&state.view.editor));
    }
    park_input(state);
    let now = Instant::now();
    let (repaint, blink) = state.view.blink(now);
    let wake = state.ui.wake_at();
    if repaint || wake.is_some_and(|wake| wake <= now) {
        state.window.request_redraw();
    }
    if STORE_DUE.get().is_none() && notebook::fs::changed() {
        STORE_DUE.set(Some(now + STORE_AFTER));
    }
    if STORE_DUE.get().is_some_and(|due| due <= now) {
        store();
    }
    let next = [blink, wake.filter(|wake| *wake > now), STORE_DUE.get()]
        .into_iter()
        .flatten()
        .min();
    if let Some(next) = next {
        wake_in(next.saturating_duration_since(now).as_secs_f64() * 1e3);
    }
    Ok(())
}

/// Fetches the fallback faces `text` calls for that are not yet asked for.
fn fetch_fallbacks(text: &str) {
    for character in text.chars().map(u32::from) {
        for (index, (name, ranges)) in FALLBACKS.iter().enumerate() {
            if FETCHED.get() & (1 << index) == 0
                && ranges.iter().any(|range| range.contains(&character))
            {
                FETCHED.set(FETCHED.get() | 1 << index);
                fetch_font(name);
            }
        }
    }
}

/// A fallback face arrived: the page, its loaders and the interface draw what the bundled
/// faces lack with it, and the page is laid out again.
#[wasm_bindgen]
pub fn font_arrived(bytes: Vec<u8>) {
    let face = parley::fontique::Blob::new(Arc::new(bytes));
    send(UserEvent::Then(Box::new(move |state| {
        let collection = &mut state.view.engine.fonts.collection;
        let families: Vec<String> = collection
            .register_fonts(face.clone(), None)
            .into_iter()
            .filter_map(|(family, _)| collection.family_name(family).map(String::from))
            .collect();
        draw::fall_back_to(collection, &families);
        if let Ok(mut layouts) = state.layouts.lock() {
            layouts.fonts.collection.register_fonts(face.clone(), None);
            draw::fall_back_to(&mut layouts.fonts.collection, &families);
        }
        for family in &families {
            state.ui.preview_font(face.clone(), family);
        }
        state.ui.fall_back_to(&families);
        state.renderer.clear_glyph_cache();
        state.refresh()
    })));
}

/// Parks the text area while the page takes no text, keeping it writable for the
/// interface's fields.
fn park_input(state: &State) {
    if state.ui.focused() != Some(page()) || !state.view.accepts_text() {
        place_input(0.0, 0.0, 1.0, state.ui.focused().is_some());
    }
}

/// The `KeyboardEvent.key` value `key` as winit names it.
fn logical_key(key: &str) -> Key {
    let named = match key {
        "Enter" => NamedKey::Enter,
        "Tab" => NamedKey::Tab,
        " " => NamedKey::Space,
        "Backspace" => NamedKey::Backspace,
        "Delete" => NamedKey::Delete,
        "Escape" => NamedKey::Escape,
        "ArrowLeft" => NamedKey::ArrowLeft,
        "ArrowRight" => NamedKey::ArrowRight,
        "ArrowUp" => NamedKey::ArrowUp,
        "ArrowDown" => NamedKey::ArrowDown,
        "Home" => NamedKey::Home,
        "End" => NamedKey::End,
        "PageUp" => NamedKey::PageUp,
        "PageDown" => NamedKey::PageDown,
        "Insert" => NamedKey::Insert,
        "Shift" => NamedKey::Shift,
        "Control" => NamedKey::Control,
        "Alt" => NamedKey::Alt,
        "AltGraph" => NamedKey::AltGraph,
        "Meta" => NamedKey::Meta,
        "ContextMenu" => NamedKey::ContextMenu,
        "F1" => NamedKey::F1,
        "F2" => NamedKey::F2,
        "F3" => NamedKey::F3,
        "F4" => NamedKey::F4,
        "F5" => NamedKey::F5,
        "F6" => NamedKey::F6,
        "F7" => NamedKey::F7,
        "F8" => NamedKey::F8,
        "F9" => NamedKey::F9,
        "F10" => NamedKey::F10,
        "F11" => NamedKey::F11,
        "F12" => NamedKey::F12,
        _ => return Key::Character(key.into()),
    };
    Key::Named(named)
}
