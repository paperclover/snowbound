//! One running app per cache folder on Windows and Linux. A second launch hands the files it
//! was asked to open to the first and exits, as Launch Services does for the macOS bundle: two
//! processes on one cache would wait on each other's section replicas. The first listens on a
//! named pipe on Windows and on a Unix socket in the cache folder on Linux.

use std::path::{Path, PathBuf};
use winit::event_loop::EventLoopProxy;

/// The launcher's activation token, which may be empty, then the files a launch asks to open,
/// each ended by a NUL, which neither holds.
fn encode(token: Option<&str>, paths: &[PathBuf]) -> Vec<u8> {
    let mut bytes = token.unwrap_or_default().as_bytes().to_vec();
    bytes.push(0);
    for path in paths {
        bytes.extend(path_bytes(path));
        bytes.push(0);
    }
    bytes
}

fn decode(bytes: &[u8]) -> (Option<String>, Vec<PathBuf>) {
    let mut fields = bytes.split(|&byte| byte == 0);
    let token = fields
        .next()
        .filter(|token| !token.is_empty())
        .map(|token| String::from_utf8_lossy(token).into_owned());
    let paths = fields
        .filter(|path| !path.is_empty())
        .map(bytes_path)
        .collect();
    (token, paths)
}

/// Opens what each later launch hands over, on a thread of its own.
fn serve<C: std::io::Read + Send + 'static>(
    proxy: EventLoopProxy<crate::UserEvent>,
    mut accept: impl FnMut() -> Option<C> + Send + 'static,
) {
    std::thread::spawn(move || {
        while let Some(mut connection) = accept() {
            let mut bytes = Vec::new();
            if connection.read_to_end(&mut bytes).is_ok() {
                #[cfg_attr(not(target_os = "linux"), allow(unused_variables))]
                let (token, paths) = decode(&bytes);
                let _ = proxy.send_event(crate::UserEvent::Open(paths));
                #[cfg(target_os = "linux")]
                if let Some(token) = token {
                    let _ = proxy.send_event(crate::UserEvent::Activate(token));
                }
            }
        }
    });
}

#[cfg(unix)]
fn path_bytes(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().to_vec()
}

#[cfg(unix)]
fn bytes_path(bytes: &[u8]) -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::OsStr::from_bytes(bytes).into()
}

#[cfg(windows)]
fn path_bytes(path: &Path) -> Vec<u8> {
    path.to_string_lossy().into_owned().into_bytes()
}

#[cfg(windows)]
fn bytes_path(bytes: &[u8]) -> PathBuf {
    String::from_utf8_lossy(bytes).into_owned().into()
}

#[cfg(target_os = "linux")]
pub use self::linux::*;

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::{
        io::{ErrorKind, Write},
        os::unix::net::{UnixListener, UnixStream},
    };

    /// Listening, unless the socket couldn't be made.
    pub struct Instance(Option<UnixListener>);

    /// This launch as the running app for `cache`, or None once the running one took `paths`.
    pub fn claim(cache: &Path, paths: &[PathBuf]) -> Option<Instance> {
        let socket = cache.join("instance.sock");
        for _ in 0..3 {
            match UnixStream::connect(&socket) {
                Ok(mut stream) => {
                    let token = crate::desktop::activation_token();
                    if stream.write_all(&encode(token.as_deref(), paths)).is_ok() {
                        return None;
                    }
                }
                // The socket of an app that ended without removing it.
                Err(error) if error.kind() == ErrorKind::ConnectionRefused => {
                    let _ = std::fs::remove_file(&socket);
                }
                Err(_) => {}
            }
            let _ = std::fs::create_dir_all(cache);
            match UnixListener::bind(&socket) {
                Ok(listener) => return Some(Instance(Some(listener))),
                // Another launch bound it first.
                Err(error) if error.kind() == ErrorKind::AddrInUse => {}
                Err(error) => {
                    eprintln!("Cannot listen at {}: {error}", socket.display());
                    break;
                }
            }
        }
        Some(Instance(None))
    }

    impl Instance {
        pub fn serve(self, proxy: EventLoopProxy<crate::UserEvent>) {
            if let Some(listener) = self.0 {
                super::serve(proxy, move || listener.incoming().find_map(Result::ok));
            }
        }
    }
}

#[cfg(windows)]
pub use self::windows::*;

#[cfg(windows)]
mod windows {
    use super::*;
    use std::{fs::File, io::Write, os::windows::io::FromRawHandle};
    use windows_sys::Win32::{
        Foundation::{
            ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, GENERIC_WRITE, GetLastError,
            INVALID_HANDLE_VALUE,
        },
        Storage::FileSystem::{
            CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, OPEN_EXISTING, PIPE_ACCESS_INBOUND,
        },
        System::Pipes::{
            ConnectNamedPipe, CreateNamedPipeW, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE,
            PIPE_UNLIMITED_INSTANCES, PIPE_WAIT, WaitNamedPipeW,
        },
        UI::WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow},
    };

    pub struct Instance {
        name: Vec<u16>,
        /// The pipe's first end, unless the name couldn't be claimed.
        first: Option<File>,
    }

    /// The pipe named for `cache`, which a pipe's name may hold but for its backslashes.
    fn pipe_name(cache: &Path) -> Vec<u16> {
        let cache = cache.to_string_lossy().replace('\\', "/").to_lowercase();
        format!(r"\\.\pipe\Snowbound\{cache}")
            .encode_utf16()
            .chain([0])
            .collect()
    }

    /// A new end of the pipe for the next launch to write to; the first claims the name.
    fn listen(name: &[u16], first: bool) -> Option<File> {
        let flags = if first {
            FILE_FLAG_FIRST_PIPE_INSTANCE
        } else {
            0
        };
        let handle = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                PIPE_ACCESS_INBOUND | flags,
                PIPE_TYPE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                0,
                64 * 1024,
                0,
                std::ptr::null(),
            )
        };
        (handle != INVALID_HANDLE_VALUE).then(|| unsafe { File::from_raw_handle(handle) })
    }

    /// This launch as the running app for `cache`, or None once the running one took `paths`.
    pub fn claim(cache: &Path, paths: &[PathBuf]) -> Option<Instance> {
        let name = pipe_name(cache);
        for _ in 0..3 {
            let handle = unsafe {
                CreateFileW(
                    name.as_ptr(),
                    GENERIC_WRITE,
                    0,
                    std::ptr::null(),
                    OPEN_EXISTING,
                    0,
                    std::ptr::null_mut(),
                )
            };
            if handle != INVALID_HANDLE_VALUE {
                // The running app may then bring its window to the front.
                unsafe { AllowSetForegroundWindow(ASFW_ANY) };
                let mut pipe = unsafe { File::from_raw_handle(handle) };
                if pipe.write_all(&encode(None, paths)).is_ok() {
                    return None;
                }
            } else if unsafe { GetLastError() } == ERROR_PIPE_BUSY {
                unsafe { WaitNamedPipeW(name.as_ptr(), 2000) };
                continue;
            }
            if let Some(first) = listen(&name, true) {
                return Some(Instance {
                    name,
                    first: Some(first),
                });
            }
        }
        eprintln!("Cannot reach the running Snowbound; opening another.");
        Some(Instance { name, first: None })
    }

    impl Instance {
        pub fn serve(self, proxy: EventLoopProxy<crate::UserEvent>) {
            let Instance { name, first } = self;
            let mut waiting = first;
            super::serve(proxy, move || {
                loop {
                    let pipe = waiting.take()?;
                    let handle = std::os::windows::io::AsRawHandle::as_raw_handle(&pipe);
                    let connected = unsafe { ConnectNamedPipe(handle, std::ptr::null_mut()) } != 0
                        || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
                    // The next end opens before this one closes, so the name never lapses.
                    waiting = listen(&name, false);
                    if connected {
                        return Some(pipe);
                    }
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handed_token_and_paths_read_back() {
        let paths = [
            PathBuf::from("/notes/Personal/Garden.one"),
            PathBuf::from("/notes/a b/Open Notebook.onetoc2"),
        ];
        assert_eq!(decode(&encode(None, &paths)), (None, paths.to_vec()));
        let token = Some("gnome-shell/Snowbound/1234-0-host_TIME5678".to_owned());
        assert_eq!(decode(&encode(token.as_deref(), &[])), (token, vec![]));
    }
}
