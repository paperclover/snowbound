//! Changes below a notebook folder, as the system reports them: FSEvents on macOS, inotify on
//! Linux. A notebook on a share learns of its changes from the server instead
//! (`notebook::session::Background::smb`).

use std::path::{Path, PathBuf};

/// Reports until dropped.
pub struct Watch {
    _stream: platform::Stream,
    /// In iCloud Drive, file coordination's reports too, which name the conflict versions a
    /// file gains; those change no file.
    _presenter: Option<crate::icloud::Presenter>,
}

/// Watches the folder at `root`: `changed` hears the paths that changed below it, relative to
/// it and `/`-separated, or a folder's path when the system can only say something in it did.
/// `None` where the system does not report every change: on a network volume other than a
/// Mac's SMB mount, another computer's changes go unreported.
pub fn watch(root: &Path, changed: impl Fn(Vec<String>) + Send + Sync + 'static) -> Option<Watch> {
    let root = root.canonicalize().ok()?;
    let volume = statfs(&root)?;
    let folders = if local(&volume) {
        vec![root.clone()]
    } else if smbfs(&volume) {
        // smbfs reports another client's change only in a folder watched for itself, and
        // names just that folder.
        let mut folders = vec![root.clone()];
        let mut at = 0;
        while let Some(folder) = folders.get(at).cloned() {
            at += 1;
            for entry in std::fs::read_dir(folder).into_iter().flatten().flatten() {
                if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                    folders.push(entry.path());
                }
            }
        }
        folders
    } else {
        return None;
    };
    let changed = std::sync::Arc::new(changed);
    let presenter = crate::icloud::ubiquitous(&root)
        .then(|| {
            let changed = std::sync::Arc::clone(&changed);
            crate::icloud::presenter(&root, move |paths| changed(paths))
        })
        .flatten();
    let folder = root.clone();
    let relative = move |paths: Vec<PathBuf>| {
        changed(
            paths
                .iter()
                .map(|path| match path.strip_prefix(&folder) {
                    Ok(path) => path.to_string_lossy().replace('\\', "/"),
                    Err(_) => String::new(),
                })
                .collect(),
        )
    };
    platform::Stream::start(&folders, Box::new(relative))
        .inspect_err(|error| eprintln!("{}: changes go unwatched: {error}", root.display()))
        .ok()
        .map(|stream| Watch {
            _stream: stream,
            _presenter: presenter,
        })
}

/// Whether the folder at `path` is on this computer's own disks rather than on a network
/// volume.
pub fn on_this_computer(path: &Path) -> bool {
    statfs(path).is_none_or(|volume| local(&volume))
}

fn statfs(path: &Path) -> Option<libc::statfs> {
    use std::os::unix::ffi::OsStrExt;
    let name = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut volume: libc::statfs = unsafe { std::mem::zeroed() };
    (unsafe { libc::statfs(name.as_ptr(), &mut volume) } == 0).then_some(volume)
}

#[cfg(target_os = "macos")]
fn local(volume: &libc::statfs) -> bool {
    volume.f_flags & libc::MNT_LOCAL as u32 != 0
}

// NFS, SMB, CIFS, SMB2, FUSE (sshfs and the like), 9P, Ceph, AFS.
#[cfg(target_os = "linux")]
fn local(volume: &libc::statfs) -> bool {
    ![
        0x6969,
        0x517b,
        0xff53_4d42,
        0xfe53_4d42,
        0x6573_5546,
        0x0102_1997,
        0x00c3_6400,
        0x5346_414f,
    ]
    .contains(&(volume.f_type as u64))
}

#[cfg(target_os = "macos")]
fn smbfs(volume: &libc::statfs) -> bool {
    let name = unsafe { std::ffi::CStr::from_ptr(volume.f_fstypename.as_ptr()) };
    name.to_bytes() == b"smbfs"
}

/// A Linux SMB mount reports only this computer's own changes.
#[cfg(target_os = "linux")]
fn smbfs(_: &libc::statfs) -> bool {
    false
}

type Changed = Box<dyn Fn(Vec<std::path::PathBuf>) + Send + Sync>;

#[cfg(target_os = "macos")]
mod platform {
    use super::Changed;
    use std::{
        ffi::{CStr, c_char, c_void},
        io,
        os::unix::ffi::OsStrExt,
        path::PathBuf,
    };

    type Stream_ = *mut c_void;

    #[repr(C)]
    struct Context {
        version: isize,
        info: *mut c_void,
        retain: *const c_void,
        release: Option<unsafe extern "C" fn(*const c_void)>,
        description: *const c_void,
    }

    type Callback =
        unsafe extern "C" fn(Stream_, *mut c_void, usize, *mut c_void, *const u32, *const u64);

    #[link(name = "CoreServices", kind = "framework")]
    unsafe extern "C" {
        fn FSEventStreamCreate(
            allocator: *const c_void,
            callback: Callback,
            context: *const Context,
            paths: *const c_void,
            since: u64,
            latency: f64,
            flags: u32,
        ) -> Stream_;
        fn FSEventStreamSetDispatchQueue(stream: Stream_, queue: *mut c_void);
        fn FSEventStreamStart(stream: Stream_) -> u8;
        fn FSEventStreamStop(stream: Stream_);
        fn FSEventStreamInvalidate(stream: Stream_);
        fn FSEventStreamRelease(stream: Stream_);
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        static kCFTypeArrayCallBacks: c_void;
        fn CFStringCreateWithBytes(
            allocator: *const c_void,
            bytes: *const u8,
            length: isize,
            encoding: u32,
            external: u8,
        ) -> *const c_void;
        fn CFArrayCreate(
            allocator: *const c_void,
            values: *const *const c_void,
            count: isize,
            callbacks: *const c_void,
        ) -> *const c_void;
        fn CFRelease(object: *const c_void);
    }

    unsafe extern "C" {
        fn dispatch_queue_create(label: *const c_char, attributes: *const c_void) -> *mut c_void;
        fn dispatch_sync_f(
            queue: *mut c_void,
            context: *mut c_void,
            work: extern "C" fn(*mut c_void),
        );
        fn dispatch_release(object: *mut c_void);
    }

    const SINCE_NOW: u64 = u64::MAX;
    const NO_DEFER: u32 = 0x2;
    const WATCH_ROOT: u32 = 0x4;
    /// Mac OS X 10.7 and later; 10.6 reports the folders that changed.
    const FILE_EVENTS: u32 = 0x10;
    const UTF8: u32 = 0x0800_0100;

    pub struct Stream {
        stream: Stream_,
        queue: *mut c_void,
    }

    // The stream and queue are only started and torn down, each call safe from any thread.
    unsafe impl Send for Stream {}
    unsafe impl Sync for Stream {}

    unsafe extern "C" fn events(
        _: Stream_,
        info: *mut c_void,
        count: usize,
        paths: *mut c_void,
        _: *const u32,
        _: *const u64,
    ) {
        let changed = unsafe { &*(info as *const Changed) };
        let paths = unsafe { std::slice::from_raw_parts(paths as *const *const c_char, count) };
        changed(
            paths
                .iter()
                .map(|&path| {
                    let path = unsafe { CStr::from_ptr(path) };
                    PathBuf::from(std::ffi::OsStr::from_bytes(path.to_bytes()))
                })
                .collect(),
        );
    }

    unsafe extern "C" fn release(info: *const c_void) {
        drop(unsafe { Box::from_raw(info as *mut Changed) });
    }

    extern "C" fn drained(_: *mut c_void) {}

    /// Whether this is Mac OS X 10.7 or later (Darwin 11).
    fn file_events() -> bool {
        let mut name: libc::utsname = unsafe { std::mem::zeroed() };
        (unsafe { libc::uname(&mut name) }) == 0
            && unsafe { CStr::from_ptr(name.release.as_ptr()) }
                .to_str()
                .ok()
                .and_then(|release| release.split('.').next()?.parse::<u32>().ok())
                .is_some_and(|major| major >= 11)
    }

    impl Stream {
        pub fn start(folders: &[PathBuf], changed: Changed) -> io::Result<Self> {
            let flags = NO_DEFER | WATCH_ROOT | if file_events() { FILE_EVENTS } else { 0 };
            unsafe {
                let folders: Vec<_> = folders
                    .iter()
                    .map(|folder| {
                        let folder = folder.as_os_str().as_bytes();
                        CFStringCreateWithBytes(
                            std::ptr::null(),
                            folder.as_ptr(),
                            folder.len() as isize,
                            UTF8,
                            0,
                        )
                    })
                    .collect();
                let paths = CFArrayCreate(
                    std::ptr::null(),
                    folders.as_ptr(),
                    folders.len() as isize,
                    &kCFTypeArrayCallBacks,
                );
                for folder in folders {
                    CFRelease(folder);
                }
                let context = Context {
                    version: 0,
                    info: Box::into_raw(Box::new(changed)).cast(),
                    retain: std::ptr::null(),
                    release: Some(release),
                    description: std::ptr::null(),
                };
                let stream = FSEventStreamCreate(
                    std::ptr::null(),
                    events,
                    &context,
                    paths,
                    SINCE_NOW,
                    0.3,
                    flags,
                );
                CFRelease(paths);
                if stream.is_null() {
                    release(context.info);
                    return Err(io::Error::other("FSEvents refused the folder"));
                }
                let queue = dispatch_queue_create(c"snowbound.watch".as_ptr(), std::ptr::null());
                FSEventStreamSetDispatchQueue(stream, queue);
                let stream = Self { stream, queue };
                if FSEventStreamStart(stream.stream) == 0 {
                    return Err(io::Error::other("FSEvents did not start"));
                }
                Ok(stream)
            }
        }
    }

    impl Drop for Stream {
        fn drop(&mut self) {
            unsafe {
                FSEventStreamStop(self.stream);
                FSEventStreamInvalidate(self.stream);
                // A callback already on the queue finishes before the closure goes.
                dispatch_sync_f(self.queue, std::ptr::null_mut(), drained);
                FSEventStreamRelease(self.stream);
                dispatch_release(self.queue);
            }
        }
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use super::Changed;
    use std::{
        collections::HashMap,
        ffi::CString,
        io,
        os::unix::ffi::OsStrExt,
        path::{Path, PathBuf},
        thread::JoinHandle,
    };

    const MASK: u32 = libc::IN_MODIFY
        | libc::IN_CLOSE_WRITE
        | libc::IN_ATTRIB
        | libc::IN_CREATE
        | libc::IN_DELETE
        | libc::IN_MOVED_FROM
        | libc::IN_MOVED_TO
        | libc::IN_DELETE_SELF
        | libc::IN_MOVE_SELF;

    /// inotify watches one folder at a time, so every folder below the root has its own.
    pub struct Stream {
        inotify: i32,
        stop: i32,
        thread: Option<JoinHandle<()>>,
    }

    fn check(result: i32) -> io::Result<i32> {
        if result < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(result)
        }
    }

    fn add(inotify: i32, folder: &Path, folders: &mut HashMap<i32, PathBuf>) {
        let Ok(name) = CString::new(folder.as_os_str().as_bytes()) else {
            return;
        };
        if let Ok(watch) = check(unsafe { libc::inotify_add_watch(inotify, name.as_ptr(), MASK) }) {
            folders.insert(watch, folder.to_owned());
        }
        for entry in std::fs::read_dir(folder).into_iter().flatten().flatten() {
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                add(inotify, &entry.path(), folders);
            }
        }
    }

    impl Stream {
        pub fn start(folders: &[PathBuf], changed: Changed) -> io::Result<Self> {
            let inotify = check(unsafe { libc::inotify_init1(libc::IN_CLOEXEC) })?;
            let stop = match check(unsafe { libc::eventfd(0, libc::EFD_CLOEXEC) }) {
                Ok(stop) => stop,
                Err(error) => {
                    unsafe { libc::close(inotify) };
                    return Err(error);
                }
            };
            let root = folders[0].clone();
            let mut folders = HashMap::new();
            add(inotify, &root, &mut folders);
            let thread = std::thread::Builder::new()
                .name("snowbound-watch".into())
                .spawn(move || {
                    let mut buffer = vec![0u8; 64 * 1024];
                    loop {
                        let mut polled = [
                            libc::pollfd {
                                fd: inotify,
                                events: libc::POLLIN,
                                revents: 0,
                            },
                            libc::pollfd {
                                fd: stop,
                                events: libc::POLLIN,
                                revents: 0,
                            },
                        ];
                        if unsafe { libc::poll(polled.as_mut_ptr(), 2, -1) } < 0 {
                            if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                                continue;
                            }
                            return;
                        }
                        if polled[1].revents != 0 {
                            return;
                        }
                        let read = unsafe {
                            libc::read(inotify, buffer.as_mut_ptr().cast(), buffer.len())
                        };
                        let Ok(read) = usize::try_from(read) else {
                            return;
                        };
                        let mut paths = Vec::new();
                        let mut at = 0;
                        while at + std::mem::size_of::<libc::inotify_event>() <= read {
                            let event: libc::inotify_event =
                                unsafe { std::ptr::read_unaligned(buffer[at..].as_ptr().cast()) };
                            let start = at + std::mem::size_of::<libc::inotify_event>();
                            let end = (start + event.len as usize).min(read);
                            at = end;
                            if event.mask & libc::IN_Q_OVERFLOW != 0 {
                                paths.push(root.clone());
                                continue;
                            }
                            let Some(folder) = folders.get(&event.wd).cloned() else {
                                continue;
                            };
                            let name = &buffer[start..end];
                            let name =
                                &name[..name.iter().position(|&b| b == 0).unwrap_or(name.len())];
                            let path = if name.is_empty() {
                                folder
                            } else {
                                folder.join(std::ffi::OsStr::from_bytes(name))
                            };
                            if event.mask & libc::IN_ISDIR != 0
                                && event.mask & (libc::IN_CREATE | libc::IN_MOVED_TO) != 0
                            {
                                add(inotify, &path, &mut folders);
                            }
                            if event.mask & libc::IN_IGNORED != 0 {
                                folders.remove(&event.wd);
                            }
                            paths.push(path);
                        }
                        if !paths.is_empty() {
                            changed(paths);
                        }
                    }
                })?;
            Ok(Self {
                inotify,
                stop,
                thread: Some(thread),
            })
        }
    }

    impl Drop for Stream {
        fn drop(&mut self) {
            unsafe { libc::write(self.stop, (&1u64 as *const u64).cast(), 8) };
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
            unsafe {
                libc::close(self.inotify);
                libc::close(self.stop);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::mpsc, time::Duration};

    #[test]
    fn a_folder_on_this_computer_reports_the_file_that_changed() {
        let folder = std::env::temp_dir().join(format!("snowbound-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(folder.join("Group")).unwrap();
        std::fs::write(folder.join("Group/Section.one"), b"before").unwrap();
        assert!(on_this_computer(&folder));
        let (sender, changes) = mpsc::channel();
        let sender = std::sync::Mutex::new(sender);
        let watching = watch(&folder, move |paths| {
            let _ = sender.lock().unwrap().send(paths);
        })
        .unwrap();
        // FSEvents starts reporting a moment after the stream starts.
        std::thread::sleep(Duration::from_millis(500));
        std::fs::write(folder.join("Group/Section.one"), b"after").unwrap();
        let mut reported = Vec::new();
        while let Ok(paths) = changes.recv_timeout(Duration::from_secs(5)) {
            reported.extend(paths);
            if reported.iter().any(|path| path == "Group/Section.one") {
                break;
            }
        }
        drop(watching);
        std::fs::remove_dir_all(&folder).unwrap();
        assert!(
            reported.iter().any(|path| path == "Group/Section.one"),
            "{reported:?}"
        );
    }
}
