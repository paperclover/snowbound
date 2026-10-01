//! Paints `scene::interface` through `draw` over OpenGL and writes `text-probe.png`, to
//! set beside a Cocoa label's pixels.
#![allow(deprecated)] // OpenGL is the only GPU API 10.6 has.
mod context;
mod scene;

fn main() {
    objc2::rc::autoreleasepool(|_| {
        let _context = context::current();
        let mut renderer = draw::Renderer::opengl().unwrap_or_else(|error| panic!("{error}"));
        let target = renderer.target(scene::TEXT_SIZE).unwrap();
        let ui = scene::interface(1.0);
        let layers = ui.layers();
        renderer.draw(&target, scene::TEXT_SIZE, [1.0; 4], &scene::interface_layers(&layers, 1.0)).unwrap();
        let file = std::fs::File::create("text-probe.png").unwrap();
        let mut encoder = png::Encoder::new(file, scene::TEXT_SIZE[0], scene::TEXT_SIZE[1]);
        encoder.set_color(png::ColorType::Rgba);
        encoder.write_header().unwrap().write_image_data(&renderer.read_pixels(&target).unwrap()).unwrap();
    });
}
