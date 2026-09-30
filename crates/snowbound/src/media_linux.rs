//! Audio through GStreamer, loaded at run time so builds need neither its headers nor its
//! library: `playbin` plays whatever the installed plugins decode (Windows Media with
//! gst-libav), and the default source records as PCM WAV.

use std::{
    cell::Cell,
    ffi::{CStr, CString, c_char, c_int, c_void},
    path::Path,
    sync::OnceLock,
};

type Element = *mut c_void;

/// GST_STATE_NULL, GST_STATE_PAUSED and GST_STATE_PLAYING.
const NULL: c_int = 1;
const PAUSED: c_int = 3;
const PLAYING: c_int = 4;
/// GST_FORMAT_TIME, in nanoseconds.
const TIME: c_int = 3;
/// GST_SEEK_FLAG_FLUSH | GST_SEEK_FLAG_ACCURATE.
const SEEK: c_int = 1 | 2;
/// GST_MESSAGE_EOS, GST_MESSAGE_ERROR and GST_MESSAGE_ASYNC_DONE.
const EOS: c_int = 1;
const ERROR: c_int = 2;
const ASYNC_DONE: c_int = 1 << 21;
/// Nanoseconds a pipeline has to get ready or to finish.
const PATIENCE: u64 = 5_000_000_000;
/// GST_STATE_CHANGE_FAILURE.
const FAILURE: c_int = 0;

/// The GStreamer entry points used, from `libgstreamer-1.0.so.0`.
struct Gst {
    parse_launch: unsafe extern "C" fn(*const c_char, *mut *mut c_void) -> Element,
    set_state: unsafe extern "C" fn(Element, c_int) -> c_int,
    query_position: unsafe extern "C" fn(Element, c_int, *mut i64) -> c_int,
    query_duration: unsafe extern "C" fn(Element, c_int, *mut i64) -> c_int,
    seek_simple: unsafe extern "C" fn(Element, c_int, c_int, i64) -> c_int,
    send_event: unsafe extern "C" fn(Element, *mut c_void) -> c_int,
    new_eos: unsafe extern "C" fn() -> *mut c_void,
    get_bus: unsafe extern "C" fn(Element) -> *mut c_void,
    pop: unsafe extern "C" fn(*mut c_void, u64, c_int) -> *mut c_void,
    unref_message: unsafe extern "C" fn(*mut c_void),
    unref: unsafe extern "C" fn(*mut c_void),
}

// SAFETY: GStreamer's entry points are thread-safe once initialized.
unsafe impl Send for Gst {}
unsafe impl Sync for Gst {}

/// The function `name` in `library`, of C signature `F`.
unsafe fn symbol<F: Copy>(library: *mut c_void, name: &CStr) -> Option<F> {
    let symbol = unsafe { libc::dlsym(library, name.as_ptr()) };
    (!symbol.is_null()).then(|| unsafe { std::mem::transmute_copy::<*mut c_void, F>(&symbol) })
}

/// GStreamer, initialized; none where it is not installed.
fn gst() -> Option<&'static Gst> {
    static GST: OnceLock<Option<Gst>> = OnceLock::new();
    GST.get_or_init(|| unsafe {
        let library = libc::dlopen(c"libgstreamer-1.0.so.0".as_ptr(), libc::RTLD_NOW);
        if library.is_null() {
            return None;
        }
        let init: unsafe extern "C" fn(*mut c_int, *mut c_void) = symbol(library, c"gst_init")?;
        init(std::ptr::null_mut(), std::ptr::null_mut());
        Some(Gst {
            parse_launch: symbol(library, c"gst_parse_launch")?,
            set_state: symbol(library, c"gst_element_set_state")?,
            query_position: symbol(library, c"gst_element_query_position")?,
            query_duration: symbol(library, c"gst_element_query_duration")?,
            seek_simple: symbol(library, c"gst_element_seek_simple")?,
            send_event: symbol(library, c"gst_element_send_event")?,
            new_eos: symbol(library, c"gst_event_new_eos")?,
            get_bus: symbol(library, c"gst_element_get_bus")?,
            pop: symbol(library, c"gst_bus_timed_pop_filtered")?,
            unref_message: symbol(library, c"gst_mini_object_unref")?,
            unref: symbol(library, c"gst_object_unref")?,
        })
    })
    .as_ref()
}

/// `path` as a `file://` URI, every byte outside the unreserved set escaped.
fn uri(path: &Path) -> String {
    let mut uri = String::from("file://");
    for byte in path.as_os_str().as_encoded_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                uri.push(char::from(*byte));
            }
            _ => uri.push_str(&format!("%{byte:02X}")),
        }
    }
    uri
}

/// A pipeline described as `gst-launch` takes one, paused.
struct Pipeline {
    gst: &'static Gst,
    element: Element,
}

impl Pipeline {
    fn launch(description: &str) -> Option<Self> {
        let gst = gst()?;
        let description = CString::new(description).ok()?;
        let element = unsafe { (gst.parse_launch)(description.as_ptr(), std::ptr::null_mut()) };
        if element.is_null() {
            return None;
        }
        let pipeline = Self { gst, element };
        pipeline.set(PAUSED).then_some(pipeline)
    }

    fn set(&self, state: c_int) -> bool {
        unsafe { (self.gst.set_state)(self.element, state) != FAILURE }
    }

    /// The type of the next message of `types` the pipeline posts within `timeout`
    /// nanoseconds.
    fn wait(&self, timeout: u64, types: c_int) -> Option<c_int> {
        unsafe {
            let bus = (self.gst.get_bus)(self.element);
            let message = (self.gst.pop)(bus, timeout, types);
            (self.gst.unref)(bus);
            if message.is_null() {
                return None;
            }
            // GstMessage's type follows its GstMiniObject header.
            let kind = *message
                .cast::<u8>()
                .add(std::mem::size_of::<MiniObject>())
                .cast::<c_int>();
            (self.gst.unref_message)(message);
            Some(kind)
        }
    }

    fn query(&self, query: unsafe extern "C" fn(Element, c_int, *mut i64) -> c_int) -> u32 {
        let mut nanoseconds = 0_i64;
        let known = unsafe { query(self.element, TIME, &mut nanoseconds) } != 0;
        if known {
            (nanoseconds.max(0) / 1_000_000) as u32
        } else {
            0
        }
    }
}

impl Drop for Pipeline {
    fn drop(&mut self) {
        self.set(NULL);
        unsafe { (self.gst.unref)(self.element) };
    }
}

/// GstMiniObject's layout, which a message's type follows.
#[repr(C)]
struct MiniObject {
    kind: usize,
    refcount: c_int,
    lockstate: c_int,
    flags: u32,
    copy: *mut c_void,
    dispose: *mut c_void,
    free: *mut c_void,
    priv_uint: u32,
    priv_pointer: *mut c_void,
}

/// A recording or other audio file playing, paused or ended.
pub struct Player {
    pipeline: Pipeline,
    playing: bool,
    /// Whether the file played to its end, which the pipeline reports once.
    ended: Cell<bool>,
}

impl Player {
    /// The file at `path`, ready to play; none when GStreamer is missing or cannot decode it.
    pub fn open(path: &Path) -> Option<Self> {
        let pipeline = Pipeline::launch(&format!(
            "playbin uri=\"{}\" video-sink=fakesink",
            uri(path)
        ))?;
        // Prerolling reports a file no plugin decodes as an error.
        if pipeline.wait(PATIENCE, ASYNC_DONE | ERROR) != Some(ASYNC_DONE) {
            return None;
        }
        Some(Self {
            pipeline,
            playing: false,
            ended: Cell::new(false),
        })
    }

    pub fn play(&mut self) {
        self.ended.set(false);
        self.playing = self.pipeline.set(PLAYING);
    }

    pub fn pause(&mut self) {
        self.pipeline.set(PAUSED);
        self.playing = false;
    }

    pub fn playing(&self) -> bool {
        if self.playing && !self.ended.get() && self.pipeline.wait(0, EOS).is_some() {
            self.ended.set(true);
        }
        self.playing && !self.ended.get()
    }

    pub fn position_ms(&self) -> u32 {
        if self.ended.get() {
            return self.duration_ms();
        }
        self.pipeline.query(self.pipeline.gst.query_position)
    }

    pub fn duration_ms(&self) -> u32 {
        self.pipeline.query(self.pipeline.gst.query_duration)
    }

    pub fn seek(&mut self, ms: u32) {
        self.ended.set(false);
        unsafe {
            (self.pipeline.gst.seek_simple)(
                self.pipeline.element,
                TIME,
                SEEK,
                i64::from(ms) * 1_000_000,
            );
        }
    }
}

/// The default microphone recording to a file.
pub struct Microphone(Pipeline);

impl Microphone {
    /// Records mono 16-bit PCM at `rate` into a WAV file at `path`.
    pub fn start(path: &Path, rate: u32) -> Result<Self, String> {
        let location = path
            .to_str()
            .filter(|location| !location.contains('"'))
            .ok_or("Try recording again.")?;
        let pipeline = Pipeline::launch(&format!(
            "autoaudiosrc ! audioconvert ! audioresample ! \
             audio/x-raw,format=S16LE,rate={rate},channels=1 ! wavenc ! \
             filesink location=\"{location}\""
        ))
        .ok_or("Install GStreamer and its base plugins to record audio.")?;
        if !pipeline.set(PLAYING) {
            return Err("Connect a microphone and try again.".into());
        }
        Ok(Self(pipeline))
    }

    /// Ends the recording, its file complete: the end travels down the pipeline so the WAV
    /// header is written.
    pub fn stop(self) -> Result<(), String> {
        let pipeline = &self.0;
        unsafe { (pipeline.gst.send_event)(pipeline.element, (pipeline.gst.new_eos)()) };
        match pipeline.wait(PATIENCE, EOS | ERROR) {
            Some(EOS) => Ok(()),
            _ => Err("The recording stopped early. Try recording again.".into()),
        }
    }
}
