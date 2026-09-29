//! Mac OS X 10.6's window, scrollers and resize grip. The window is textured, its title's
//! gradient running on through the toolbar's row. Its scrollers sit beside the content
//! rather than over it, so AppKit paints them here, into bitmaps the interface draws; it
//! resizes only from the grip, which the OpenGL surface covers, so the app draws that.

use objc2::{
    ClassType,
    declare::ClassBuilder,
    msg_send, msg_send_id,
    rc::{Allocated, Retained},
    runtime::{AnyClass, AnyObject, Bool, Sel},
    sel,
};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize};
use std::collections::HashMap;
use ui::{Axis, PaintedScroller, Scroller, ScrollerPart};
use winit::{
    raw_window_handle::{HasWindowHandle, RawWindowHandle},
    window::{CursorIcon, Window},
};

/// Whether the system predates 10.7, which moved scrollers over the content, gave
/// NSScroller its styles and let windows resize from any edge.
pub(crate) fn before_lion() -> bool {
    let scroller = AnyClass::get("NSScroller").expect("AppKit is linked");
    let styled: bool =
        unsafe { msg_send![scroller, respondsToSelector: sel!(preferredScrollerStyle)] };
    !styled
}

/// Makes the window textured, AppKit's gradient running from the title through `row` points
/// of content below it, as 10.6's unified toolbars do. Where frames are transparent it
/// shows through. Returns whether the system draws windows so, which only 10.6 does here.
pub fn textured(window: &Window, row: f32) -> bool {
    if !before_lion() {
        return false;
    }
    let RawWindowHandle::AppKit(handle) =
        window.window_handle().expect("Live AppKit window").as_raw()
    else {
        unreachable!()
    };
    unsafe {
        let view = &*handle.ns_view.as_ptr().cast::<AnyObject>();
        let window: Retained<AnyObject> = msg_send_id![view, window];
        let title: Retained<AnyObject> = msg_send_id![&window, title];
        let mask: usize = msg_send![&window, styleMask];
        // NSTexturedBackgroundWindowMask.
        let _: () = msg_send![&window, setStyleMask: mask | 1 << 8];
        // The style change leaves 10.6's title nil, which winit's getter can't return.
        let _: () = msg_send![&window, setTitle: &*title];
        // NSMaxYEdge.
        let _: () =
            msg_send![&window, setAutorecalculatesContentBorderThickness: false, forEdge: 3usize];
        let _: () = msg_send![&window, setContentBorderThickness: f64::from(row), forEdge: 3usize];
    }
    true
}

/// `-mouseDownCanMoveWindow` for the content view: a textured window drags from any press
/// on a view that allows it, asking once. The app drags from its toolbar's empty space
/// itself, through `-performWindowDragWithEvent:`.
pub extern "C" fn no_window_drags(_: &AnyObject, _: Sel) -> Bool {
    Bool::NO
}

/// The cursor for moving something: winit loads its move cursor from a folder of cursors
/// 10.6 doesn't have, so there it is the open hand.
pub fn move_cursor() -> CursorIcon {
    if before_lion() {
        CursorIcon::Grab
    } else {
        CursorIcon::Move
    }
}

/// NSScrollerKnob, NSScrollerKnobSlot, NSScrollerDecrementLine, NSScrollerIncrementLine.
const KNOB: usize = 2;
const SLOT: usize = 6;
const DECREMENT_LINE: usize = 4;
const INCREMENT_LINE: usize = 5;

extern "C" fn yes(_: &AnyObject, _: Sel) -> Bool {
    Bool::YES
}

/// AppKit's scrollers, on systems whose scrollers sit beside the content.
pub fn scrollers() -> Option<ui::Scrollers> {
    if !before_lion() {
        return None;
    }
    MainThreadMarker::new().expect("Views belong to the main thread");
    let window_class = objc2_app_kit::NSWindow::class();
    // A scroller colours its knob by whether its window is key; these windows never show.
    let mut key = ClassBuilder::new("SnowboundKeyWindow", window_class).expect("Unique class");
    unsafe {
        key.add_method(sel!(isKeyWindow), yes as extern "C" fn(_, _) -> _);
        key.add_method(sel!(isMainWindow), yes as extern "C" fn(_, _) -> _);
    }
    let key = key.register();
    let windows = [window_class, key].map(|class| unsafe {
        let window: Allocated<AnyObject> = msg_send_id![class, alloc];
        let window: Retained<AnyObject> = msg_send_id![
            window,
            initWithContentRect: NSRect::new(NSPoint::new(-10000.0, -10000.0), NSSize::new(16.0, 16.0)),
            styleMask: 0usize,
            backing: 2usize,
            defer: true
        ];
        let _: () = msg_send![&window, setReleasedWhenClosed: false];
        window
    });
    let thickness: f64 = unsafe { msg_send![AnyClass::get("NSScroller").unwrap(), scrollerWidth] };
    let mut views: HashMap<(bool, bool), Retained<AnyObject>> = HashMap::new();
    let mut painted: HashMap<[u32; 6], PaintedScroller> = HashMap::new();
    Some(ui::Scrollers {
        thickness: thickness as f32,
        paint: Box::new(move |request: &Scroller| {
            let key = [
                u32::from(request.axis == Axis::Y),
                request.length.to_bits(),
                request.value.to_bits(),
                request.proportion.to_bits(),
                request.held.map_or(0, |part| part as u32 + 1),
                u32::from(request.active),
            ];
            if let Some(cached) = painted.get(&key) {
                return cached.clone();
            }
            let vertical = request.axis == Axis::Y;
            let window = &windows[usize::from(request.active)];
            let view = views
                .entry((request.active, vertical))
                .or_insert_with(|| unsafe {
                    let view: Allocated<AnyObject> =
                        msg_send_id![AnyClass::get("NSScroller").unwrap(), alloc];
                    let size = if vertical {
                        NSSize::new(thickness, 100.0)
                    } else {
                        NSSize::new(100.0, thickness)
                    };
                    let view: Retained<AnyObject> =
                        msg_send_id![view, initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), size)];
                    let content: Retained<AnyObject> = msg_send_id![window, contentView];
                    let _: () = msg_send![&content, addSubview: &*view];
                    view
                });
            let result = unsafe { paint(window, view, request, thickness) };
            if painted.len() > 32 {
                painted.clear();
            }
            painted.insert(key, result.clone());
            result
        }),
    })
}

unsafe fn paint(
    window: &AnyObject,
    view: &AnyObject,
    request: &Scroller,
    thickness: f64,
) -> PaintedScroller {
    let vertical = request.axis == Axis::Y;
    let length = f64::from(request.length);
    let size = if vertical {
        NSSize::new(thickness, length)
    } else {
        NSSize::new(length, thickness)
    };
    unsafe {
        let _: () = msg_send![window, setContentSize: size];
        let _: () = msg_send![view, setFrame: NSRect::new(NSPoint::new(0.0, 0.0), size)];
        let _: () = msg_send![view, setEnabled: true];
        let _: () = msg_send![view, setKnobProportion: f64::from(request.proportion)];
        let _: () = msg_send![view, setDoubleValue: f64::from(request.value)];
        let bounds: NSRect = msg_send![view, bounds];
        let rep: Retained<AnyObject> =
            msg_send_id![view, bitmapImageRepForCachingDisplayInRect: bounds];
        let _: () = msg_send![view, cacheDisplayInRect: bounds, toBitmapImageRep: &*rep];
        // NSScrollerIncrementArrow, NSScrollerDecrementArrow.
        let arrow = match request.held {
            Some(ScrollerPart::Increment) => Some(0usize),
            Some(ScrollerPart::Decrement) => Some(1usize),
            _ => None,
        };
        if let Some(arrow) = arrow {
            let class = AnyClass::get("NSGraphicsContext").unwrap();
            let context: Retained<AnyObject> =
                msg_send_id![class, graphicsContextWithBitmapImageRep: &*rep];
            let _: () = msg_send![class, saveGraphicsState];
            let _: () = msg_send![class, setCurrentContext: &*context];
            let flipped: bool = msg_send![view, isFlipped];
            if flipped {
                let transform: Retained<AnyObject> =
                    msg_send_id![AnyClass::get("NSAffineTransform").unwrap(), transform];
                let _: () = msg_send![&transform, translateXBy: 0.0f64, yBy: size.height];
                let _: () = msg_send![&transform, scaleXBy: 1.0f64, yBy: -1.0f64];
                let _: () = msg_send![&transform, concat];
            }
            let _: () = msg_send![view, drawArrow: arrow, highlight: true];
            let _: () = msg_send![class, restoreGraphicsState];
        }
        let along = |part: usize| {
            let rect: NSRect = msg_send![view, rectForPart: part];
            let (start, extent) = if vertical {
                (rect.origin.y, rect.size.height)
            } else {
                (rect.origin.x, rect.size.width)
            };
            [start as f32, (start + extent) as f32]
        };
        PaintedScroller {
            image: pixels(&rep),
            knob: along(KNOB),
            slot: along(SLOT),
            decrement: along(DECREMENT_LINE),
            increment: along(INCREMENT_LINE),
        }
    }
}

/// An NSBitmapImageRep's pixels as premultiplied RGBA rows.
unsafe fn pixels(rep: &AnyObject) -> draw::RasterImage {
    unsafe {
        let width: isize = msg_send![rep, pixelsWide];
        let height: isize = msg_send![rep, pixelsHigh];
        let row: isize = msg_send![rep, bytesPerRow];
        let samples: isize = msg_send![rep, samplesPerPixel];
        // NSAlphaFirstBitmapFormat, NSAlphaNonpremultipliedBitmapFormat.
        let format: usize = msg_send![rep, bitmapFormat];
        let data: *const u8 = msg_send![rep, bitmapData];
        let [width, height, row, samples] = [width, height, row, samples].map(|v| v as usize);
        let bytes = std::slice::from_raw_parts(data, row * height);
        let mut rgba = Vec::with_capacity(width * height * 4);
        for line in bytes.chunks(row) {
            for pixel in line[..width * samples].chunks(samples) {
                let [r, g, b, a] = match (samples, format & 1 != 0) {
                    (4, true) => [pixel[1], pixel[2], pixel[3], pixel[0]],
                    (4, false) => [pixel[0], pixel[1], pixel[2], pixel[3]],
                    _ => [pixel[0], pixel[1], pixel[2], 255],
                };
                let premultiply = |value: u8| (u16::from(value) * u16::from(a) / 255) as u8;
                rgba.extend(if format & 2 != 0 {
                    [premultiply(r), premultiply(g), premultiply(b), a]
                } else {
                    [r, g, b, a]
                });
            }
        }
        draw::RasterImage::new([width as u32, height as u32], rgba)
            .expect("A scroller's bitmap is a valid image")
    }
}

/// Where each pixel of the grip lies from the window's bottom-right corner, it takes a
/// diagonal's black at this opacity: a line, then a shade either side, as 10.6 draws it.
fn grip_alpha(diagonal: u32) -> u8 {
    match diagonal % 4 {
        3 => 102,
        2 => 31,
        0 => 18,
        _ => 0,
    }
}

/// 10.6's resize grip at the window's bottom-right corner, over everything the app draws,
/// where the OpenGL surface hides AppKit's own. AppKit still takes the drag.
pub fn resize_grip(ui: &mut ui::Ui, window: &Window, size: [f32; 2]) {
    if !before_lion() {
        return;
    }
    static HIDDEN: std::sync::Once = std::sync::Once::new();
    HIDDEN.call_once(|| {
        let RawWindowHandle::AppKit(handle) =
            window.window_handle().expect("Live AppKit window").as_raw()
        else {
            unreachable!()
        };
        unsafe {
            let view = &*handle.ns_view.as_ptr().cast::<AnyObject>();
            let window: Retained<AnyObject> = msg_send_id![view, window];
            let _: () = msg_send![&window, setShowsResizeIndicator: false];
        }
    });
    thread_local! {
        static GRIP: draw::RasterImage = {
            // Three diagonals within 15 pixels of the corner, the last pixel row and column
            // left clear.
            let pixels = (0..15u32)
                .flat_map(|y| (0..15u32).map(move |x| (x, y)))
                .flat_map(|(x, y)| {
                    let alpha = if x < 14 && y < 14 && x + y >= 16 {
                        grip_alpha(x + y + 2)
                    } else {
                        0
                    };
                    [0, 0, 0, alpha]
                })
                .collect();
            draw::RasterImage::new([15, 15], pixels).expect("The grip is a valid image")
        };
    }
    GRIP.with(|grip| {
        ui.leaf(
            "resize grip",
            ui::Spec {
                flags: ui::Flags::FLOAT,
                size: [ui::px(15.0), ui::px(15.0)],
                position: [size[0] - 15.0, size[1] - 15.0],
                image: Some(grip),
                ..ui::Spec::default()
            },
        );
    });
}
