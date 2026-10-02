//! The clipboard on Linux. On Wayland it lives on winit's own connection, so other apps'
//! paste requests are answered on the event loop as they arrive, with no thread of its own;
//! text, pages, pictures and files are all read from the selection there. On X11, arboard's.

use std::{
    error::Error,
    io::Read,
    os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd},
    path::PathBuf,
    sync::{Arc, Mutex},
    task::{Context, Wake, Waker},
    time::{Duration, Instant},
};
use wayland_client::{
    Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum,
    backend::{Backend, ObjectData, ObjectId, protocol::Message},
    protocol::{
        wl_data_device::{self, WlDataDevice},
        wl_data_device_manager::WlDataDeviceManager,
        wl_data_offer::{self, WlDataOffer},
        wl_data_source::{self, WlDataSource},
        wl_keyboard::{self, WlKeyboard},
        wl_pointer::{self, WlPointer},
        wl_registry::{self, WlRegistry},
        wl_seat::{self, WlSeat},
        wl_surface::{self, WlSurface},
    },
};
use wayland_sys::{
    client::{wayland_client_handle, wl_event_queue, wl_proxy},
    ffi_dispatch,
};
use winit::{
    raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle},
    window::Window,
};

/// The MIME types text is offered and read as, the most precise first.
const TEXT: [&str; 4] = [
    "text/plain;charset=utf-8",
    "UTF8_STRING",
    "text/plain",
    "STRING",
];
/// Snowbound's own format, [`crate::paste::Copied::clip`].
const CLIP: &str = "application/x-snowbound-clip";
/// How long a paste waits on the app that copied.
const PATIENCE: Duration = Duration::from_secs(2);

/// The HTML and Snowbound's own format of this process's last copy through X11.
static LAST_COPY: Mutex<Option<(String, String)>> = Mutex::new(None);

pub enum Clipboard {
    Wayland(Box<Wayland>),
    X11(arboard::Clipboard),
}

impl Clipboard {
    pub fn new(window: &Window) -> Result<Self, arboard::Error> {
        let handles = window
            .display_handle()
            .ok()
            .zip(window.window_handle().ok());
        match handles.map(|(display, window)| (display.as_raw(), window.as_raw())) {
            Some((RawDisplayHandle::Wayland(display), RawWindowHandle::Wayland(window))) => Ok(
                Self::Wayland(Box::new(Wayland::new(display.display, window.surface))),
            ),
            _ => arboard::Clipboard::new().map(Self::X11),
        }
    }

    pub fn set_text(&mut self, text: String) -> Result<(), Box<dyn Error>> {
        match self {
            Self::Wayland(wayland) => {
                wayland.copy(TEXT.map(|kind| (kind, text.clone().into_bytes())).to_vec())
            }
            Self::X11(clipboard) => Ok(clipboard.set_text(text)?),
        }
    }

    /// Text, HTML and Snowbound's own format, as one copy: see [`crate::paste::Copied`].
    pub fn set(&mut self, copied: &crate::paste::Copied) -> Result<(), Box<dyn Error>> {
        match self {
            Self::Wayland(wayland) => {
                let text = TEXT.map(|kind| (kind, copied.text.clone().into_bytes()));
                let rich = [("text/html", &copied.html), (CLIP, &copied.clip)]
                    .map(|(kind, bytes)| (kind, bytes.clone().into_bytes()));
                wayland.copy(text.into_iter().chain(rich).collect())
            }
            Self::X11(clipboard) => {
                clipboard.set().html(&copied.html, Some(&copied.text))?;
                *LAST_COPY.lock().unwrap() = Some((copied.html.clone(), copied.clip.clone()));
                Ok(())
            }
        }
    }

    /// What Snowbound itself copied, if it did: on X11, which arboard offers no format of
    /// Snowbound's own through, the last copy's, while the clipboard holds its HTML.
    pub fn get_clip(&mut self) -> Option<String> {
        if let Self::Wayland(wayland) = self {
            return String::from_utf8(wayland.paste(CLIP)?).ok();
        }
        let html = self.get_html()?;
        match &*LAST_COPY.lock().unwrap() {
            Some((copied, clip)) if *copied == html => Some(clip.clone()),
            _ => None,
        }
    }

    pub fn get_text(&mut self) -> Result<String, Box<dyn Error>> {
        match self {
            Self::Wayland(wayland) => {
                let bytes = TEXT.iter().find_map(|kind| wayland.paste(kind));
                Ok(String::from_utf8_lossy(&bytes.unwrap_or_default()).into_owned())
            }
            Self::X11(clipboard) => Ok(clipboard.get_text()?),
        }
    }

    pub fn get_files(&mut self) -> Vec<PathBuf> {
        use std::os::unix::ffi::OsStrExt;
        match self {
            Self::Wayland(wayland) => files(&wayland.paste("text/uri-list").unwrap_or_default()),
            // arboard splits text/uri-list at LF and keeps the CR its CRLF lines end in.
            Self::X11(clipboard) => (clipboard.get().file_list().unwrap_or_default().into_iter())
                .map(|path| {
                    let bytes = path.as_os_str().as_bytes();
                    let bytes = bytes.strip_suffix(b"\r").unwrap_or(bytes);
                    std::ffi::OsStr::from_bytes(bytes).into()
                })
                .collect(),
        }
    }

    pub fn get_html(&mut self) -> Option<String> {
        match self {
            Self::Wayland(wayland) => wayland
                .paste("text/html")
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned()),
            Self::X11(clipboard) => clipboard.get().html().ok(),
        }
    }

    pub fn get_picture(&mut self) -> Option<Vec<u8>> {
        match self {
            Self::Wayland(wayland) => ["image/png", "image/jpeg", "image/gif"]
                .iter()
                .find_map(|kind| wayland.paste(kind)),
            Self::X11(clipboard) => crate::paste::bitmap(clipboard.get_image().ok()?),
        }
    }

    /// Answers what the compositor asked of the clipboard since it last woke the event loop.
    pub fn serve(&mut self) {
        if let Self::Wayland(wayland) = self {
            wayland.serve();
        }
    }
}

/// The local files a `text/uri-list` names; its lines end in CRLF, though some apps end
/// them in LF alone.
fn files(list: &[u8]) -> Vec<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    String::from_utf8_lossy(list)
        .lines()
        .map(|line| line.trim_end_matches('\r'))
        .filter(|line| !line.starts_with('#'))
        .filter_map(|uri| {
            let path = uri.strip_prefix("file://")?;
            let path = path.strip_prefix("localhost").unwrap_or(path);
            path.starts_with('/')
                .then(|| std::ffi::OsString::from_vec(crate::paste::percent_decode(path)).into())
        })
        .collect()
}

/// A copy's bytes in each MIME type it is offered as.
type Formats = Vec<(&'static str, Vec<u8>)>;

/// The Wayland clipboard, on winit's connection once the window's first frame lends it.
pub struct Wayland {
    lent: Arc<Mutex<Option<Backend>>>,
    joined: Option<(Connection, EventQueue<Seat>, Seat)>,
}

/// What the clipboard knows of the seat: the selection and the serial to claim it with.
#[derive(Default)]
struct Seat {
    seat: Option<WlSeat>,
    manager: Option<WlDataDeviceManager>,
    device: Option<WlDataDevice>,
    keyboard: Option<WlKeyboard>,
    pointer: Option<WlPointer>,
    /// The latest input event's, which claiming the selection must name.
    serial: u32,
    /// The selection another app offers.
    offer: Option<WlDataOffer>,
    /// What this app copied, in each format it offers, while it holds the selection.
    copied: Option<(WlDataSource, Formats)>,
}

/// Wakes the event loop to serve the clipboard.
struct Serve;

impl Wake for Serve {
    fn wake(self: Arc<Self>) {
        if let Some(proxy) = crate::platform::PROXY.get() {
            let _ = proxy.send_event(crate::UserEvent::Clipboard);
        }
    }
}

/// Takes winit's connection from the event that reaches it, a frame callback moved onto
/// winit's queue: winit dispatches only that queue, with its own connection.
struct Lend(Arc<Mutex<Option<Backend>>>);

impl ObjectData for Lend {
    fn event(
        self: Arc<Self>,
        backend: &Backend,
        _: Message<ObjectId, OwnedFd>,
    ) -> Option<Arc<dyn ObjectData>> {
        *self.0.lock().unwrap() = Some(backend.clone());
        Arc::new(Serve).wake();
        None
    }

    fn destroyed(&self, _: ObjectId) {}
}

impl Wayland {
    fn new(
        display: std::ptr::NonNull<std::ffi::c_void>,
        surface: std::ptr::NonNull<std::ffi::c_void>,
    ) -> Self {
        let lent = Arc::new(Mutex::new(None));
        // Safety: winit's display and surface outlive the window, which outlives this.
        let backend = unsafe { Backend::from_foreign_display(display.as_ptr().cast()) };
        let connection = Connection::from_backend(backend);
        let surface =
            unsafe { ObjectId::from_ptr(WlSurface::interface(), surface.as_ptr().cast()) }
                .and_then(|id| WlSurface::from_id(&connection, id));
        if let Ok(surface) = surface {
            let lend: Arc<dyn ObjectData> = Arc::new(Lend(Arc::clone(&lent)));
            let frame = wl_surface::Request::Frame {};
            if let Ok(callback) = connection.send_request(&surface, frame, Some(lend)) {
                // Safety: both proxies are live, and the callback has had no event yet.
                unsafe { onto_queue_of(callback.as_ptr(), surface.id().as_ptr()) };
            }
            let _ = connection.flush();
        }
        // The callback, on winit's queue, ends under winit's connection, which leaves this one
        // a record of it that dropping it would free a second time.
        std::mem::forget(connection);
        Self { lent, joined: None }
    }

    /// Dispatches the seat's events and asks to be woken for the next; first joins winit's
    /// connection once lent.
    fn serve(&mut self) {
        if self.joined.is_none()
            && let Some(backend) = self.lent.lock().unwrap().take()
        {
            let connection = Connection::from_backend(backend);
            let queue = connection.new_event_queue();
            connection.display().get_registry(&queue.handle(), ());
            self.joined = Some((connection, queue, Seat::default()));
        }
        if let Some((_, queue, seat)) = &mut self.joined {
            let waker = Waker::from(Arc::new(Serve));
            if let std::task::Poll::Ready(Err(error)) =
                queue.poll_dispatch_pending(&mut Context::from_waker(&waker), seat)
            {
                eprintln!("The clipboard stopped: {error}");
                self.joined = None;
            }
        }
    }

    fn copy(&mut self, formats: Formats) -> Result<(), Box<dyn Error>> {
        self.serve();
        let Some((connection, queue, seat)) = &mut self.joined else {
            return Err("The clipboard isn't ready yet.".into());
        };
        let (Some(manager), Some(device)) = (&seat.manager, &seat.device) else {
            return Err("The compositor offers no clipboard.".into());
        };
        let source = manager.create_data_source(&queue.handle(), ());
        for (kind, _) in &formats {
            source.offer(kind.to_string());
        }
        device.set_selection(Some(&source), seat.serial);
        // The source replaced is cancelled, and destroyed then.
        seat.copied = Some((source, formats));
        connection.flush()?;
        Ok(())
    }

    /// The selection as `kind`, where it is offered as that: this app's own copy without a
    /// round trip through the compositor, or what another app answers.
    fn paste(&mut self, kind: &str) -> Option<Vec<u8>> {
        self.serve();
        let (connection, _, seat) = self.joined.as_ref()?;
        if let Some((_, formats)) = &seat.copied {
            return offered(formats, kind).map(<[u8]>::to_vec);
        }
        let offer = seat.offer.as_ref()?;
        let kinds = offer.data::<Mutex<Vec<String>>>()?;
        if !kinds.lock().unwrap().iter().any(|offered| offered == kind) {
            return None;
        }
        let (mut reader, writer) = std::io::pipe().ok()?;
        offer.receive(kind.to_owned(), writer.as_fd());
        drop(writer);
        connection.flush().ok()?;
        let deadline = Instant::now() + PATIENCE;
        let mut bytes = Vec::new();
        loop {
            if !ready(reader.as_fd(), libc::POLLIN, deadline) {
                eprintln!("The app that copied didn't answer the paste in time.");
                return None;
            }
            let mut buffer = [0; 1 << 16];
            match reader.read(&mut buffer) {
                Ok(0) => return Some(bytes),
                Ok(read) => bytes.extend_from_slice(&buffer[..read]),
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => return None,
            }
        }
    }
}

/// Moves `proxy` onto `owner`'s event queue. libwayland names a proxy's queue only from 1.23
/// (`wl_proxy_get_queue`), so this reads it from `struct wl_proxy`, laid out alike since 1.0.
unsafe fn onto_queue_of(proxy: *mut wl_proxy, owner: *mut wl_proxy) {
    #[repr(C)]
    struct Head {
        interface: *const std::ffi::c_void,
        implementation: *const std::ffi::c_void,
        id: u32,
        display: *mut std::ffi::c_void,
        queue: *mut wl_event_queue,
    }
    let queue = unsafe { (*owner.cast::<Head>()).queue };
    unsafe { ffi_dispatch!(wayland_client_handle(), wl_proxy_set_queue, proxy, queue) };
}

/// The bytes of `formats` in the format `kind`.
fn offered<'a>(formats: &'a [(&str, Vec<u8>)], kind: &str) -> Option<&'a [u8]> {
    let (_, bytes) = formats.iter().find(|(offered, _)| *offered == kind)?;
    Some(bytes)
}

/// Whether `fd` is ready for `events` before `deadline`.
fn ready(fd: BorrowedFd, events: i16, deadline: Instant) -> bool {
    let left = deadline.saturating_duration_since(Instant::now());
    let mut poll = libc::pollfd {
        fd: fd.as_raw_fd(),
        events,
        revents: 0,
    };
    !left.is_zero() && unsafe { libc::poll(&mut poll, 1, left.as_millis() as i32) } > 0
}

/// Writes `bytes` to the pipe `fd`, waiting on a slow reader no longer than a paste waits.
fn answer(fd: OwnedFd, bytes: &[u8]) {
    use std::io::Write;
    let mut pipe = std::fs::File::from(fd);
    let deadline = Instant::now() + PATIENCE;
    // A pipe ready for writing takes PIPE_BUF bytes without blocking.
    for chunk in bytes.chunks(libc::PIPE_BUF) {
        if !ready(pipe.as_fd(), libc::POLLOUT, deadline) || pipe.write_all(chunk).is_err() {
            return;
        }
    }
}

impl Dispatch<WlRegistry, ()> for Seat {
    fn event(
        seat: &mut Self,
        registry: &WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        queue: &QueueHandle<Self>,
    ) {
        let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        else {
            return;
        };
        match interface.as_str() {
            "wl_seat" if seat.seat.is_none() => {
                seat.seat = Some(registry.bind(name, version.min(5), queue, ()));
            }
            "wl_data_device_manager" if seat.manager.is_none() => {
                seat.manager = Some(registry.bind(name, version.min(3), queue, ()));
            }
            _ => return,
        }
        if let (Some(manager), Some(wl_seat), None) = (&seat.manager, &seat.seat, &seat.device) {
            seat.device = Some(manager.get_data_device(wl_seat, queue, ()));
        }
    }
}

impl Dispatch<WlSeat, ()> for Seat {
    fn event(
        seat: &mut Self,
        wl_seat: &WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        queue: &QueueHandle<Self>,
    ) {
        let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(capabilities),
        } = event
        else {
            return;
        };
        let keyboard = capabilities.contains(wl_seat::Capability::Keyboard);
        if keyboard != seat.keyboard.is_some() {
            seat.keyboard = keyboard.then(|| wl_seat.get_keyboard(queue, ()));
        }
        let pointer = capabilities.contains(wl_seat::Capability::Pointer);
        if pointer != seat.pointer.is_some() {
            seat.pointer = pointer.then(|| wl_seat.get_pointer(queue, ()));
        }
    }
}

impl Dispatch<WlKeyboard, ()> for Seat {
    fn event(
        seat: &mut Self,
        _: &WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_keyboard::Event::Enter { serial, .. } | wl_keyboard::Event::Key { serial, .. } =
            event
        {
            seat.serial = serial;
        }
    }
}

impl Dispatch<WlPointer, ()> for Seat {
    fn event(
        seat: &mut Self,
        _: &WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_pointer::Event::Enter { serial, .. } | wl_pointer::Event::Button { serial, .. } =
            event
        {
            seat.serial = serial;
        }
    }
}

impl Dispatch<WlDataDeviceManager, ()> for Seat {
    fn event(
        _: &mut Self,
        _: &WlDataDeviceManager,
        _: <WlDataDeviceManager as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlDataDevice, ()> for Seat {
    fn event(
        seat: &mut Self,
        _: &WlDataDevice,
        event: wl_data_device::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_data_device::Event::Selection { id } => {
                if let Some(old) = std::mem::replace(&mut seat.offer, id) {
                    old.destroy();
                }
            }
            // Drags aren't taken.
            wl_data_device::Event::Enter {
                id: Some(offer), ..
            } => offer.destroy(),
            _ => {}
        }
    }

    wayland_client::event_created_child!(Seat, WlDataDevice, [
        wl_data_device::EVT_DATA_OFFER_OPCODE => (WlDataOffer, Mutex::new(Vec::<String>::new())),
    ]);
}

impl Dispatch<WlDataOffer, Mutex<Vec<String>>> for Seat {
    fn event(
        _: &mut Self,
        _: &WlDataOffer,
        event: wl_data_offer::Event,
        kinds: &Mutex<Vec<String>>,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_data_offer::Event::Offer { mime_type } = event {
            kinds.lock().unwrap().push(mime_type);
        }
    }
}

impl Dispatch<WlDataSource, ()> for Seat {
    fn event(
        seat: &mut Self,
        source: &WlDataSource,
        event: wl_data_source::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let copied = seat.copied.as_ref().filter(|(copied, _)| copied == source);
        match event {
            wl_data_source::Event::Send { mime_type, fd } => {
                if let Some(bytes) = copied.and_then(|(_, formats)| offered(formats, &mime_type)) {
                    answer(fd, bytes);
                }
            }
            wl_data_source::Event::Cancelled => {
                if copied.is_some() {
                    seat.copied = None;
                }
                source.destroy();
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_lists_name_local_files_whatever_their_line_ends() {
        let list = b"# copied\r\nfile:///home/ben/Notes%20One.one\r\nfile://localhost/tmp/a.png\nhttps://example.com/b.png\r\n";
        assert_eq!(
            files(list),
            ["/home/ben/Notes One.one", "/tmp/a.png"].map(PathBuf::from)
        );
    }
}
