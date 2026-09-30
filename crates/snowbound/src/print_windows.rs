//! Printing on Windows: the PDF goes to the shell's print verb for PDFs, whose handler
//! shows the print dialog; where none prints PDFs, it opens in the viewer to print from.

use std::{error::Error, os::windows::ffi::OsStrExt};
use windows_sys::Win32::{
    Globalization::{GetLocaleInfoEx, LOCALE_IPAPERSIZE},
    UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL},
};
use winit::{
    raw_window_handle::{HasWindowHandle, RawWindowHandle},
    window::Window,
};

fn wide(text: impl AsRef<std::ffi::OsStr>) -> Vec<u16> {
    text.as_ref().encode_wide().chain([0]).collect()
}

/// The paper the user's locale prints on: 1 is US Letter, 5 Legal, 9 A4.
pub fn paper() -> [f32; 2] {
    let mut value = [0_u16; 4];
    // SAFETY: the buffer's length is passed with it; a null name is the user's locale.
    let length = unsafe {
        GetLocaleInfoEx(
            std::ptr::null(),
            LOCALE_IPAPERSIZE,
            value.as_mut_ptr(),
            value.len() as i32,
        )
    };
    let size = String::from_utf16_lossy(&value[..(length.max(1) - 1) as usize]);
    match size.as_str() {
        "9" => canvas::print::A4,
        _ => canvas::print::LETTER,
    }
}

pub fn print(window: &Window, pdf: &[u8], title: &str) -> Result<(), Box<dyn Error>> {
    let path = crate::print::spooled(pdf, title)?;
    let owner = match window.window_handle()?.as_raw() {
        RawWindowHandle::Win32(handle) => handle.hwnd.get() as _,
        _ => std::ptr::null_mut(),
    };
    for verb in ["print", "open"] {
        // SAFETY: the strings are NUL-terminated and outlive the call.
        let done = unsafe {
            ShellExecuteW(
                owner,
                wide(verb).as_ptr(),
                wide(&path).as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                SW_SHOWNORMAL,
            )
        };
        // ShellExecute's success is a pseudo-handle above 32.
        if done as isize > 32 {
            return Ok(());
        }
    }
    Err("No application on this computer prints or opens PDFs.".into())
}
