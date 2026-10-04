use crate::{owned, report, string};
use std::ffi::c_char;

#[unsafe(no_mangle)]
pub extern "C" fn sb_crash_address() -> *const c_char {
    crash_report::ADDRESS.as_ptr()
}

/// # Safety
/// Each argument is NUL-terminated UTF-8; `path` is a private local report file.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sb_crash_start(
    path: *const c_char,
    build: *const c_char,
    system: *const c_char,
) -> bool {
    crash_report::hook(
        &string(build),
        format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
        string(system),
        |report| eprint!("{report}"),
    );
    report(crash_report::set_path(string(path).into()).map_err(Into::into)).is_some()
}

/// The saved report, freed with `sb_string_free`, or null.
#[unsafe(no_mangle)]
pub extern "C" fn sb_crash_report() -> *mut c_char {
    crash_report::kept().map_or(std::ptr::null_mut(), owned)
}

#[unsafe(no_mangle)]
pub extern "C" fn sb_crash_forget() {
    crash_report::forget();
}
