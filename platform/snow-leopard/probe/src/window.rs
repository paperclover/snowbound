//! Opens a window with an OpenGL 2.1 view, paints a test pattern through an sRGB
//! framebuffer object, presents it, and writes the framebuffer to `window-probe.png`.
#![allow(deprecated)] // OpenGL is the only GPU API 10.6 has.
use gl21::*;
use objc2::{MainThreadMarker, rc::Retained};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSOpenGLPFAAccelerated,
    NSOpenGLPFAAlphaSize, NSOpenGLPFAColorSize, NSOpenGLPFADoubleBuffer, NSOpenGLPixelFormat,
    NSOpenGLView, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{NSDate, NSDefaultRunLoopMode, NSPoint, NSRect, NSSize, NSString};

const SIZE: [i32; 2] = [480, 320];

fn main() {
    // 10.6 has no implicit pool on the main thread; AppKit autoreleases from the start.
    objc2::rc::autoreleasepool(|_| run());
}

fn run() {
    let mtm = MainThreadMarker::new().unwrap();
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    app.finishLaunching();

    let frame = NSRect::new(NSPoint::new(200.0, 200.0), NSSize::new(SIZE[0] as f64, SIZE[1] as f64));
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            mtm.alloc(),
            frame,
            NSWindowStyleMask::Titled | NSWindowStyleMask::Closable | NSWindowStyleMask::Resizable,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    unsafe { window.setReleasedWhenClosed(false) };
    window.setTitle(&NSString::from_str("Snowbound on Snow Leopard"));
    let attributes = [
        NSOpenGLPFADoubleBuffer,
        NSOpenGLPFAAccelerated,
        NSOpenGLPFAColorSize,
        24,
        NSOpenGLPFAAlphaSize,
        8,
        0,
    ];
    let format: Retained<NSOpenGLPixelFormat> = unsafe {
        NSOpenGLPixelFormat::initWithAttributes(
            mtm.alloc(),
            std::ptr::NonNull::new(attributes.as_ptr().cast_mut()).unwrap(),
        )
    }
    .expect("pixel format");
    let view = NSOpenGLView::initWithFrame_pixelFormat(mtm.alloc(), frame, Some(&format)).unwrap();
    window.setContentView(Some(&view));
    window.makeKeyAndOrderFront(None);
    app.activateIgnoringOtherApps(true);

    // Let the window server map the window before painting into it.
    let until = NSDate::dateWithTimeIntervalSinceNow(0.5);
    while let Some(event) = unsafe {
        app.nextEventMatchingMask_untilDate_inMode_dequeue(
            objc2_app_kit::NSEventMask::Any,
            Some(&until),
            NSDefaultRunLoopMode,
            true,
        )
    } {
        app.sendEvent(&event);
    }

    let context = view.openGLContext().expect("context");
    context.makeCurrentContext();
    println!("GL_VENDOR    {}", string(VENDOR));
    println!("GL_RENDERER  {}", string(RENDERER));
    println!("GL_VERSION   {}", string(VERSION));
    let extensions = string(EXTENSIONS);
    for wanted in ["GL_EXT_framebuffer_object", "GL_EXT_framebuffer_blit", "GL_EXT_framebuffer_sRGB", "GL_EXT_texture_sRGB", "GL_ARB_texture_non_power_of_two"] {
        println!("{wanted:32} {}", extensions.split(' ').any(|e| e == wanted));
    }

    let pixels = unsafe { paint() };
    unsafe {
        glBindFramebufferEXT(DRAW_FRAMEBUFFER, 0);
        glBlitFramebufferEXT(0, 0, SIZE[0], SIZE[1], 0, 0, SIZE[0], SIZE[1], COLOR_BUFFER_BIT, NEAREST as GLenum);
    }
    context.flushBuffer();
    println!("GL error after present: {:#x}", unsafe { glGetError() });

    let file = std::fs::File::create("window-probe.png").unwrap();
    let mut encoder = png::Encoder::new(file, SIZE[0] as u32, SIZE[1] as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.write_header().unwrap().write_image_data(&pixels).unwrap();
    println!("wrote window-probe.png");
}

/// Clears an sRGB framebuffer object and fills three bands with scissored clears in
/// linear colours; returns its pixels top row first.
unsafe fn paint() -> Vec<u8> {
    unsafe {
        let mut texture = 0;
        glGenTextures(1, &mut texture);
        glBindTexture(TEXTURE_2D, texture);
        glTexParameteri(TEXTURE_2D, TEXTURE_MIN_FILTER, NEAREST);
        glTexParameteri(TEXTURE_2D, TEXTURE_MAG_FILTER, NEAREST);
        glTexImage2D(TEXTURE_2D, 0, SRGB8_ALPHA8, SIZE[0], SIZE[1], 0, RGBA, UNSIGNED_BYTE, std::ptr::null());
        let mut framebuffer = 0;
        glGenFramebuffersEXT(1, &mut framebuffer);
        glBindFramebufferEXT(FRAMEBUFFER, framebuffer);
        glFramebufferTexture2DEXT(FRAMEBUFFER, COLOR_ATTACHMENT0, TEXTURE_2D, texture, 0);
        assert_eq!(glCheckFramebufferStatusEXT(FRAMEBUFFER), FRAMEBUFFER_COMPLETE);
        glEnable(FRAMEBUFFER_SRGB);
        glViewport(0, 0, SIZE[0], SIZE[1]);
        glClearColor(0.9, 0.9, 0.9, 1.0);
        glClear(COLOR_BUFFER_BIT);
        glEnable(SCISSOR_TEST);
        // Linear 0.214 is sRGB 128: the middle band reads back as 128 if encoding works.
        for (i, color) in [[0.8, 0.1, 0.1], [0.214, 0.214, 0.214], [0.1, 0.2, 0.8]].iter().enumerate() {
            glScissor(40 + i as i32 * 140, 60, 120, 200);
            glClearColor(color[0], color[1], color[2], 1.0);
            glClear(COLOR_BUFFER_BIT);
        }
        glDisable(SCISSOR_TEST);
        let mut pixels = vec![0u8; (SIZE[0] * SIZE[1] * 4) as usize];
        glReadPixels(0, 0, SIZE[0], SIZE[1], RGBA, UNSIGNED_BYTE, pixels.as_mut_ptr().cast());
        glBindFramebufferEXT(READ_FRAMEBUFFER, framebuffer);
        let row = (SIZE[0] * 4) as usize;
        let flipped: Vec<u8> = pixels.chunks(row).rev().flatten().copied().collect();
        let middle = &flipped[(160 * SIZE[0] as usize + 250) * 4..][..4];
        println!("middle band reads {middle:?} (128 means sRGB encode on write)");
        flipped
    }
}
