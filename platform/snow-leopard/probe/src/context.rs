//! An offscreen OpenGL context, current on this thread.
use objc2::{MainThreadMarker, rc::Retained};
use objc2_app_kit::{NSOpenGLContext, NSOpenGLPFAAccelerated, NSOpenGLPFAColorSize, NSOpenGLPixelFormat};

pub fn current() -> Retained<NSOpenGLContext> {
    let mtm = MainThreadMarker::new().unwrap();
    let attributes = [NSOpenGLPFAAccelerated, NSOpenGLPFAColorSize, 24, 0];
    let format = unsafe {
        NSOpenGLPixelFormat::initWithAttributes(mtm.alloc(), std::ptr::NonNull::new(attributes.as_ptr().cast_mut()).unwrap())
    }
    .expect("pixel format");
    let context = NSOpenGLContext::initWithFormat_shareContext(mtm.alloc(), &format, None).expect("context");
    context.makeCurrentContext();
    context
}
