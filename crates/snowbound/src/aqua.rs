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
use std::{
    collections::HashMap,
    sync::atomic::{AtomicBool, Ordering},
};
use ui::{Axis, PaintedScroller, Scroller, ScrollerPart};
use winit::window::{CursorIcon, Window};

static PRETEND: AtomicBool = AtomicBool::new(false);

/// Lays the app out as on 10.6, for `--screenshot` to draw a Snow Leopard window anywhere.
pub fn pretend() {
    PRETEND.store(true, Ordering::Relaxed);
}

/// Whether the system predates 10.7, which moved scrollers over the content, gave
/// NSScroller its styles and let windows resize from any edge.
pub(crate) fn before_lion() -> bool {
    if PRETEND.load(Ordering::Relaxed) {
        return true;
    }
    let scroller = AnyClass::get("NSScroller").expect("AppKit is linked");
    let styled: bool =
        unsafe { msg_send![scroller, respondsToSelector: sel!(preferredScrollerStyle)] };
    !styled
}

/// Makes the window textured, AppKit's gradient running from the title through `row` points
/// of content below it, as 10.6's unified toolbars run theirs. Where frames are transparent it
/// shows through. Returns whether the system draws windows so, which only 10.6 does here.
pub fn textured(window: &Window, row: f32) -> bool {
    if !before_lion() {
        return false;
    }
    TEXTURED.store(true, Ordering::Relaxed);
    let window = crate::platform::ns_window(window);
    unsafe {
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

static TEXTURED: AtomicBool = AtomicBool::new(false);

/// 10.6 draws a dark line along the lower edge of a textured window's top content border.
/// The app paints the window's own grey over it, key or not, beneath everything else, so
/// the gradient runs straight into the notebook's frame and pane.
pub fn cover_border_line(ui: &mut ui::Ui, width: f32) {
    if !TEXTURED.load(Ordering::Relaxed) {
        return;
    }
    let grey = if ui.window_focused { 167 } else { 216 };
    ui.leaf(
        "border line",
        ui::Spec {
            flags: ui::Flags::FLOAT,
            size: [ui::px(width), ui::px(1.0)],
            position: [0.0, crate::TOOLBAR + crate::TAB_ROW - 1.0],
            fill: Some(draw::srgb(grey, grey, grey)),
            ..ui::Spec::default()
        },
    );
}

/// `-mouseDownCanMoveWindow` for the content view: a textured window drags from any press
/// on a view that allows it, and a title bar the view lies under drags and zooms from it,
/// buttons drawn there included. The app drags and zooms from its toolbar's empty space
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

/// 10.6's interface font, Lucida Grande, which fontique's system-ui doesn't name, and its
/// scrollers.
pub fn system_interface(ui: &mut ui::Ui) {
    if !before_lion() {
        return;
    }
    ui.set_system_font("Lucida Grande");
    ui.scrollers = Some(if PRETEND.load(Ordering::Relaxed) {
        sampled_scrollers()
    } else {
        scrollers()
    });
}

/// AppKit's scrollers.
fn scrollers() -> ui::Scrollers {
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
    ui::Scrollers {
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
    }
}

/// A scroller's pieces as 10.6 draws them, sampled with `remote.sh art`: a track with its
/// arrows and no knob, and the same with a knob, 400 pixels long.
struct Pieces {
    track: Vec<Vec<[u8; 4]>>,
    knob: Vec<Vec<[u8; 4]>>,
}

/// Lines of `png`'s pixels across `axis`, from its start: rows for a vertical scroller,
/// columns for a horizontal one.
fn lines(png: &[u8], axis: Axis) -> Vec<Vec<[u8; 4]>> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(png));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().expect("A sampled scroller is a PNG");
    let mut bytes = vec![0; reader.output_buffer_size().expect("Small")];
    let info = reader
        .next_frame(&mut bytes)
        .expect("A sampled scroller is a PNG");
    let [width, height] = [info.width, info.height].map(|side| side as usize);
    let pixel = |x: usize, y: usize| -> [u8; 4] {
        let at = (y * width + x) * 4;
        bytes[at..at + 4].try_into().unwrap()
    };
    match axis {
        Axis::Y => (0..height)
            .map(|y| (0..width).map(|x| pixel(x, y)).collect())
            .collect(),
        Axis::X => (0..width)
            .map(|x| (0..height).map(|y| pixel(x, y)).collect())
            .collect(),
    }
}

fn pieces(axis: Axis, active: bool) -> Pieces {
    macro_rules! art {
        ($name:literal) => {
            include_bytes!(concat!("../assets/snow-leopard/", $name))
        };
    }
    let [track, full]: [&[u8]; 2] = match (axis, active) {
        (Axis::Y, true) => [
            art!("scroller-vertical-track-key.png"),
            art!("scroller-vertical-key.png"),
        ],
        (Axis::Y, false) => [
            art!("scroller-vertical-track-other.png"),
            art!("scroller-vertical-other.png"),
        ],
        (Axis::X, true) => [
            art!("scroller-horizontal-track-key.png"),
            art!("scroller-horizontal-key.png"),
        ],
        (Axis::X, false) => [
            art!("scroller-horizontal-track-other.png"),
            art!("scroller-horizontal-other.png"),
        ],
    };
    let [track, full] = [track, full].map(|png| lines(png, axis));
    // The knob is where the two differ.
    let differs: Vec<usize> = (0..track.len())
        .filter(|&at| track[at] != full[at])
        .collect();
    let knob = full[differs[0]..=differs[differs.len() - 1]].to_vec();
    Pieces { track, knob }
}

/// Lines `length` long from `source`: its first `head` and last `tail` lines, and its
/// middle line repeated between them.
fn stretch(source: &[Vec<[u8; 4]>], length: usize, head: usize, tail: usize) -> Vec<Vec<[u8; 4]>> {
    let middle = &source[source.len() / 2];
    let head = head.min(length);
    let tail = tail.min(length - head);
    source[..head]
        .iter()
        .chain(std::iter::repeat_n(middle, length - head - tail))
        .chain(&source[source.len() - tail..])
        .cloned()
        .collect()
}

/// Scrollers put together from 10.6's own pieces, for a Snow Leopard window drawn
/// elsewhere. Parts lie where AppKit puts them: the track from 4 pixels in to 30 from the
/// end, then the decrement arrow's 14 and the increment arrow's 16.
fn sampled_scrollers() -> ui::Scrollers {
    const THICKNESS: usize = 15;
    let mut cache: HashMap<(bool, bool), Pieces> = HashMap::new();
    ui::Scrollers {
        thickness: THICKNESS as f32,
        paint: Box::new(move |request: &Scroller| {
            let pieces = cache
                .entry((request.axis == Axis::Y, request.active))
                .or_insert_with(|| pieces(request.axis, request.active));
            let length = request.length.round().max(60.0) as usize;
            let mut lines = stretch(&pieces.track, length, 12, 44);
            let slot = [4.0, length as f32 - 30.0];
            let room = slot[1] - slot[0];
            let knob_length = (room * request.proportion).clamp(20.0, room);
            let start = (slot[0] + request.value * (room - knob_length)).round();
            let knob = stretch(&pieces.knob, knob_length.round() as usize, 10, 10);
            for (line, knob) in lines[start as usize..].iter_mut().zip(knob) {
                *line = knob;
            }
            let pixels = match request.axis {
                Axis::Y => lines.concat(),
                Axis::X => (0..THICKNESS)
                    .flat_map(|across| lines.iter().map(move |line| line[across]))
                    .collect(),
            };
            let rgba = pixels
                .into_iter()
                .flat_map(|[r, g, b, a]| {
                    [r, g, b]
                        .map(|v| (u16::from(v) * u16::from(a) / 255) as u8)
                        .into_iter()
                        .chain([a])
                })
                .collect();
            let size = match request.axis {
                Axis::Y => [THICKNESS as u32, length as u32],
                Axis::X => [length as u32, THICKNESS as u32],
            };
            PaintedScroller {
                image: draw::RasterImage::new(size, rgba).expect("A scroller is a valid image"),
                knob: [start, start + knob_length.round()],
                slot,
                decrement: [length as f32 - 30.0, length as f32 - 16.0],
                increment: [length as f32 - 16.0, length as f32],
            }
        }),
    }
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
        let window = crate::platform::ns_window(window);
        let _: () = unsafe { msg_send![&window, setShowsResizeIndicator: false] };
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
