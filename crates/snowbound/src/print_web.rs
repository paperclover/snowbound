//! Printing in the browser: the PDF Snowbound prints downloads, to print from the viewer.

use crate::platform::{Window, download};
use std::error::Error;

/// Letter where the browser's language is American English, A4 elsewhere.
pub fn paper() -> [f32; 2] {
    if crate::platform::input_language() == "en-US" {
        canvas::print::LETTER
    } else {
        canvas::print::A4
    }
}

pub fn print(_: &Window, pdf: &[u8], title: &str) -> Result<(), Box<dyn Error>> {
    download(&format!("{title}.pdf"), pdf, "application/pdf");
    Ok(())
}
