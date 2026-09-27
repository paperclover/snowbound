//! OpenGL 2.1 and the extensions every Mac OS X 10.6 GPU driver exposes, linked straight
//! from OpenGL.framework, which exports them all; no loader is needed.
#![allow(non_snake_case, clippy::missing_safety_doc)]

pub mod painter;

use std::ffi::{c_char, c_void};

pub type GLenum = u32;
pub type GLuint = u32;
pub type GLint = i32;
pub type GLsizei = i32;
pub type GLfloat = f32;
pub type GLboolean = u8;
pub type GLbitfield = u32;
pub type GLsizeiptr = isize;

pub const NO_ERROR: GLenum = 0;
pub const TRIANGLES: GLenum = 0x0004;
pub const COLOR_BUFFER_BIT: GLbitfield = 0x4000;
pub const BLEND: GLenum = 0x0BE2;
pub const SCISSOR_TEST: GLenum = 0x0C11;
pub const TEXTURE_2D: GLenum = 0x0DE1;
pub const UNSIGNED_BYTE: GLenum = 0x1401;
pub const FLOAT: GLenum = 0x1406;
pub const RGBA: GLenum = 0x1908;
pub const VENDOR: GLenum = 0x1F00;
pub const RENDERER: GLenum = 0x1F01;
pub const VERSION: GLenum = 0x1F02;
pub const EXTENSIONS: GLenum = 0x1F03;
pub const NEAREST: GLint = 0x2600;
pub const LINEAR: GLint = 0x2601;
pub const TEXTURE_MAG_FILTER: GLenum = 0x2800;
pub const TEXTURE_MIN_FILTER: GLenum = 0x2801;
pub const TEXTURE_WRAP_S: GLenum = 0x2802;
pub const TEXTURE_WRAP_T: GLenum = 0x2803;
pub const CLAMP_TO_EDGE: GLint = 0x812F;
pub const ONE: GLenum = 1;
pub const SRC_ALPHA: GLenum = 0x0302;
pub const ONE_MINUS_SRC_ALPHA: GLenum = 0x0303;
pub const TEXTURE0: GLenum = 0x84C0;
pub const ARRAY_BUFFER: GLenum = 0x8892;
pub const STREAM_DRAW: GLenum = 0x88E0;
pub const FRAGMENT_SHADER: GLenum = 0x8B30;
pub const VERTEX_SHADER: GLenum = 0x8B31;
pub const COMPILE_STATUS: GLenum = 0x8B81;
pub const LINK_STATUS: GLenum = 0x8B82;
pub const INFO_LOG_LENGTH: GLenum = 0x8B84;
pub const SRGB8_ALPHA8: GLint = 0x8C43;
pub const READ_FRAMEBUFFER: GLenum = 0x8CA8;
pub const DRAW_FRAMEBUFFER: GLenum = 0x8CA9;
pub const FRAMEBUFFER_COMPLETE: GLenum = 0x8CD5;
pub const COLOR_ATTACHMENT0: GLenum = 0x8CE0;
pub const FRAMEBUFFER: GLenum = 0x8D40;
pub const FRAMEBUFFER_SRGB: GLenum = 0x8DB9;

#[link(name = "OpenGL", kind = "framework")]
unsafe extern "C" {
    pub fn glGetError() -> GLenum;
    pub fn glGetString(name: GLenum) -> *const u8;
    pub fn glEnable(cap: GLenum);
    pub fn glDisable(cap: GLenum);
    pub fn glViewport(x: GLint, y: GLint, width: GLsizei, height: GLsizei);
    pub fn glScissor(x: GLint, y: GLint, width: GLsizei, height: GLsizei);
    pub fn glClearColor(r: GLfloat, g: GLfloat, b: GLfloat, a: GLfloat);
    pub fn glClear(mask: GLbitfield);
    pub fn glFinish();
    pub fn glFlush();
    pub fn glBlendFuncSeparate(src_rgb: GLenum, dst_rgb: GLenum, src_a: GLenum, dst_a: GLenum);
    pub fn glReadPixels(x: GLint, y: GLint, w: GLsizei, h: GLsizei, format: GLenum, ty: GLenum, data: *mut c_void);
    pub fn glPixelStorei(name: GLenum, value: GLint);

    pub fn glGenTextures(n: GLsizei, textures: *mut GLuint);
    pub fn glDeleteTextures(n: GLsizei, textures: *const GLuint);
    pub fn glBindTexture(target: GLenum, texture: GLuint);
    pub fn glActiveTexture(unit: GLenum);
    pub fn glTexParameteri(target: GLenum, name: GLenum, value: GLint);
    pub fn glTexImage2D(target: GLenum, level: GLint, internal: GLint, w: GLsizei, h: GLsizei, border: GLint, format: GLenum, ty: GLenum, data: *const c_void);
    pub fn glTexSubImage2D(target: GLenum, level: GLint, x: GLint, y: GLint, w: GLsizei, h: GLsizei, format: GLenum, ty: GLenum, data: *const c_void);

    pub fn glGenBuffers(n: GLsizei, buffers: *mut GLuint);
    pub fn glBindBuffer(target: GLenum, buffer: GLuint);
    pub fn glBufferData(target: GLenum, size: GLsizeiptr, data: *const c_void, usage: GLenum);
    pub fn glEnableVertexAttribArray(index: GLuint);
    pub fn glVertexAttribPointer(index: GLuint, size: GLint, ty: GLenum, normalized: GLboolean, stride: GLsizei, offset: *const c_void);
    pub fn glDrawArrays(mode: GLenum, first: GLint, count: GLsizei);

    pub fn glCreateShader(kind: GLenum) -> GLuint;
    pub fn glShaderSource(shader: GLuint, count: GLsizei, sources: *const *const c_char, lengths: *const GLint);
    pub fn glCompileShader(shader: GLuint);
    pub fn glGetShaderiv(shader: GLuint, name: GLenum, value: *mut GLint);
    pub fn glGetShaderInfoLog(shader: GLuint, size: GLsizei, length: *mut GLsizei, log: *mut c_char);
    pub fn glCreateProgram() -> GLuint;
    pub fn glAttachShader(program: GLuint, shader: GLuint);
    pub fn glBindAttribLocation(program: GLuint, index: GLuint, name: *const c_char);
    pub fn glLinkProgram(program: GLuint);
    pub fn glGetProgramiv(program: GLuint, name: GLenum, value: *mut GLint);
    pub fn glGetProgramInfoLog(program: GLuint, size: GLsizei, length: *mut GLsizei, log: *mut c_char);
    pub fn glUseProgram(program: GLuint);
    pub fn glGetUniformLocation(program: GLuint, name: *const c_char) -> GLint;
    pub fn glUniform1i(location: GLint, value: GLint);

    pub fn glGenFramebuffersEXT(n: GLsizei, framebuffers: *mut GLuint);
    pub fn glDeleteFramebuffersEXT(n: GLsizei, framebuffers: *const GLuint);
    pub fn glBindFramebufferEXT(target: GLenum, framebuffer: GLuint);
    pub fn glFramebufferTexture2DEXT(target: GLenum, attachment: GLenum, textarget: GLenum, texture: GLuint, level: GLint);
    pub fn glCheckFramebufferStatusEXT(target: GLenum) -> GLenum;
    pub fn glBlitFramebufferEXT(sx0: GLint, sy0: GLint, sx1: GLint, sy1: GLint, dx0: GLint, dy0: GLint, dx1: GLint, dy1: GLint, mask: GLbitfield, filter: GLenum);
}

/// A GL string such as `VERSION` or `EXTENSIONS`, empty without a current context.
pub fn string(name: GLenum) -> String {
    let text = unsafe { glGetString(name) };
    if text.is_null() {
        return String::new();
    }
    unsafe { std::ffi::CStr::from_ptr(text.cast()) }.to_string_lossy().into_owned()
}
