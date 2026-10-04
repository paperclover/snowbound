use super::{KEPT, Ordering};
use std::{path::Path, sync::OnceLock};

pub(super) static HEADING: OnceLock<String> = OnceLock::new();
// Encoding the path before a fault keeps allocation out of the handler.
#[cfg(unix)]
static PATH: OnceLock<std::ffi::CString> = OnceLock::new();
#[cfg(windows)]
static PATH: OnceLock<Vec<u16>> = OnceLock::new();

pub(super) fn prepare(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let path = std::ffi::CString::new(path.as_os_str().as_bytes())?;
        let _ = PATH.set(path);
        // Linux forwards its existing signal handler, preserving its symbolized log.
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        for signal in [
            libc::SIGSEGV,
            libc::SIGBUS,
            libc::SIGILL,
            libc::SIGFPE,
            libc::SIGABRT,
        ] {
            let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
            action.sa_sigaction = fatal as *const () as libc::sighandler_t;
            action.sa_flags = libc::SA_SIGINFO | libc::SA_RESETHAND;
            unsafe {
                libc::sigemptyset(&mut action.sa_mask);
                libc::sigaction(signal, &action, std::ptr::null_mut());
            }
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let path: Vec<_> = path.as_os_str().encode_wide().chain([0]).collect();
        if path[..path.len() - 1].contains(&0) {
            return Err(std::io::ErrorKind::InvalidInput.into());
        }
        let _ = PATH.set(path);
        unsafe {
            windows_sys::Win32::System::Diagnostics::Debug::SetUnhandledExceptionFilter(Some(
                fault,
            ));
        }
    }
    Ok(())
}

fn hex(mut value: u64) -> [u8; 16] {
    let mut digits = [b'0'; 16];
    for digit in digits.iter_mut().rev() {
        *digit = b"0123456789abcdef"[(value & 15) as usize];
        value >>= 4;
    }
    digits
}

/// Records the fatal signal without allocating, locking, or reading note data.
///
/// # Safety
/// `info` is the live `siginfo_t` supplied to a fatal signal handler.
#[cfg(unix)]
pub unsafe fn record_signal(signal: i32, info: *const libc::siginfo_t) {
    if PATH.get().is_none() || KEPT.swap(true, Ordering::Relaxed) {
        return;
    }
    let Some(path) = PATH.get() else {
        return;
    };
    let fd = unsafe {
        libc::open(
            path.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC | libc::O_NOFOLLOW,
            0o600,
        )
    };
    if fd < 0 {
        return;
    }
    let code = hex(signal as u64);
    let write = |parts: &[&[u8]]| {
        for mut bytes in parts.iter().copied() {
            while !bytes.is_empty() {
                let wrote = unsafe { libc::write(fd, bytes.as_ptr().cast(), bytes.len()) };
                if wrote <= 0 {
                    break;
                }
                bytes = &bytes[wrote as usize..];
            }
        }
    };
    write(&[
        HEADING
            .get()
            .map_or(&b"Snowbound\n"[..], |heading| heading.as_bytes()),
        b"Native signal: 0x",
        &code,
        b"\n",
    ]);
    if unsafe { (*info).si_code } > 0
        && matches!(
            signal,
            libc::SIGSEGV | libc::SIGBUS | libc::SIGILL | libc::SIGFPE
        )
    {
        let address = hex(unsafe { (*info).si_addr() } as usize as u64);
        write(&[b"Fault address: 0x", &address, b"\n"]);
    }
    unsafe {
        libc::fsync(fd);
        libc::close(fd);
    }
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
extern "C" fn fatal(signal: i32, info: *mut libc::siginfo_t, _: *mut libc::c_void) {
    unsafe {
        record_signal(signal, info);
        libc::raise(signal);
    }
}

#[cfg(windows)]
unsafe extern "system" fn fault(
    pointers: *const windows_sys::Win32::System::Diagnostics::Debug::EXCEPTION_POINTERS,
) -> i32 {
    use windows_sys::Win32::{
        Foundation::{GENERIC_WRITE, INVALID_HANDLE_VALUE},
        Storage::FileSystem::{
            CREATE_ALWAYS, CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_WRITE_THROUGH, WriteFile,
        },
    };
    let Some(path) = PATH.get() else {
        return 0;
    };
    if KEPT.swap(true, Ordering::Relaxed) {
        return 0;
    }
    let file = unsafe {
        CreateFileW(
            path.as_ptr(),
            GENERIC_WRITE,
            0,
            std::ptr::null(),
            CREATE_ALWAYS,
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_WRITE_THROUGH,
            std::ptr::null_mut(),
        )
    };
    if file == INVALID_HANDLE_VALUE {
        return 0;
    }
    let record = unsafe { &*(*pointers).ExceptionRecord };
    let code = hex(record.ExceptionCode as u32 as u64);
    let address = hex(record.ExceptionAddress as usize as u64);
    for bytes in [
        HEADING
            .get()
            .map_or(&b"Snowbound\n"[..], |heading| heading.as_bytes()),
        b"Native exception: 0x",
        &code,
        b"\nInstruction address: 0x",
        &address,
        b"\n",
    ] {
        let mut wrote = 0;
        unsafe {
            WriteFile(
                file,
                bytes.as_ptr(),
                bytes.len() as u32,
                &mut wrote,
                std::ptr::null_mut(),
            );
        }
    }
    unsafe {
        windows_sys::Win32::Foundation::CloseHandle(file);
    }
    0
}
