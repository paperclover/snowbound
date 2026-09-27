use canvas::date::DateField;
use objc2::{
    ClassType, DeclaredClass,
    declare::ClassBuilder,
    declare_class, msg_send, msg_send_id, mutability,
    rc::{Allocated, Retained},
    runtime::{AnyClass, AnyObject, Sel},
    sel,
};
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSAlertSecondButtonReturn, NSApplication, NSColor,
    NSColorSpace, NSDatePicker, NSDatePickerElementFlags, NSDatePickerStyle, NSEvent, NSEventType,
};
use objc2_foundation::{
    MainThreadMarker, NSAttributedString, NSCalendar, NSCalendarUnit, NSDate, NSDateFormatter,
    NSDateFormatterStyle, NSPoint, NSRange, NSRect, NSSize, NSString,
};
use std::{cell::Cell, sync::OnceLock};
use winit::{
    error::EventLoopError,
    event_loop::{EventLoop, EventLoopProxy},
    platform::macos::WindowAttributesExtMacOS,
    raw_window_handle::{HasWindowHandle, RawWindowHandle},
    window::{Window, WindowAttributes},
};

static QUIT: OnceLock<EventLoopProxy<crate::UserEvent>> = OnceLock::new();
static INPUT_CLASS: OnceLock<&'static AnyClass> = OnceLock::new();
thread_local! { static IN_KEY_DOWN: Cell<bool> = const { Cell::new(false) }; }

unsafe extern "C" fn key_down(view: &AnyObject, _: Sel, event: &NSEvent) {
    let previous = IN_KEY_DOWN.replace(true);
    unsafe {
        let superclass = INPUT_CLASS.get().unwrap().superclass().unwrap();
        let _: () = msg_send![super(view, superclass), keyDown: event];
    }
    IN_KEY_DOWN.set(previous);
}

unsafe extern "C" fn insert_text(view: &AnyObject, _: Sel, text: &AnyObject, range: NSRange) {
    unsafe {
        let marked: bool = msg_send![view, hasMarkedText];
        if IN_KEY_DOWN.get() || marked {
            let superclass = INPUT_CLASS.get().unwrap().superclass().unwrap();
            let _: () =
                msg_send![super(view, superclass), insertText: text replacementRange: range];
        } else {
            // NSTextInputClient supplies either NSString or NSAttributedString.
            let attributed: bool = msg_send![text, isKindOfClass: NSAttributedString::class()];
            let string: Retained<NSString> = if attributed {
                msg_send_id![text, string]
            } else {
                msg_send_id![text, copy]
            };
            if let Some(proxy) = QUIT.get() {
                let _ = proxy.send_event(crate::UserEvent::InsertText(string.to_string()));
            }
        }
    }
}

fn ns_window(window: &Window) -> Retained<AnyObject> {
    let RawWindowHandle::AppKit(handle) =
        window.window_handle().expect("Live AppKit window").as_raw()
    else {
        unreachable!()
    };
    unsafe {
        let view = &*handle.ns_view.as_ptr().cast::<AnyObject>();
        msg_send_id![view, window]
    }
}

/// Room the traffic lights take at the title bar's leading edge.
pub const LEADING: f32 = 78.0;
/// How far AppKit rounds a window's corners.
pub const CORNER_RADIUS: f32 = 10.0;

/// A transparent title bar over the content, where the window draws its own.
pub fn window_attributes() -> WindowAttributes {
    Window::default_attributes()
        .with_titlebar_transparent(true)
        .with_title_hidden(true)
        .with_fullsize_content_view(true)
}

pub struct Clipboard(arboard::Clipboard);

impl Clipboard {
    pub fn new(_: &Window) -> Result<Self, arboard::Error> {
        arboard::Clipboard::new().map(Self)
    }

    pub fn set_text(&mut self, text: String) -> Result<(), arboard::Error> {
        self.0.set_text(text)
    }

    pub fn get_text(&mut self) -> Result<String, arboard::Error> {
        self.0.get_text()
    }
}

/// AppKit draws the traffic lights.
pub fn window_controls(_: &mut ui::Ui, _: &Window) {}

/// The app draws the title bar around the traffic lights.
pub fn system_titlebar(_: &Window) -> bool {
    false
}

/// AppKit's window frame takes resizing presses.
pub fn resize_direction(_: &Window, _: [f32; 2]) -> Option<winit::window::ResizeDirection> {
    None
}

pub fn cache_dir() -> Option<std::path::PathBuf> {
    Some(std::path::PathBuf::from(std::env::var_os("HOME")?).join("Library/Caches/snowbound"))
}

pub fn settings_dir() -> Option<std::path::PathBuf> {
    Some(
        std::path::PathBuf::from(std::env::var_os("HOME")?)
            .join("Library/Application Support/Snowbound"),
    )
}

/// Asks for a notebook folder or a notebook file with the system's open panel, titled
/// `title`.
pub fn pick_notebook(title: &str) -> Option<std::path::PathBuf> {
    let mtm = MainThreadMarker::new().expect("Panels belong to the main thread");
    unsafe {
        let panel = objc2_app_kit::NSOpenPanel::openPanel(mtm);
        panel.setCanChooseDirectories(true);
        panel.setCanChooseFiles(true);
        panel.setCanCreateDirectories(true);
        panel.setTitle(Some(&NSString::from_str(title)));
        panel.setPrompt(Some(&NSString::from_str("Open")));
        if panel.runModal() != objc2_app_kit::NSModalResponseOK {
            return None;
        }
        let path = panel.URLs().firstObject()?.path()?;
        Some(path.to_string().into())
    }
}

/// Asks where to create something named `name` by default, with the system's save panel.
pub fn pick_new(title: &str, name: &str) -> Option<std::path::PathBuf> {
    let mtm = MainThreadMarker::new().expect("Panels belong to the main thread");
    unsafe {
        let panel = objc2_app_kit::NSSavePanel::savePanel(mtm);
        panel.setCanCreateDirectories(true);
        panel.setTitle(Some(&NSString::from_str(title)));
        panel.setPrompt(Some(&NSString::from_str("Create")));
        panel.setNameFieldStringValue(&NSString::from_str(name));
        if panel.runModal() != objc2_app_kit::NSModalResponseOK {
            return None;
        }
        Some(panel.URL()?.path()?.to_string().into())
    }
}

/// Tells the user something they asked for could not be done: `message`, then what to do.
pub fn alert(message: &str, detail: &str) {
    let mtm = MainThreadMarker::new().expect("Window events run on the main thread");
    unsafe {
        let alert = NSAlert::new(mtm);
        alert.setMessageText(&NSString::from_str(message));
        alert.setInformativeText(&NSString::from_str(detail));
        alert.runModal();
    }
}

pub fn appearance(window: &Window) -> winit::window::Theme {
    window.theme().unwrap_or(winit::window::Theme::Dark)
}

/// Zooms the window once the current event is handled: AppKit's zoom animation runs its
/// own loop, and started from inside winit's handler it would hold every resize until the
/// end, stretching the last frame instead of drawing each step.
pub fn zoom(window: &Window) {
    let window = ns_window(window);
    unsafe {
        let _: () = msg_send![
            &window,
            performSelector: sel!(zoom:),
            withObject: std::ptr::null::<AnyObject>(),
            afterDelay: 0.0f64
        ];
    }
}

/// Install before AccessKit subclasses the same view, preserving its restoration chain.
pub fn install_text_input(window: &Window) {
    MainThreadMarker::new().expect("Text input belongs to the main thread");
    let RawWindowHandle::AppKit(handle) =
        window.window_handle().expect("Live AppKit window").as_raw()
    else {
        unreachable!()
    };
    unsafe {
        let view = &*handle.ns_view.as_ptr().cast::<AnyObject>();
        let class = INPUT_CLASS.get_or_init(|| {
            let mut class = ClassBuilder::new("SnowboundTextInputView", view.class())
                .expect("Unique text input class");
            class.add_method(sel!(keyDown:), key_down as unsafe extern "C" fn(_, _, _));
            class.add_method(
                sel!(insertText:replacementRange:),
                insert_text as unsafe extern "C" fn(_, _, _, _),
            );
            class.register()
        });
        assert_eq!(class.superclass(), Some(view.class()));
        assert_eq!(class.instance_size(), view.class().instance_size());
        // The subclass adds no ivars, so the existing allocation remains valid.
        AnyObject::set_class(view, class);
    }
}

pub fn double_click_interval() -> std::time::Duration {
    std::time::Duration::from_secs_f64(unsafe { NSEvent::doubleClickInterval() })
}

pub fn show_character_palette() {
    let mtm = MainThreadMarker::new().expect("Text input belongs to the main thread");
    NSApplication::sharedApplication(mtm).orderFrontCharacterPalette(None);
}

pub fn edit_date(
    timestamp: u64,
    field: DateField,
    title: &str,
) -> Result<Option<(u64, [String; 2])>, &'static str> {
    let mtm = MainThreadMarker::new().expect("Date controls belong to the main thread");
    unsafe {
        let calendar = NSCalendar::currentCalendar();
        let picker = NSDatePicker::new(mtm);
        picker.setDatePickerStyle(NSDatePickerStyle::TextFieldAndStepper);
        picker.setDatePickerElements(match field {
            DateField::Date => NSDatePickerElementFlags::NSDatePickerElementFlagYearMonthDay,
            DateField::Time => NSDatePickerElementFlags::NSDatePickerElementFlagHourMinute,
        });
        picker.setPresentsCalendarOverlay(true);
        picker.setCalendar(Some(&calendar));
        picker.setTimeZone(Some(&calendar.timeZone()));
        picker.setDateValue(&NSDate::dateWithTimeIntervalSince1970(
            (timestamp / 10_000_000) as f64 - 11_644_473_600.0,
        ));
        picker.setMinDate(Some(&NSDate::dateWithTimeIntervalSince1970(
            -11_644_473_600.0,
        )));
        let label = NSString::from_str(canvas::interaction::DATE_LABELS[field as usize]);
        let _: () = msg_send![&picker, setAccessibilityLabel: &*label];
        picker.sizeToFit();
        let alert = NSAlert::new(mtm);
        alert.setMessageText(&NSString::from_str(title));
        alert.setAccessoryView(Some(&picker));
        alert.addButtonWithTitle(&NSString::from_str("Apply"));
        alert.addButtonWithTitle(&NSString::from_str("Cancel"));
        alert.layout();
        alert.window().makeFirstResponder(Some(&picker));
        if alert.runModal() != NSAlertFirstButtonReturn {
            return Ok(None);
        }
        alert.window().makeFirstResponder(None);
        merge_date(timestamp, &picker.dateValue(), field, &calendar).map(Some)
    }
}

fn merge_date(
    timestamp: u64,
    selected: &NSDate,
    field: DateField,
    calendar: &NSCalendar,
) -> Result<(u64, [String; 2]), &'static str> {
    unsafe {
        let original = NSDate::dateWithTimeIntervalSince1970(
            (timestamp / 10_000_000) as f64 - 11_644_473_600.0,
        );
        let units = NSCalendarUnit::Era
            | NSCalendarUnit::Year
            | NSCalendarUnit::Month
            | NSCalendarUnit::Day
            | NSCalendarUnit::Hour
            | NSCalendarUnit::Minute
            | NSCalendarUnit::Second;
        let components = calendar.components_fromDate(units, &original);
        let changed = calendar.components_fromDate(units, selected);
        match field {
            DateField::Date => {
                components.setEra(changed.era());
                components.setYear(changed.year());
                components.setMonth(changed.month());
                components.setDay(changed.day());
            }
            DateField::Time => {
                components.setHour(changed.hour());
                components.setMinute(changed.minute());
            }
        }
        let date = calendar
            .dateFromComponents(&components)
            .ok_or(crate::DATE_UNCHOSEN)?;
        let seconds = date.timeIntervalSince1970() + 11_644_473_600.0;
        let outside_range = crate::DATE_OUT_OF_RANGE;
        if !seconds.is_finite() || seconds < 0.0 || seconds > (u64::MAX / 10_000_000) as f64 {
            return Err(outside_range);
        }
        // NSDate cannot retain FILETIME's subsecond precision at this epoch distance.
        let updated = (seconds.round() as u64)
            .checked_mul(10_000_000)
            .and_then(|value| value.checked_add(timestamp % 10_000_000))
            .ok_or(outside_range)?;
        Ok((updated, labels(&date, calendar)))
    }
}

/// `date` as a page's title shows it: the long date and the short time.
fn labels(date: &NSDate, calendar: &NSCalendar) -> [String; 2] {
    unsafe {
        let formatter = NSDateFormatter::new();
        formatter.setCalendar(Some(calendar));
        formatter.setTimeZone(Some(&calendar.timeZone()));
        formatter.setDateStyle(NSDateFormatterStyle::NSDateFormatterFullStyle);
        let date_text = formatter.stringFromDate(date).to_string();
        formatter.setDateStyle(NSDateFormatterStyle::NSDateFormatterNoStyle);
        formatter.setTimeStyle(NSDateFormatterStyle::NSDateFormatterShortStyle);
        [date_text, formatter.stringFromDate(date).to_string()]
    }
}

/// FILETIME as a new page's title shows it: the long date and the short time.
pub fn date_text(filetime: u64) -> [String; 2] {
    unsafe {
        let date = NSDate::dateWithTimeIntervalSince1970(
            (filetime / 10_000_000) as f64 - 11_644_473_600.0,
        );
        labels(&date, &NSCalendar::currentCalendar())
    }
}

/// The account's full name, which OneNote's author fields take from Office's user name.
pub fn user_name() -> String {
    unsafe { objc2_foundation::NSFullUserName().as_ref() }.to_string()
}

/// FILETIME as the system's short date, as OneNote labels a conflict page.
pub fn short_date(filetime: u64) -> String {
    unsafe {
        let date = NSDate::dateWithTimeIntervalSince1970(
            (filetime / 10_000_000) as f64 - 11_644_473_600.0,
        );
        let formatter = NSDateFormatter::new();
        formatter.setDateStyle(NSDateFormatterStyle::NSDateFormatterShortStyle);
        formatter.setTimeStyle(NSDateFormatterStyle::NSDateFormatterNoStyle);
        formatter.stringFromDate(&date).to_string()
    }
}

#[cfg(test)]
mod date_tests {
    use super::*;
    use objc2_foundation::{NSCalendarIdentifierGregorian, NSTimeZone};

    #[test]
    fn calendar_changes_preserve_hidden_components_and_filetime_fraction() {
        unsafe {
            let calendar = NSCalendar::initWithCalendarIdentifier(
                NSCalendar::alloc(),
                NSCalendarIdentifierGregorian,
            )
            .unwrap();
            calendar.setTimeZone(
                &NSTimeZone::timeZoneWithName(&NSString::from_str("America/Los_Angeles")).unwrap(),
            );
            for (original, selected, field, expected) in [
                (
                    1_788_786_864_u64,
                    1_789_023_599.0,
                    DateField::Date,
                    1_788_959_664_u64,
                ),
                (1_788_786_864, 978_413_702.0, DateField::Time, 1_788_842_124),
                (
                    1_709_259_645,
                    1_740_772_800.0,
                    DateField::Date,
                    1_740_795_645,
                ),
                (
                    1_772_879_424,
                    1_772_996_400.0,
                    DateField::Date,
                    1_772_965_824,
                ),
            ] {
                let ticks = (original + 11_644_473_600) * 10_000_000 + 9_123_456;
                let (updated, text) = merge_date(
                    ticks,
                    &NSDate::dateWithTimeIntervalSince1970(selected),
                    field,
                    &calendar,
                )
                .unwrap();
                assert_eq!(
                    updated,
                    (expected + 11_644_473_600) * 10_000_000 + 9_123_456
                );
                assert!(text.iter().all(|value| !value.is_empty()));
            }
        }
    }
}

pub fn configure_presentation(surface: &wgpu::Surface<'_>) {
    MainThreadMarker::new().expect("Layer presentation belongs to the main thread");
    // Retain wgpu's surface ownership while synchronizing presentation with AppKit resize transactions.
    if let Some(surface) = unsafe { surface.as_hal::<wgpu::hal::api::Metal>() } {
        surface
            .render_layer()
            .lock()
            .setPresentsWithTransaction(true);
    }
}

/// Commits a frame presented with the transaction: winit redraws after Core Animation's
/// commit observer, so otherwise the frame waits for the next event. A live resize leaves
/// the commit to AppKit, which pairs the frame with the window's new size.
pub fn commit_presentation(window: &Window) {
    let resizing: bool = unsafe { msg_send![&ns_window(window), inLiveResize] };
    if !resizing {
        let class = AnyClass::get("CATransaction").expect("QuartzCore is linked");
        let _: () = unsafe { msg_send![class, flush] };
    }
}

/// The application's icon as the Dock shows it, `pixels` square.
pub fn app_icon(pixels: u32) -> Option<draw::RasterImage> {
    let side = pixels as usize;
    let mut rgba = unsafe {
        let mtm = MainThreadMarker::new()?;
        let icon: Retained<AnyObject> =
            msg_send_id![&NSApplication::sharedApplication(mtm), applicationIconImage];
        let bitmap: Allocated<AnyObject> = msg_send_id![AnyClass::get("NSBitmapImageRep")?, alloc];
        let planes: *mut *mut u8 = std::ptr::null_mut();
        let bitmap: Option<Retained<AnyObject>> = msg_send_id![
            bitmap,
            initWithBitmapDataPlanes: planes,
            pixelsWide: side as isize,
            pixelsHigh: side as isize,
            bitsPerSample: 8isize,
            samplesPerPixel: 4isize,
            hasAlpha: true,
            isPlanar: false,
            colorSpaceName: &*NSString::from_str("NSCalibratedRGBColorSpace"),
            bytesPerRow: 4 * side as isize,
            bitsPerPixel: 32isize
        ];
        let bitmap = bitmap?;
        let contexts = AnyClass::get("NSGraphicsContext")?;
        let context: Option<Retained<AnyObject>> =
            msg_send_id![contexts, graphicsContextWithBitmapImageRep: &*bitmap];
        let _: () = msg_send![contexts, saveGraphicsState];
        let _: () = msg_send![contexts, setCurrentContext: &*context?];
        let bounds = NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(side as f64, side as f64),
        );
        // NSCompositingOperationSourceOver
        let _: () = msg_send![&icon, drawInRect: bounds, fromRect: NSRect::ZERO, operation: 2usize, fraction: 1.0f64];
        let _: () = msg_send![contexts, restoreGraphicsState];
        let data: *const u8 = msg_send![&bitmap, bitmapData];
        std::slice::from_raw_parts(data, 4 * side * side).to_vec()
    };
    // AppKit's bitmap is premultiplied.
    for pixel in rgba.chunks_exact_mut(4) {
        let alpha = u32::from(pixel[3]);
        for channel in &mut pixel[..3] {
            *channel = (u32::from(*channel) * 255)
                .checked_div(alpha)
                .map_or(0, |value| value.min(255) as u8);
        }
    }
    draw::RasterImage::new([pixels; 2], rgba).ok()
}

/// The insertion point's colour and selected text's fill with and without keyboard focus,
/// linear RGBA, in `window`'s appearance.
pub fn text_colors(window: &Window) -> [[f32; 4]; 3] {
    let class = AnyClass::get("NSAppearance").expect("AppKit is linked");
    unsafe {
        let appearance: Retained<AnyObject> = msg_send_id![&ns_window(window), effectiveAppearance];
        // System colours resolve in the thread's appearance, which outside drawing does
        // not follow the window's.
        let previous: Option<Retained<AnyObject>> = msg_send_id![class, currentAppearance];
        let _: () = msg_send![class, setCurrentAppearance: &*appearance];
        let space = NSColorSpace::sRGBColorSpace();
        let colors = [
            NSColor::textInsertionPointColor(),
            NSColor::selectedTextBackgroundColor(),
            NSColor::unemphasizedSelectedTextBackgroundColor(),
        ]
        .map(|color| {
            let color = color
                .colorUsingColorSpace(&space)
                .expect("System text colors convert to sRGB");
            let [r, g, b] = [
                color.redComponent(),
                color.greenComponent(),
                color.blueComponent(),
            ]
            .map(|value| {
                let value = value as f32;
                if value <= 0.04045 {
                    value / 12.92
                } else {
                    ((value + 0.055) / 1.055).powf(2.4)
                }
            });
            [r, g, b, color.alphaComponent() as f32]
        });
        let _: () = msg_send![class, setCurrentAppearance: previous.as_deref()];
        colors
    }
}

declare_class!(
    struct CanvasApplication;

    unsafe impl ClassType for CanvasApplication {
        type Super = NSApplication;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "SnowboundApplication";
    }

    impl DeclaredClass for CanvasApplication {
        type Ivars = ();
    }

    unsafe impl CanvasApplication {
        #[method_id(init)]
        fn init(this: Allocated<Self>) -> Retained<Self> {
            let this = this.set_ivars(());
            unsafe { msg_send_id![super(this), init] }
        }

        #[method(terminate:)]
        fn terminate(&self, _sender: Option<&AnyObject>) {
            if let Some(proxy) = QUIT.get() { let _ = proxy.send_event(crate::UserEvent::Quit); }
        }

        // NSAlert consumes Escape before it reaches cancelOperation:.
        #[method(sendEvent:)]
        fn send_event(&self, event: &NSEvent) {
            unsafe {
                if event.r#type() == NSEventType::KeyDown
                    && self.modalWindow().is_some()
                    && event.charactersIgnoringModifiers().is_some_and(|text| text.to_string() == "\u{1b}")
                {
                    self.stopModal();
                } else {
                    let _: () = msg_send![super(self), sendEvent: event];
                }
            }
        }
    }
);

/// A `headless` application takes no Dock icon and never activates.
pub fn event_loop(headless: bool) -> Result<EventLoop<crate::UserEvent>, EventLoopError> {
    use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
    MainThreadMarker::new().expect("AppKit must start on the main thread");
    // Create the application's own subclass before Winit obtains the singleton.
    let _: Retained<CanvasApplication> =
        unsafe { msg_send_id![CanvasApplication::class(), sharedApplication] };
    let mut builder = EventLoop::with_user_event();
    if headless {
        builder
            .with_activation_policy(ActivationPolicy::Prohibited)
            .with_activate_ignoring_other_apps(false);
    }
    let event_loop = builder.build()?;
    QUIT.set(event_loop.create_proxy())
        .expect("Only one application event loop is created");
    Ok(event_loop)
}

/// Asks whether to go ahead with `action`, offering `cancel` first.
pub fn confirm(message: &str, detail: &str, cancel: &str, action: &str) -> bool {
    let mtm = MainThreadMarker::new().expect("Window events run on the main thread");
    // The alert and its strings stay on AppKit's main thread for the modal call.
    unsafe {
        let alert = NSAlert::new(mtm);
        alert.setMessageText(&NSString::from_str(message));
        alert.setInformativeText(&NSString::from_str(detail));
        alert.addButtonWithTitle(&NSString::from_str(cancel));
        alert.addButtonWithTitle(&NSString::from_str(action));
        alert.runModal() == NSAlertSecondButtonReturn
    }
}

/// End AppKit preedit after a canvas action commits or cancels the composition.
pub fn clear_marked_text(window: &Window) {
    MainThreadMarker::new().expect("Text input belongs to the main thread");
    let RawWindowHandle::AppKit(handle) =
        window.window_handle().expect("Live AppKit window").as_raw()
    else {
        unreachable!()
    };
    // Winit owns this view and implements the NSTextInputClient selectors.
    unsafe {
        let view = &*handle.ns_view.as_ptr().cast::<AnyObject>();
        let marked: bool = msg_send![view, hasMarkedText];
        if marked {
            let _: () = msg_send![view, unmarkText];
        }
    }
}

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn TISCopyCurrentKeyboardInputSource() -> *mut AnyObject;
    fn TISGetInputSourceProperty<'a>(
        source: &'a AnyObject,
        key: &NSString,
    ) -> Option<&'a AnyObject>;
    static kTISPropertyInputSourceLanguages: &'static NSString;
}

/// The current keyboard input source's primary language as a BCP-47 tag, empty if it has none.
pub fn input_language() -> String {
    MainThreadMarker::new().expect("Text Input Sources belong to the main thread");
    // Input sources are CFTypes, released as Objective-C objects; languages is an NSArray.
    unsafe {
        let Some(source) = Retained::from_raw(TISCopyCurrentKeyboardInputSource()) else {
            return String::new();
        };
        let language: Option<Retained<NSString>> =
            TISGetInputSourceProperty(&source, kTISPropertyInputSourceLanguages)
                .and_then(|languages| msg_send_id![languages, firstObject]);
        language.map_or_else(String::new, |language| language.to_string())
    }
}
