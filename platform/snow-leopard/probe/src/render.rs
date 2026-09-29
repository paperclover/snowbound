//! Paints `scene` through `draw` over OpenGL into an offscreen target and writes
//! `render-probe.png`.
#![allow(deprecated)] // OpenGL is the only GPU API 10.6 has.
mod context;
mod scene;

fn main() {
    objc2::rc::autoreleasepool(|_| run());
}

fn run() {
    let _context = context::current();
    let mut renderer = draw::Renderer::new().unwrap_or_else(|error| panic!("{error}"));
    let target = draw::Target::new(scene::SIZE).unwrap();
    let checker = scene::checker();
    let primitives = scene::primitives(&checker);
    let started = std::time::Instant::now();
    renderer.draw(&target, scene::SIZE, scene::CLEAR, &[scene::layer(&primitives)]).unwrap();
    let pixels = target.read_pixels();
    println!("drew in {:?}", started.elapsed());
    let file = std::fs::File::create("render-probe.png").unwrap();
    let mut encoder = png::Encoder::new(file, scene::SIZE[0], scene::SIZE[1]);
    encoder.set_color(png::ColorType::Rgba);
    encoder.write_header().unwrap().write_image_data(&pixels).unwrap();
}
