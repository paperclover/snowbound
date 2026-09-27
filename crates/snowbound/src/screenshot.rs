//! `--screenshot`: the window drawn offscreen at a fixed size in each appearance. On macOS
//! the parts AppKit draws over it, the traffic lights and the rounded corners, are painted
//! in; elsewhere the app draws its whole window.

use super::*;
use winit::window::Theme as Appearance;

/// The window in points, drawn at `SCALE` pixels per point.
const SIZE: [f32; 2] = [1440.0, 900.0];
const SCALE: f32 = 2.0;
/// A standard window's close, minimize and zoom buttons as AppKit places them beside a
/// transparent title bar: centres in points, fill and rim.
#[cfg(target_os = "macos")]
const LIGHTS: [(f32, [u8; 3], [u8; 3]); 3] = [
    (14.0, [0xff, 0x5f, 0x57], [0xe2, 0x46, 0x3f]),
    (34.0, [0xfe, 0xbc, 0x2e], [0xe1, 0xa1, 0x16]),
    (54.0, [0x28, 0xc8, 0x40], [0x12, 0xac, 0x28]),
];

impl State {
    /// Writes `PREFIX-light.png` and `PREFIX-dark.png`, the page unfocused so no caret shows.
    pub(crate) fn screenshot(&mut self, prefix: &Path) -> Result<(), Box<dyn Error>> {
        [self.config.width, self.config.height] = SIZE.map(|side| (side * SCALE) as u32);
        self.renderer.clear_glyph_cache();
        self.app_icon = platform::app_icon((16.0 * SCALE) as u32);
        let response = self.view.scale_factor_changed(SCALE)?;
        self.respond(response);
        self.ui.set_focus(None);
        if let Some(title) = self
            .view
            .editor
            .outlines()
            .iter()
            .find(|outline| outline.title)
        {
            self.view.editor.focus_outline(title.id)?;
        }
        for (name, appearance) in [("light", Appearance::Light), ("dark", Appearance::Dark)] {
            self.window.set_theme(Some(appearance));
            self.ui.theme = theme(appearance, self.light_pages);
            // The page's box sizes the view whose pictures settle.
            self.layout(SIZE, SCALE)?;
            let paper = self.paper();
            if let Some((scene, _)) = &mut self.view.scene {
                scene.settle(Some(&self.view.editor), self.view.viewport.scale, paper);
            }
            // Later frames lay out with earlier frames' measurements while colours ease.
            let deadline = Instant::now() + std::time::Duration::from_secs(2);
            loop {
                self.layout(SIZE, SCALE)?;
                if !self.ui.wants_frame() || Instant::now() > deadline {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(16));
            }
            let path = PathBuf::from(format!("{}-{name}.png", prefix.display()));
            self.snapshot(&path)?;
            #[cfg(target_os = "macos")]
            window_frame(&path, appearance)?;
        }
        Ok(())
    }
}

/// Paints the traffic lights into the PNG at `path`, rounds its corners to transparency
/// and edges them with the window's hairline.
#[cfg(target_os = "macos")]
fn window_frame(path: &Path, appearance: Appearance) -> Result<(), Box<dyn Error>> {
    let mut reader =
        png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path)?)).read_info()?;
    let mut pixels = vec![0; reader.output_buffer_size().ok_or("Snapshot too large")?];
    let info = reader.next_frame(&mut pixels)?;
    let hairline = match appearance {
        Appearance::Light => [0, 0, 0, 56],
        Appearance::Dark => [255, 255, 255, 56],
    };
    // Coverage of a pixel whose centre lies `distance` pixels outside a shape's edge.
    let inside = |distance: f32| (0.5 - distance).clamp(0.0, 1.0);
    let blend = |pixel: &mut [u8], [r, g, b, a]: [u8; 4], coverage: f32| {
        let weight = coverage * f32::from(a) / 255.0;
        for (channel, value) in pixel.iter_mut().zip([r, g, b]) {
            *channel =
                (f32::from(*channel) * (1.0 - weight) + f32::from(value) * weight).round() as u8;
        }
    };
    let [width, height] = [info.width as f32, info.height as f32];
    let radius = platform::CORNER_RADIUS * SCALE;
    for (index, pixel) in pixels.chunks_exact_mut(4).enumerate() {
        let x = (index as u32 % info.width) as f32 + 0.5;
        let y = (index as u32 / info.width) as f32 + 0.5;
        for (centre, fill, rim) in LIGHTS {
            let distance = (x - centre * SCALE).hypot(y - TITLE / 2.0 * SCALE);
            blend(
                pixel,
                [rim[0], rim[1], rim[2], 255],
                inside(distance - 6.0 * SCALE),
            );
            blend(
                pixel,
                [fill[0], fill[1], fill[2], 255],
                inside(distance - 5.5 * SCALE),
            );
        }
        // Signed distance to the window's rounded rectangle.
        let [dx, dy] = [
            (x - width / 2.0).abs() - (width / 2.0 - radius),
            (y - height / 2.0).abs() - (height / 2.0 - radius),
        ];
        let edge = dx.max(0.0).hypot(dy.max(0.0)) + dx.max(dy).min(0.0) - radius;
        blend(pixel, hairline, inside((edge + 0.5).abs() - 0.5));
        pixel[3] = (f32::from(pixel[3]) * inside(edge)).round() as u8;
    }
    let partial = path.with_extension("partial");
    let mut encoder = png::Encoder::new(std::fs::File::create(&partial)?, info.width, info.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(&pixels)?;
    std::fs::rename(partial, path)?;
    Ok(())
}
