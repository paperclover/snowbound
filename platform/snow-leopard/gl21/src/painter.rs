//! The submission half of `draw::Renderer` on OpenGL 2.1: the atlas, image textures, one
//! vertex buffer and the ported shader. `draw` keeps building vertices and batches; this
//! draws them into an sRGB framebuffer object, as wgpu draws them into its target.
use crate::*;
use std::{ffi::CString, ops::Range, ptr};

/// Laid out as `draw`'s vertex.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Vertex {
    pub position: [f32; 2],
    pub uv: [f32; 2],
    pub color: [f32; 4],
    pub local: [f32; 2],
    pub shape: [f32; 4],
    pub stroke: f32,
}

pub struct Batch<'a> {
    pub vertices: Range<u32>,
    /// None samples the atlas with straight alpha; an image is premultiplied.
    pub image: Option<&'a Texture>,
    /// Device pixels from the top left: `[x, y, width, height]`.
    pub scissor: [u32; 4],
}

/// An sRGB RGBA texture.
pub struct Texture {
    name: GLuint,
    size: [u32; 2],
}

impl Texture {
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
                pixels.map_or(ptr::null(), |p| p.as_ptr().cast()),
            );
        }
        Self { name, size }
    }

    /// Writes `rgba` rows, top first, at `origin`.
    pub fn write(&self, origin: [u32; 2], size: [u32; 2], rgba: &[u8]) {
        assert_eq!(rgba.len(), (size[0] * size[1] * 4) as usize);
        unsafe {
            glBindTexture(TEXTURE_2D, self.name);
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

    pub fn size(&self) -> [u32; 2] {
        self.size
    }
}

impl Drop for Texture {
    fn drop(&mut self) {
        unsafe { glDeleteTextures(1, &self.name) };
    }
}

/// An offscreen sRGB colour target; blending happens in linear light.
pub struct Target {
    framebuffer: GLuint,
    texture: Texture,
}

impl Target {
    pub fn new(size: [u32; 2]) -> Result<Self, String> {
        let texture = Texture::new(size, NEAREST, None);
        let mut framebuffer = 0;
        unsafe {
            glGenFramebuffersEXT(1, &mut framebuffer);
            glBindFramebufferEXT(FRAMEBUFFER, framebuffer);
            glFramebufferTexture2DEXT(FRAMEBUFFER, COLOR_ATTACHMENT0, TEXTURE_2D, texture.name, 0);
            let status = glCheckFramebufferStatusEXT(FRAMEBUFFER);
            glBindFramebufferEXT(FRAMEBUFFER, 0);
            if status != FRAMEBUFFER_COMPLETE {
                return Err(format!("framebuffer incomplete: {status:#x}"));
            }
        }
        Ok(Self { framebuffer, texture })
    }

    pub fn size(&self) -> [u32; 2] {
        self.texture.size
    }

    /// Copies the target to the current context's window framebuffer, same size.
    pub fn present(&self) {
        let [w, h] = self.texture.size.map(|v| v as GLint);
        unsafe {
            glBindFramebufferEXT(READ_FRAMEBUFFER, self.framebuffer);
            glBindFramebufferEXT(DRAW_FRAMEBUFFER, 0);
            glBlitFramebufferEXT(0, 0, w, h, 0, 0, w, h, COLOR_BUFFER_BIT, NEAREST as GLenum);
            glBindFramebufferEXT(FRAMEBUFFER, 0);
        }
    }

    /// sRGB-encoded RGBA rows, top first.
    pub fn read_pixels(&self) -> Vec<u8> {
        let [w, h] = self.texture.size;
        let mut pixels = vec![0u8; (w * h * 4) as usize];
        unsafe {
            glBindFramebufferEXT(FRAMEBUFFER, self.framebuffer);
            glReadPixels(0, 0, w as GLsizei, h as GLsizei, RGBA, UNSIGNED_BYTE, pixels.as_mut_ptr().cast());
            glBindFramebufferEXT(FRAMEBUFFER, 0);
        }
        pixels.chunks(w as usize * 4).rev().flatten().copied().collect()
    }
}

impl Drop for Target {
    fn drop(&mut self) {
        unsafe { glDeleteFramebuffersEXT(1, &self.framebuffer) };
    }
}

pub struct Painter {
    program: GLuint,
    buffer: GLuint,
    pub atlas: Texture,
}

const ATTRIBUTES: [(&str, GLint, usize); 6] = [
    ("position", 2, 0),
    ("uv", 2, 8),
    ("color", 4, 16),
    ("local", 2, 32),
    ("shape", 4, 40),
    ("stroke", 1, 56),
];

impl Painter {
    /// Needs a current context.
    pub fn new(atlas_size: u32) -> Result<Self, String> {
        let source = include_str!("draw.glsl");
        let program = unsafe {
            let vertex = compile(VERTEX_SHADER, &format!("#version 120\n#define VERTEX\n{source}"))?;
            let fragment = compile(FRAGMENT_SHADER, &format!("#version 120\n{source}"))?;
            let program = glCreateProgram();
            glAttachShader(program, vertex);
            glAttachShader(program, fragment);
            for (index, (name, _, _)) in ATTRIBUTES.iter().enumerate() {
                let name = CString::new(*name).unwrap();
                glBindAttribLocation(program, index as GLuint, name.as_ptr());
            }
            glLinkProgram(program);
            let mut linked = 0;
            glGetProgramiv(program, LINK_STATUS, &mut linked);
            if linked == 0 {
                return Err(format!("link: {}", log(program, glGetProgramiv, glGetProgramInfoLog)));
            }
            program
        };
        let atlas = Texture::new([atlas_size; 2], NEAREST, None);
        // The atlas's first texel is the white that solid fills sample.
        atlas.write([0, 0], [1, 1], &[255; 4]);
        let mut buffer = 0;
        unsafe { glGenBuffers(1, &mut buffer) };
        Ok(Self { program, buffer, atlas })
    }

    /// An image texture, filtered linearly; pixels are premultiplied rows, top first.
    pub fn image(&self, size: [u32; 2], rgba: &[u8]) -> Texture {
        Texture::new(size, LINEAR, Some(rgba))
    }

    /// Clears `target` to linear `clear` and draws the batches in order.
    pub fn draw(&self, target: &Target, clear: [f32; 4], vertices: &[Vertex], batches: &[Batch<'_>]) {
        let [width, height] = target.size().map(|v| v as GLsizei);
        unsafe {
            glBindFramebufferEXT(FRAMEBUFFER, target.framebuffer);
            glEnable(FRAMEBUFFER_SRGB);
            glViewport(0, 0, width, height);
            glDisable(SCISSOR_TEST);
            glClearColor(clear[0], clear[1], clear[2], clear[3]);
            glClear(COLOR_BUFFER_BIT);
            glUseProgram(self.program);
            glUniform1i(glGetUniformLocation(self.program, c"atlas".as_ptr()), 0);
            glActiveTexture(TEXTURE0);
            glBindBuffer(ARRAY_BUFFER, self.buffer);
            glBufferData(ARRAY_BUFFER, size_of_val(vertices) as GLsizeiptr, vertices.as_ptr().cast(), STREAM_DRAW);
            for (index, (_, size, offset)) in ATTRIBUTES.iter().enumerate() {
                glEnableVertexAttribArray(index as GLuint);
                glVertexAttribPointer(
                    index as GLuint,
                    *size,
                    FLOAT,
                    0,
                    size_of::<Vertex>() as GLsizei,
                    ptr::without_provenance(*offset),
                );
            }
            glEnable(BLEND);
            glEnable(SCISSOR_TEST);
            for batch in batches {
                let [x, y, w, h] = batch.scissor.map(|v| v as GLint);
                glScissor(x, height - y - h, w, h);
                let (texture, source) = match batch.image {
                    Some(image) => (image.name, ONE),
                    None => (self.atlas.name, SRC_ALPHA),
                };
                glBlendFuncSeparate(source, ONE_MINUS_SRC_ALPHA, ONE, ONE_MINUS_SRC_ALPHA);
                glBindTexture(TEXTURE_2D, texture);
                glDrawArrays(
                    TRIANGLES,
                    batch.vertices.start as GLint,
                    batch.vertices.len() as GLsizei,
                );
            }
            glDisable(SCISSOR_TEST);
            glDisable(BLEND);
            glBindFramebufferEXT(FRAMEBUFFER, 0);
        }
    }
}

unsafe fn compile(kind: GLenum, source: &str) -> Result<GLuint, String> {
    unsafe {
        let shader = glCreateShader(kind);
        let text = CString::new(source).unwrap();
        glShaderSource(shader, 1, &text.as_ptr(), ptr::null());
        glCompileShader(shader);
        let mut compiled = 0;
        glGetShaderiv(shader, COMPILE_STATUS, &mut compiled);
        if compiled == 0 {
            return Err(format!("compile: {}", log(shader, glGetShaderiv, glGetShaderInfoLog)));
        }
        Ok(shader)
    }
}

unsafe fn log(
    object: GLuint,
    get: unsafe extern "C" fn(GLuint, GLenum, *mut GLint),
    read: unsafe extern "C" fn(GLuint, GLsizei, *mut GLsizei, *mut std::ffi::c_char),
) -> String {
    unsafe {
        let mut length = 0;
        get(object, INFO_LOG_LENGTH, &mut length);
        let mut text = vec![0u8; length.max(1) as usize];
        read(object, length, ptr::null_mut(), text.as_mut_ptr().cast());
        String::from_utf8_lossy(&text).trim_end_matches('\0').to_owned()
    }
}
