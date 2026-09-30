//! Printing on macOS: AppKit's print panel as a sheet on the window, printing the PDF
//! through PDFKit, which fits each sheet to the paper chosen. Mac OS X 10.6's PDFKit
//! cannot print a document, so there the PDF opens in Preview.

use objc2::{
    msg_send, msg_send_id,
    rc::Retained,
    runtime::{AnyClass, AnyObject, Bool, Sel},
    sel,
};
use objc2_foundation::{MainThreadMarker, NSSize, NSString};
use std::error::Error;
use winit::{
    raw_window_handle::{HasWindowHandle, RawWindowHandle},
    window::Window,
};

// PDFKit, inside Quartz on every macOS.
#[link(name = "Quartz", kind = "framework")]
unsafe extern "C" {}

fn class(name: &str) -> &'static AnyClass {
    AnyClass::get(name).expect("AppKit and PDFKit are linked")
}

/// The paper Page Setup defaults to, in points.
pub fn paper() -> [f32; 2] {
    MainThreadMarker::new().expect("Printing belongs to the main thread");
    unsafe {
        let info: Retained<AnyObject> = msg_send_id![class("NSPrintInfo"), sharedPrintInfo];
        let size: NSSize = msg_send![&info, paperSize];
        if size.width > 0.0 && size.height > 0.0 {
            [size.width as f32, size.height as f32]
        } else {
            canvas::print::LETTER
        }
    }
}

/// PDFKit's print operation for `pdf` fitted to `info`'s paper, where PDFKit prints.
fn operation(pdf: &[u8], info: &AnyObject) -> Option<Retained<AnyObject>> {
    unsafe {
        let data: Retained<AnyObject> = msg_send_id![
            class("NSData"),
            dataWithBytes: pdf.as_ptr().cast::<std::ffi::c_void>(),
            length: pdf.len()
        ];
        let document: Option<Retained<AnyObject>> =
            msg_send_id![msg_send_id![class("PDFDocument"), alloc], initWithData: &*data];
        let document = document?;
        let prints: bool = msg_send![
            &document,
            respondsToSelector: sel!(printOperationForPrintInfo:scalingMode:autoRotate:)
        ];
        if !prints {
            return None;
        }
        // kPDFPrintPageScaleToFit.
        msg_send_id![
            &document,
            printOperationForPrintInfo: info,
            scalingMode: 1_isize,
            autoRotate: Bool::YES
        ]
    }
}

/// Shows the print panel for `pdf` as a sheet on `window`.
pub fn print(window: &Window, pdf: &[u8], title: &str) -> Result<(), Box<dyn Error>> {
    MainThreadMarker::new().expect("Printing belongs to the main thread");
    unsafe {
        let shared: Retained<AnyObject> = msg_send_id![class("NSPrintInfo"), sharedPrintInfo];
        let info: Retained<AnyObject> = msg_send_id![&shared, copy];
        let Some(operation) = operation(pdf, &info) else {
            crate::platform::reveal(crate::print::spooled(pdf, title)?);
            return Ok(());
        };
        let _: () = msg_send![&operation, setJobTitle: &*NSString::from_str(title)];
        let RawWindowHandle::AppKit(handle) = window.window_handle()?.as_raw() else {
            unreachable!("A macOS window is AppKit's")
        };
        let view = &*handle.ns_view.as_ptr().cast::<AnyObject>();
        let window: Retained<AnyObject> = msg_send_id![view, window];
        let _: () = msg_send![
            &operation,
            runOperationModalForWindow: &*window,
            delegate: std::ptr::null::<AnyObject>(),
            didRunSelector: None::<Sel>,
            contextInfo: std::ptr::null_mut::<std::ffi::c_void>()
        ];
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The print operation `print` runs, saving to a file instead of showing the panel.
    #[test]
    fn pdfkit_prints_the_pages() {
        let folder = std::env::temp_dir().join(format!("snowbound-print-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let printed = folder.join("printed.pdf");
        let sheet = draw::Sheet {
            size: canvas::print::LETTER,
            layers: Vec::new(),
        };
        let pdf = draw::pdf(
            "Two sheets",
            &[
                sheet,
                draw::Sheet {
                    size: canvas::print::LETTER,
                    layers: Vec::new(),
                },
            ],
        )
        .unwrap();
        unsafe {
            let info: Retained<AnyObject> = msg_send_id![class("NSPrintInfo"), new];
            let url: Retained<AnyObject> = msg_send_id![
                class("NSURL"),
                fileURLWithPath: &*NSString::from_str(printed.to_str().unwrap())
            ];
            let settings: Retained<AnyObject> = msg_send_id![&info, dictionary];
            let _: () = msg_send![
                &settings,
                setObject: &*url,
                forKey: &*NSString::from_str("NSJobSavingURL")
            ];
            let _: () = msg_send![&info, setJobDisposition: &*NSString::from_str("NSPrintSaveJob")];
            let operation = operation(&pdf, &info).expect("PDFKit prints documents");
            let _: () = msg_send![&operation, setShowsPrintPanel: false];
            let _: () = msg_send![&operation, setShowsProgressPanel: false];
            let ran: bool = msg_send![&operation, runOperation];
            assert!(ran);
        }
        let bytes = std::fs::read(&printed).unwrap();
        unsafe {
            let data: Retained<AnyObject> = msg_send_id![
                class("NSData"),
                dataWithBytes: bytes.as_ptr().cast::<std::ffi::c_void>(),
                length: bytes.len()
            ];
            let document: Retained<AnyObject> =
                msg_send_id![msg_send_id![class("PDFDocument"), alloc], initWithData: &*data];
            let pages: usize = msg_send![&document, pageCount];
            assert_eq!(pages, 2);
        }
        std::fs::remove_dir_all(&folder).unwrap();
    }
}
