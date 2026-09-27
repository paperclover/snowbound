//! Paints `scene` through the GL 2.1 painter into an offscreen target and writes
//! `render-probe.png`. The vertices follow `draw::Renderer`'s quad and rounded-rectangle
//! encoding at scale 1.
#![allow(deprecated)] // OpenGL is the only GPU API 10.6 has.
mod scene;

use gl21::painter::{Batch, Painter, Target, Vertex};
use objc2::MainThreadMarker;
use objc2_app_kit::{NSOpenGLContext, NSOpenGLPFAAccelerated, NSOpenGLPFAColorSize, NSOpenGLPixelFormat};
use scene::Shape;

const WHITE_UV: [f32; 4] = [0.5 / ATLAS as f32; 4];
const ATLAS: u32 = 2048;

fn quad(vertices: &mut Vec<Vertex>, rect: [f32; 4], uv: [f32; 4], color: [f32; 4]) {
    let [w, h] = scene::SIZE.map(|v| v as f32);
    let (x0, x1) = (rect[0] * 2.0 / w - 1.0, rect[2] * 2.0 / w - 1.0);
    let (y0, y1) = (1.0 - rect[1] * 2.0 / h, 1.0 - rect[3] * 2.0 / h);
    let [hx, hy] = [(rect[2] - rect[0]) * 0.5, (rect[3] - rect[1]) * 0.5];
    for (position, uv, local) in [
        ([x0, y0], [uv[0], uv[1]], [-hx, -hy]),
        ([x0, y1], [uv[0], uv[3]], [-hx, hy]),
        ([x1, y1], [uv[2], uv[3]], [hx, hy]),
        ([x0, y0], [uv[0], uv[1]], [-hx, -hy]),
        ([x1, y1], [uv[2], uv[3]], [hx, hy]),
        ([x1, y0], [uv[2], uv[1]], [hx, -hy]),
    ] {
        vertices.push(Vertex { position, uv, color, local, ..Default::default() });
    }
}

fn rounded(vertices: &mut Vec<Vertex>, rect: [f32; 4], radius: [f32; 2], stroke: Option<(f32, bool)>, colors: [[f32; 4]; 2]) {
    let start = vertices.len();
    quad(vertices, [rect[0] - 1.0, rect[1] - 1.0, rect[2] + 1.0, rect[3] + 1.0], WHITE_UV, colors[0]);
    let half = [(rect[2] - rect[0]) * 0.5, (rect[3] - rect[1]) * 0.5];
    let radius = if radius.contains(&0.0) { [0.0; 2] } else { [radius[0].min(half[0]), radius[1].min(half[1])] };
    let width = stroke.map_or(0.0, |(width, dashed)| {
        let width = width.min(half[0].min(half[1]));
        if dashed { -width } else { width }
    });
    for vertex in &mut vertices[start..] {
        vertex.shape = [half[0], half[1], radius[0], radius[1]];
        vertex.stroke = width;
        if vertex.local[1] > 0.0 {
            vertex.color = colors[1];
        }
    }
}

fn main() {
    objc2::rc::autoreleasepool(|_| run());
}

fn run() {
    let mtm = MainThreadMarker::new().unwrap();
    let attributes = [NSOpenGLPFAAccelerated, NSOpenGLPFAColorSize, 24, 0];
    let format = unsafe {
        NSOpenGLPixelFormat::initWithAttributes(mtm.alloc(), std::ptr::NonNull::new(attributes.as_ptr().cast_mut()).unwrap())
    }
    .expect("pixel format");
    let context = NSOpenGLContext::initWithFormat_shareContext(mtm.alloc(), &format, None).expect("context");
    context.makeCurrentContext();

    let painter = Painter::new(ATLAS).unwrap_or_else(|error| panic!("{error}"));
    let target = Target::new(scene::SIZE).unwrap();
    let (checker_size, checker) = scene::checker();
    let image = painter.image(checker_size, &checker);
    let mut vertices = Vec::new();
    let mut spans = Vec::new();
    for shape in scene::shapes() {
        let start = vertices.len() as u32;
        let is_image = match shape {
            Shape::Rect { rect, color } => {
                quad(&mut vertices, rect, WHITE_UV, color);
                false
            }
            Shape::Rounded { rect, radius, stroke, colors } => {
                rounded(&mut vertices, rect, radius, stroke, colors);
                false
            }
            Shape::Image { rect } => {
                quad(&mut vertices, rect, [0.0, 0.0, 1.0, 1.0], [1.0; 4]);
                true
            }
        };
        spans.push((start..vertices.len() as u32, is_image));
    }
    let batches: Vec<Batch> = spans
        .into_iter()
        .map(|(vertices, is_image)| Batch {
            vertices,
            image: is_image.then_some(&image),
            scissor: [0, 0, scene::SIZE[0], scene::SIZE[1]],
        })
        .collect();
    let started = std::time::Instant::now();
    painter.draw(&target, scene::CLEAR, &vertices, &batches);
    let pixels = target.read_pixels();
    println!("drew {} vertices in {:?}; GL error {:#x}", vertices.len(), started.elapsed(), unsafe { gl21::glGetError() });
    let file = std::fs::File::create("render-probe.png").unwrap();
    let mut encoder = png::Encoder::new(file, scene::SIZE[0], scene::SIZE[1]);
    encoder.set_color(png::ColorType::Rgba);
    encoder.write_header().unwrap().write_image_data(&pixels).unwrap();
}
