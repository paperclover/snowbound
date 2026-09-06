use objc2::{
    ClassType, DeclaredClass, declare_class, msg_send, msg_send_id, mutability,
    rc::{Allocated, Retained},
    runtime::AnyObject,
};
use objc2_app_kit::{NSAlert, NSAlertSecondButtonReturn, NSApplication, NSEvent, NSEventType};
use objc2_foundation::{MainThreadMarker, NSString};
use std::sync::OnceLock;
use winit::{
    error::EventLoopError,
    event_loop::{EventLoop, EventLoopProxy},
    raw_window_handle::{HasWindowHandle, RawWindowHandle},
    window::Window,
};

static QUIT: OnceLock<EventLoopProxy<crate::UserEvent>> = OnceLock::new();

declare_class!(
    struct CanvasApplication;

    unsafe impl ClassType for CanvasApplication {
        type Super = NSApplication;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "OneCanvasApplication";
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

pub fn event_loop() -> Result<EventLoop<crate::UserEvent>, EventLoopError> {
    MainThreadMarker::new().expect("AppKit must start on the main thread");
    // Create the application's own subclass before Winit obtains the singleton.
    let _: Retained<CanvasApplication> =
        unsafe { msg_send_id![CanvasApplication::class(), sharedApplication] };
    let event_loop = EventLoop::with_user_event().build()?;
    QUIT.set(event_loop.create_proxy())
        .expect("Only one application event loop is created");
    Ok(event_loop)
}

pub fn discard_changes() -> bool {
    let mtm = MainThreadMarker::new().expect("Window events run on the main thread");
    // The alert and its strings stay on AppKit's main thread for the modal call.
    unsafe {
        let alert = NSAlert::new(mtm);
        alert.setMessageText(&NSString::from_str("Discard this page?"));
        alert.setInformativeText(&NSString::from_str(
            "This temporary page has no saved copy. Closing it will discard your edits.",
        ));
        alert.addButtonWithTitle(&NSString::from_str("Keep Editing"));
        alert.addButtonWithTitle(&NSString::from_str("Discard Changes"));
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
