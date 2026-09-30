//! `--screenshot`: the window drawn offscreen at a fixed size in each appearance. On macOS
//! the parts AppKit draws over it, the traffic lights and the rounded corners, are painted
//! in; elsewhere the app draws its whole window. On macOS `SNOWBOUND_SCREENSHOT_SYSTEM=
//! snow-leopard` draws it as a Mac OS X 10.6 window instead.

#[cfg(target_os = "macos")]
mod snow_leopard;

use super::*;
use winit::window::Theme as Appearance;

/// The window in points, unless `SNOWBOUND_SCREENSHOT_SIZE` gives it as `WIDTHxHEIGHT`,
/// drawn at `SCALE` pixels per point or fewer where the GPU caps textures.
const SIZE: [f32; 2] = [1440.0, 900.0];
const SCALE: f32 = 2.0;
/// The window's content as a Snow Leopard window, its default size.
const SNOW_LEOPARD_SIZE: [f32; 2] = [1180.0, 760.0];
/// The close, minimize and zoom buttons' fill and rim.
#[cfg(target_os = "macos")]
const LIGHTS: [([u8; 3], [u8; 3]); 3] = [
    ([0xff, 0x5f, 0x57], [0xe2, 0x46, 0x3f]),
    ([0xfe, 0xbc, 0x2e], [0xe1, 0xa1, 0x16]),
    ([0x28, 0xc8, 0x40], [0x12, 0xac, 0x28]),
];

/// Whether `SNOWBOUND_SCREENSHOT_SYSTEM` asks for a Snow Leopard window.
fn snow_leopard() -> bool {
    cfg!(target_os = "macos")
        && std::env::var_os("SNOWBOUND_SCREENSHOT_SYSTEM")
            .is_some_and(|system| system == "snow-leopard")
}

/// Before the window opens: a Snow Leopard window lays the app out as on 10.6.
pub(crate) fn prepare() {
    #[cfg(target_os = "macos")]
    if snow_leopard() {
        crate::aqua::pretend();
    }
}

impl State {
    /// Writes `PREFIX-light.png` and `PREFIX-dark.png`, the page unfocused so no caret shows;
    /// as a Snow Leopard window, `PREFIX-snow-leopard.png` and, not key,
    /// `PREFIX-snow-leopard-other.png`, at 10.6's one pixel per point.
    pub(crate) fn screenshot(&mut self, prefix: &Path) -> Result<(), Box<dyn Error>> {
        let size = match std::env::var("SNOWBOUND_SCREENSHOT_SIZE") {
            Ok(size) => size
                .split_once('x')
                .and_then(|(width, height)| Some([width.parse().ok()?, height.parse().ok()?]))
                .ok_or("SNOWBOUND_SCREENSHOT_SIZE must be WIDTHxHEIGHT in points.")?,
            Err(_) if snow_leopard() => SNOW_LEOPARD_SIZE,
            Err(_) => SIZE,
        };
        let most = self.renderer.max_texture_dimension() as f32;
        let scale = if snow_leopard() {
            1.0
        } else {
            SCALE.min(most / size[0].max(size[1]))
        };
        self.surface.size = size.map(|side| (side * scale).max(1.0) as u32);
        self.renderer.clear_glyph_cache();
        let response = self.view.scale_factor_changed(scale)?;
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
        #[cfg(target_os = "macos")]
        if snow_leopard() {
            return snow_leopard::screenshot(self, prefix, size);
        }
        for (name, appearance) in [("light", Appearance::Light), ("dark", Appearance::Dark)] {
            self.window.set_theme(Some(appearance));
            self.set_appearance(appearance);
            self.settle_frame(size, scale)?;
            let path = PathBuf::from(format!("{}-{name}.png", prefix.display()));
            self.snapshot(&path)?;
            #[cfg(target_os = "macos")]
            window_frame(
                &path,
                appearance,
                scale,
                platform::traffic_lights(&self.window),
            )?;
        }
        Ok(())
    }
}

impl State {
    /// Lays the frame out until pictures and eased colours settle.
    fn settle_frame(&mut self, size: [f32; 2], scale: f32) -> Result<(), Box<dyn Error>> {
        // The page's box sizes the view whose pictures settle.
        self.layout(size, scale)?;
        let paper = self.paper();
        if let Some((scene, _)) = &mut self.view.scene {
            scene.settle(Some(&self.view.editor), self.view.viewport.scale, paper);
        }
        // Later frames lay out with earlier frames' measurements while colours ease.
        let deadline = Instant::now() + std::time::Duration::from_secs(2);
        loop {
            self.layout(size, scale)?;
            if !self.ui.wants_frame() || Instant::now() > deadline {
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_millis(16));
        }
    }
}

/// Paints the traffic lights centred at `lights` into the PNG at `path`, rounds its corners
/// to transparency and edges them with the window's hairline.
#[cfg(target_os = "macos")]
fn window_frame(
    path: &Path,
    appearance: Appearance,
    scale: f32,
    lights: [[f32; 2]; 3],
) -> Result<(), Box<dyn Error>> {
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
    let radius = platform::CORNER_RADIUS * scale;
    for (index, pixel) in pixels.chunks_exact_mut(4).enumerate() {
        let x = (index as u32 % info.width) as f32 + 0.5;
        let y = (index as u32 / info.width) as f32 + 0.5;
        for ([cx, cy], (fill, rim)) in lights.into_iter().zip(LIGHTS) {
            let distance = (x - cx * scale).hypot(y - cy * scale);
            blend(
                pixel,
                [rim[0], rim[1], rim[2], 255],
                inside(distance - 6.0 * scale),
            );
            blend(
                pixel,
                [fill[0], fill[1], fill[2], 255],
                inside(distance - 5.5 * scale),
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
