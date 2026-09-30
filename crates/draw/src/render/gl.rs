//! Submission through OpenGL 2.1 with EXT_framebuffer_object, EXT_framebuffer_blit and
//! the sRGB extensions, which every Mac OS X 10.6 driver has and OpenGL.framework
//! exports; on Windows, where opengl32.dll exports only OpenGL 1.1, the driver supplies
//! them through `wglGetProcAddress`. Frames draw into an sRGB framebuffer object, so
//! blending happens in linear light as it does in wgpu's sRGB targets.
#![allow(non_snake_case)]
use super::*;
use std::{
    ffi::{CStr, CString, c_char, c_void},
    mem::offset_of,
    ptr,
};

type GLenum = u32;
type GLuint = u32;
type GLint = i32;
type GLsizei = i32;

const TRIANGLES: GLenum = 0x0004;
const COLOR_BUFFER_BIT: GLenum = 0x4000;
const ZERO: GLenum = 0;
const ONE: GLenum = 1;
const SRC_ALPHA: GLenum = 0x0302;
const ONE_MINUS_SRC_ALPHA: GLenum = 0x0303;
const DST_COLOR: GLenum = 0x0306;
const BLEND: GLenum = 0x0BE2;
const SCISSOR_TEST: GLenum = 0x0C11;
const MAX_TEXTURE_SIZE: GLenum = 0x0D33;
const TEXTURE_2D: GLenum = 0x0DE1;
const UNSIGNED_BYTE: GLenum = 0x1401;
const FLOAT: GLenum = 0x1406;
const RGBA: GLenum = 0x1908;
const NEAREST: GLint = 0x2600;
const LINEAR: GLint = 0x2601;
const TEXTURE_MAG_FILTER: GLenum = 0x2800;
const TEXTURE_MIN_FILTER: GLenum = 0x2801;
const TEXTURE_WRAP_S: GLenum = 0x2802;
const TEXTURE_WRAP_T: GLenum = 0x2803;
const CLAMP_TO_EDGE: GLint = 0x812F;
const TEXTURE0: GLenum = 0x84C0;
const ARRAY_BUFFER: GLenum = 0x8892;
const STREAM_DRAW: GLenum = 0x88E0;
const STATIC_DRAW: GLenum = 0x88E4;
const FRAGMENT_SHADER: GLenum = 0x8B30;
const VERTEX_SHADER: GLenum = 0x8B31;
const COMPILE_STATUS: GLenum = 0x8B81;
const LINK_STATUS: GLenum = 0x8B82;
const INFO_LOG_LENGTH: GLenum = 0x8B84;
const SRGB8_ALPHA8: GLint = 0x8C43;
const READ_FRAMEBUFFER: GLenum = 0x8CA8;
const DRAW_FRAMEBUFFER: GLenum = 0x8CA9;
const FRAMEBUFFER_COMPLETE: GLenum = 0x8CD5;
const COLOR_ATTACHMENT0: GLenum = 0x8CE0;
const FRAMEBUFFER: GLenum = 0x8D40;
const FRAMEBUFFER_SRGB: GLenum = 0x8DB9;

/// The entry points: linked where the system library exports them all, and on Windows
/// loaded with the first context and called through `FUNCTIONS`.
macro_rules! functions {
    ($(fn $name:ident($($arg:ident: $ty:ty),* $(,)?) $(-> $ret:ty)?;)*) => {
        #[cfg(not(windows))]
        #[cfg_attr(target_os = "macos", link(name = "OpenGL", kind = "framework"))]
        unsafe extern "C" {
            $(fn $name($($arg: $ty),*) $(-> $ret)?;)*
        }

        #[cfg(windows)]
        struct Functions {
            $($name: unsafe extern "system" fn($($ty),*) $(-> $ret)?,)*
        }

        #[cfg(windows)]
        static FUNCTIONS: std::sync::OnceLock<Functions> = std::sync::OnceLock::new();

        /// Loads the entry points from the context current on this thread.
        #[cfg(windows)]
        fn load() -> Result<(), String> {
            if FUNCTIONS.get().is_none() {
                let functions = Functions {
                    $($name: unsafe {
                        std::mem::transmute::<
                            *const c_void,
                            unsafe extern "system" fn($($ty),*) $(-> $ret)?,
                        >(windows::address(concat!(stringify!($name), "\0"))?)
                    },)*
                };
                let _ = FUNCTIONS.set(functions);
            }
            Ok(())
        }

        $(
            #[cfg(windows)]
            #[allow(clippy::too_many_arguments)]
            unsafe fn $name($($arg: $ty),*) $(-> $ret)? {
                unsafe { (FUNCTIONS.get().expect("OpenGL is loaded").$name)($($arg),*) }
            }
        )*
    };
}

#[cfg(not(windows))]
fn load() -> Result<(), String> {
    Ok(())
}

#[cfg(windows)]
mod windows {
    use std::ffi::{c_char, c_void};

    #[link(name = "opengl32")]
    unsafe extern "system" {
        fn wglGetProcAddress(name: *const c_char) -> *const c_void;
    }

    unsafe extern "system" {
        fn GetModuleHandleA(name: *const c_char) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const c_char) -> *const c_void;
    }

    /// OpenGL function `name`, NUL-terminated: from the driver, or opengl32.dll's own for
    /// OpenGL 1.1. An `EXT` name the driver lacks falls back to its core name.
    pub(super) fn address(name: &str) -> Result<*const c_void, String> {
        let find = |name: &str| unsafe {
            let driver = wglGetProcAddress(name.as_ptr().cast());
            // Some drivers answer a missing name with a small integer rather than null.
            if !matches!(driver as isize, -1..=3) {
                return Some(driver);
            }
            let system = GetProcAddress(
                GetModuleHandleA(c"opengl32.dll".as_ptr()),
                name.as_ptr().cast(),
            );
            (!system.is_null()).then_some(system)
        };
        find(name)
            .or_else(|| {
                let core = name.strip_suffix("EXT\0")?;
                find(&format!("{core}\0"))
            })
            .ok_or_else(|| format!("OpenGL has no {}", name.trim_end_matches('\0')))
    }
}

functions! {
    fn glEnable(cap: GLenum);
    fn glDisable(cap: GLenum);
    fn glGetIntegerv(name: GLenum, value: *mut GLint);
    fn glViewport(x: GLint, y: GLint, width: GLsizei, height: GLsizei);
    fn glScissor(x: GLint, y: GLint, width: GLsizei, height: GLsizei);
    fn glClearColor(r: f32, g: f32, b: f32, a: f32);
    fn glClear(mask: GLenum);
    fn glBlendFuncSeparate(src_rgb: GLenum, dst_rgb: GLenum, src_a: GLenum, dst_a: GLenum);
    fn glReadPixels(
        x: GLint,
        y: GLint,
        w: GLsizei,
        h: GLsizei,
        format: GLenum,
        ty: GLenum,
        data: *mut c_void,
    );

    fn glGenTextures(n: GLsizei, textures: *mut GLuint);
    fn glDeleteTextures(n: GLsizei, textures: *const GLuint);
    fn glBindTexture(target: GLenum, texture: GLuint);
    fn glActiveTexture(unit: GLenum);
    fn glTexParameteri(target: GLenum, name: GLenum, value: GLint);
    fn glTexImage2D(
        target: GLenum,
        level: GLint,
        internal: GLint,
        w: GLsizei,
        h: GLsizei,
        border: GLint,
        format: GLenum,
        ty: GLenum,
        data: *const c_void,
    );
    fn glTexSubImage2D(
        target: GLenum,
        level: GLint,
        x: GLint,
        y: GLint,
        w: GLsizei,
        h: GLsizei,
        format: GLenum,
        ty: GLenum,
        data: *const c_void,
    );

    fn glGenBuffers(n: GLsizei, buffers: *mut GLuint);
    fn glBindBuffer(target: GLenum, buffer: GLuint);
    fn glBufferData(target: GLenum, size: isize, data: *const c_void, usage: GLenum);
    fn glEnableVertexAttribArray(index: GLuint);
    fn glDisableVertexAttribArray(index: GLuint);
    fn glVertexAttribPointer(
        index: GLuint,
        size: GLint,
        ty: GLenum,
        normalized: u8,
        stride: GLsizei,
        offset: *const c_void,
    );
    fn glDrawArrays(mode: GLenum, first: GLint, count: GLsizei);

    fn glCreateShader(kind: GLenum) -> GLuint;
    fn glShaderSource(
        shader: GLuint,
        count: GLsizei,
        sources: *const *const c_char,
        lengths: *const GLint,
    );
    fn glCompileShader(shader: GLuint);
    fn glGetShaderiv(shader: GLuint, name: GLenum, value: *mut GLint);
    fn glGetShaderInfoLog(shader: GLuint, size: GLsizei, length: *mut GLsizei, log: *mut c_char);
    fn glCreateProgram() -> GLuint;
    fn glAttachShader(program: GLuint, shader: GLuint);
    fn glBindAttribLocation(program: GLuint, index: GLuint, name: *const c_char);
    fn glLinkProgram(program: GLuint);
    fn glGetProgramiv(program: GLuint, name: GLenum, value: *mut GLint);
    fn glGetProgramInfoLog(program: GLuint, size: GLsizei, length: *mut GLsizei, log: *mut c_char);
    fn glUseProgram(program: GLuint);
    fn glGetUniformLocation(program: GLuint, name: *const c_char) -> GLint;
    fn glUniform1i(location: GLint, value: GLint);
    fn glUniform2f(location: GLint, x: f32, y: f32);

    fn glGenFramebuffersEXT(n: GLsizei, framebuffers: *mut GLuint);
    fn glDeleteFramebuffersEXT(n: GLsizei, framebuffers: *const GLuint);
    fn glBindFramebufferEXT(target: GLenum, framebuffer: GLuint);
    fn glFramebufferTexture2DEXT(
        target: GLenum,
        attachment: GLenum,
        textarget: GLenum,
        texture: GLuint,
        level: GLint,
    );
    fn glCheckFramebufferStatusEXT(target: GLenum) -> GLenum;
    fn glBlitFramebufferEXT(
        sx0: GLint,
        sy0: GLint,
        sx1: GLint,
        sy1: GLint,
        dx0: GLint,
        dy0: GLint,
        dx1: GLint,
        dy1: GLint,
        mask: GLenum,
        filter: GLenum,
    );
}

/// Blending a premultiplied picture over what lies beneath.
const PREMULTIPLIED: [GLenum; 4] = [ONE, ONE_MINUS_SRC_ALPHA, ONE, ONE_MINUS_SRC_ALPHA];

/// An sRGB RGBA texture, deleted with its value.
pub(super) struct Image {
    name: GLuint,
    size: [u32; 2],
}

impl Image {
    fn new(size: [u32; 2], filter: GLint, pixels: Option<&[u8]>) -> Self {
        let mut name = 0;
        unsafe {
            glGenTextures(1, &mut name);
            glBindTexture(TEXTURE_2D, name);
            glTexParameteri(TEXTURE_2D, TEXTURE_MIN_FILTER, filter);
            glTexParameteri(TEXTURE_2D, TEXTURE_MAG_FILTER, filter);
            glTexParameteri(TEXTURE_2D, TEXTURE_WRAP_S, CLAMP_TO_EDGE);
            glTexParameteri(TEXTURE_2D, TEXTURE_WRAP_T, CLAMP_TO_EDGE);
            glTexImage2D(
                TEXTURE_2D,
                0,
                SRGB8_ALPHA8,
                size[0] as GLsizei,
                size[1] as GLsizei,
                0,
                RGBA,
                UNSIGNED_BYTE,
                pixels.map_or(ptr::null(), |pixels| pixels.as_ptr().cast()),
            );
        }
        Self { name, size }
    }
}

impl Drop for Image {
    fn drop(&mut self) {
        unsafe { glDeleteTextures(1, &self.name) };
    }
}

impl AsRef<Image> for Image {
    fn as_ref(&self) -> &Image {
        self
    }
}

/// An offscreen frame a window shows by `present`, the context's default framebuffer
/// being no sRGB target.
pub struct Target {
    framebuffer: GLuint,
    texture: Image,
}

impl Target {
    /// Needs the renderer's context current, as every method here does.
    pub fn new(size: [u32; 2]) -> Result<Self, String> {
        let texture = Image::new(size, NEAREST, None);
        let mut framebuffer = 0;
        unsafe {
            glGenFramebuffersEXT(1, &mut framebuffer);
            glBindFramebufferEXT(FRAMEBUFFER, framebuffer);
            glFramebufferTexture2DEXT(FRAMEBUFFER, COLOR_ATTACHMENT0, TEXTURE_2D, texture.name, 0);
            let status = glCheckFramebufferStatusEXT(FRAMEBUFFER);
            glBindFramebufferEXT(FRAMEBUFFER, 0);
            if status != FRAMEBUFFER_COMPLETE {
                glDeleteFramebuffersEXT(1, &framebuffer);
                return Err(format!("Framebuffer incomplete: {status:#x}"));
            }
        }
        Ok(Self {
            framebuffer,
            texture,
        })
    }

    pub fn size(&self) -> [u32; 2] {
        self.texture.size
    }

    /// Copies the frame to the context's window, which is the same size.
    pub fn present(&self) {
        let [width, height] = self.size().map(|side| side as GLint);
        unsafe {
            glBindFramebufferEXT(READ_FRAMEBUFFER, self.framebuffer);
            glBindFramebufferEXT(DRAW_FRAMEBUFFER, 0);
            glBlitFramebufferEXT(
                0,
                0,
                width,
                height,
                0,
                0,
                width,
                height,
                COLOR_BUFFER_BIT,
                NEAREST as GLenum,
            );
            glBindFramebufferEXT(FRAMEBUFFER, 0);
        }
    }

    /// sRGB-encoded RGBA rows, top first.
    pub fn read_pixels(&self) -> Vec<u8> {
        let [width, height] = self.size();
        let mut pixels = vec![0; (width * height * 4) as usize];
        unsafe {
            glBindFramebufferEXT(FRAMEBUFFER, self.framebuffer);
            glReadPixels(
                0,
                0,
                width as GLsizei,
                height as GLsizei,
                RGBA,
                UNSIGNED_BYTE,
                pixels.as_mut_ptr().cast(),
            );
            glBindFramebufferEXT(FRAMEBUFFER, 0);
        }
        pixels
            .chunks(width as usize * 4)
            .rev()
            .flatten()
            .copied()
            .collect()
    }
}

impl Drop for Target {
    fn drop(&mut self) {
        unsafe { glDeleteFramebuffersEXT(1, &self.framebuffer) };
    }
}

pub(super) struct Gpu {
    program: GLuint,
    buffer: GLuint,
    /// `translucent.glsl`, and the triangle it covers a window with.
    translucent: GLuint,
    triangle: GLuint,
    atlas: Image,
    max_texture: u32,
    /// Offscreen pictures for groups, the target's size.
    groups: Vec<Target>,
}

/// Each attribute's name, component count and offset, bound to its index in the shader.
const ATTRIBUTES: [(&CStr, GLint, usize); 9] = [
    (c"position", 2, offset_of!(Vertex, position)),
    (c"uv", 2, offset_of!(Vertex, uv)),
    (c"color", 4, offset_of!(Vertex, color)),
    (c"local", 2, offset_of!(Vertex, local)),
    (c"shape", 4, offset_of!(Vertex, shape)),
    (c"stroke", 1, offset_of!(Vertex, stroke)),
    (c"clip_local", 2, offset_of!(Vertex, clip_local)),
    (c"clip", 3, offset_of!(Vertex, clip)),
    (c"blur", 1, offset_of!(Vertex, blur)),
];

#[cfg(not(feature = "wgpu"))]
impl Renderer {
    /// Draws with the OpenGL context current on this thread, which must stay current
    /// whenever the renderer or a `Target` is used.
    pub fn opengl() -> Result<Self, String> {
        Gpu::new().map(Self::with_gpu)
    }

    /// Copies `target` to the context's window, the same size, premultiplying each pixel
    /// again in sRGB, as a window server compositing a transparent surface needs.
    pub fn present_translucent(&self, target: &Target) {
        self.gpu.present_translucent(target);
    }
}

impl Gpu {
    pub(super) fn new() -> Result<Self, String> {
        load()?;
        let names: Vec<_> = ATTRIBUTES.iter().map(|(name, _, _)| *name).collect();
        let draw = unsafe { program(include_str!("../draw.glsl"), &names)? };
        let translucent = unsafe { program(include_str!("translucent.glsl"), &[c"position"])? };
        let [mut buffer, mut triangle] = [0; 2];
        let mut max_texture = 0;
        unsafe {
            glGenBuffers(1, &mut buffer);
            glGenBuffers(1, &mut triangle);
            glBindBuffer(ARRAY_BUFFER, triangle);
            let corners: [f32; 6] = [-1.0, -1.0, 3.0, -1.0, -1.0, 3.0];
            glBufferData(
                ARRAY_BUFFER,
                size_of_val(&corners) as isize,
                corners.as_ptr().cast(),
                STATIC_DRAW,
            );
            glGetIntegerv(MAX_TEXTURE_SIZE, &mut max_texture);
        }
        Ok(Gpu {
            program: draw,
            buffer,
            translucent,
            triangle,
            atlas: Image::new([ATLAS_SIZE; 2], NEAREST, None),
            max_texture: max_texture as u32,
            groups: Vec::new(),
        })
    }

    pub(super) fn present_translucent(&self, target: &Target) {
        let [width, height] = target.size().map(|side| side as f32);
        let program = self.translucent;
        unsafe {
            glBindFramebufferEXT(FRAMEBUFFER, 0);
            glDisable(FRAMEBUFFER_SRGB);
            glDisable(BLEND);
            glDisable(SCISSOR_TEST);
            glViewport(0, 0, width as GLsizei, height as GLsizei);
            glUseProgram(program);
            glUniform1i(glGetUniformLocation(program, c"frame".as_ptr()), 0);
            glUniform2f(
                glGetUniformLocation(program, c"size".as_ptr()),
                width,
                height,
            );
            glActiveTexture(TEXTURE0);
            glBindTexture(TEXTURE_2D, target.texture.name);
            glBindBuffer(ARRAY_BUFFER, self.triangle);
            for index in 1..ATTRIBUTES.len() {
                glDisableVertexAttribArray(index as GLuint);
            }
            glEnableVertexAttribArray(0);
            glVertexAttribPointer(0, 2, FLOAT, 0, 0, ptr::null());
            glDrawArrays(TRIANGLES, 0, 3);
        }
    }

    /// Offscreen pictures' rows run bottom first.
    pub(super) fn flipped(&self) -> bool {
        true
    }

    pub(super) fn max_texture_dimension(&self) -> u32 {
        self.max_texture
    }

    pub(super) fn atlas_side(&self) -> u32 {
        self.atlas.size[0]
    }

    pub(super) fn new_atlas(&mut self, side: u32) {
        self.atlas = Image::new([side; 2], NEAREST, None);
    }

    pub(super) fn write_atlas(&self, origin: [u32; 2], size: [u32; 2], rgba: &[u8]) {
        unsafe {
            glBindTexture(TEXTURE_2D, self.atlas.name);
            glTexSubImage2D(
                TEXTURE_2D,
                0,
                origin[0] as GLint,
                origin[1] as GLint,
                size[0] as GLsizei,
                size[1] as GLsizei,
                RGBA,
                UNSIGNED_BYTE,
                rgba.as_ptr().cast(),
            );
        }
    }

    pub(super) fn upload_image(&self, image: &RasterImage) -> Image {
        Image::new(image.size, LINEAR, Some(image.pixels()))
    }

    /// Clears `target`, `size` device pixels, to linear `clear` and draws the prepared
    /// batches, each group's offscreen first.
    pub(super) fn submit(
        &mut self,
        frame: &Frame<'_>,
        target: &Target,
        size: [u32; 2],
        clear: [f32; 4],
    ) {
        if self
            .groups
            .first()
            .is_some_and(|picture| picture.size() != size)
        {
            self.groups.clear();
        }
        while self.groups.len() < frame.groups.len() {
            match Target::new(size) {
                Ok(picture) => unsafe {
                    // Filtered, so a picture leaning back stays smooth.
                    glBindTexture(TEXTURE_2D, picture.texture.name);
                    glTexParameteri(TEXTURE_2D, TEXTURE_MIN_FILTER, LINEAR);
                    glTexParameteri(TEXTURE_2D, TEXTURE_MAG_FILTER, LINEAR);
                    self.groups.push(picture);
                },
                Err(error) => {
                    eprintln!("{error}");
                    return;
                }
            }
        }
        let [width, height] = size.map(|side| side as GLsizei);
        let gpu = &*self;
        let begin = |target: &Target, clear: [f32; 4]| unsafe {
            glBindFramebufferEXT(FRAMEBUFFER, target.framebuffer);
            glDisable(SCISSOR_TEST);
            glClearColor(clear[0], clear[1], clear[2], clear[3]);
            glClear(COLOR_BUFFER_BIT);
            glEnable(SCISSOR_TEST);
        };
        let draw = |batches: &[Batch]| unsafe {
            for batch in batches {
                let [x, y, w, h] = batch.scissor.map(|value| value as GLint);
                glScissor(x, height - y - h, w, h);
                let (texture, [src_rgb, dst_rgb, src_alpha, dst_alpha]) = match batch.blend {
                    Blend::Over => (
                        gpu.atlas.name,
                        [SRC_ALPHA, ONE_MINUS_SRC_ALPHA, ONE, ONE_MINUS_SRC_ALPHA],
                    ),
                    Blend::Image(id) => {
                        let image: &Image = frame.images[&id].texture.as_ref();
                        (image.name, PREMULTIPLIED)
                    }
                    Blend::Erase => (
                        gpu.atlas.name,
                        [ZERO, ONE_MINUS_SRC_ALPHA, ZERO, ONE_MINUS_SRC_ALPHA],
                    ),
                    Blend::Multiply => {
                        (gpu.atlas.name, [DST_COLOR, ONE_MINUS_SRC_ALPHA, ZERO, ONE])
                    }
                };
                glBlendFuncSeparate(src_rgb, dst_rgb, src_alpha, dst_alpha);
                glBindTexture(TEXTURE_2D, texture);
                glDrawArrays(
                    TRIANGLES,
                    batch.vertices.start as GLint,
                    batch.vertices.len() as GLsizei,
                );
            }
        };
        unsafe {
            glEnable(FRAMEBUFFER_SRGB);
            glViewport(0, 0, width, height);
            glUseProgram(gpu.program);
            glUniform1i(glGetUniformLocation(gpu.program, c"atlas".as_ptr()), 0);
            glActiveTexture(TEXTURE0);
            glBindBuffer(ARRAY_BUFFER, gpu.buffer);
            glBufferData(
                ARRAY_BUFFER,
                size_of_val(frame.vertices) as isize,
                frame.vertices.as_ptr().cast(),
                STREAM_DRAW,
            );
            for (index, (_, components, offset)) in ATTRIBUTES.iter().enumerate() {
                glEnableVertexAttribArray(index as GLuint);
                glVertexAttribPointer(
                    index as GLuint,
                    *components,
                    FLOAT,
                    0,
                    size_of::<Vertex>() as GLsizei,
                    ptr::without_provenance(*offset),
                );
            }
            glEnable(BLEND);
            for (group, picture) in frame.groups.iter().zip(&gpu.groups) {
                begin(picture, [0.0; 4]);
                draw(&frame.batches[group.batches.clone()]);
            }
            begin(target, clear);
            let mut next = 0;
            for (group, picture) in frame.groups.iter().zip(&gpu.groups) {
                draw(&frame.batches[next..group.batches.start]);
                glScissor(0, 0, width, height);
                let [src_rgb, dst_rgb, src_alpha, dst_alpha] = PREMULTIPLIED;
                glBlendFuncSeparate(src_rgb, dst_rgb, src_alpha, dst_alpha);
                glBindTexture(TEXTURE_2D, picture.texture.name);
                glDrawArrays(
                    TRIANGLES,
                    group.composite.start as GLint,
                    group.composite.len() as GLsizei,
                );
                next = group.batches.end;
            }
            draw(&frame.batches[next..]);
            glDisable(SCISSOR_TEST);
            glDisable(BLEND);
            glBindFramebufferEXT(FRAMEBUFFER, 0);
        }
    }
}

/// A program from `source`'s two stages, its attributes bound in order.
unsafe fn program(source: &str, attributes: &[&CStr]) -> Result<GLuint, String> {
    unsafe {
        let vertex = compile(
            VERTEX_SHADER,
            &format!("#version 120\n#define VERTEX\n{source}"),
        )?;
        let fragment = compile(FRAGMENT_SHADER, &format!("#version 120\n{source}"))?;
        let program = glCreateProgram();
        glAttachShader(program, vertex);
        glAttachShader(program, fragment);
        for (index, name) in attributes.iter().enumerate() {
            glBindAttribLocation(program, index as GLuint, name.as_ptr());
        }
        glLinkProgram(program);
        let mut linked = 0;
        glGetProgramiv(program, LINK_STATUS, &mut linked);
        if linked == 0 {
            return Err(format!("Linking a shader failed: {}", log(program, false)));
        }
        Ok(program)
    }
}

unsafe fn compile(kind: GLenum, source: &str) -> Result<GLuint, String> {
    unsafe {
        let shader = glCreateShader(kind);
        let text = CString::new(source).expect("The shader has no NUL");
        glShaderSource(shader, 1, &text.as_ptr(), ptr::null());
        glCompileShader(shader);
        let mut compiled = 0;
        glGetShaderiv(shader, COMPILE_STATUS, &mut compiled);
        if compiled == 0 {
            return Err(format!("Compiling a shader failed: {}", log(shader, true)));
        }
        Ok(shader)
    }
}

/// The info log of `object`, a shader or else a program.
unsafe fn log(object: GLuint, shader: bool) -> String {
    unsafe {
        let mut length = 0;
        if shader {
            glGetShaderiv(object, INFO_LOG_LENGTH, &mut length);
        } else {
            glGetProgramiv(object, INFO_LOG_LENGTH, &mut length);
        }
        let mut text = vec![0u8; length.max(1) as usize];
        let log = text.as_mut_ptr().cast();
        if shader {
            glGetShaderInfoLog(object, length, ptr::null_mut(), log);
        } else {
            glGetProgramInfoLog(object, length, ptr::null_mut(), log);
        }
        String::from_utf8_lossy(&text)
            .trim_end_matches('\0')
            .to_owned()
    }
}
