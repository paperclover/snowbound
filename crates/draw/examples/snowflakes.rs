//! Snowflakes grown by Reiter's hexagonal automaton ("A local cellular model for snow
//! crystal growth", 2005), painted through Snowbound's draw crate.
//!
//! `cargo run --release -p draw --example snowflakes -- [first seed] [out dir]` writes
//! `gallery.png`, twelve flakes from consecutive seeds, and `growing.gif`, the first one freezing.

use std::{f32::consts::PI, fmt::Write as _, path::PathBuf, time::Duration};

use draw::{Layer, PathStyle, Primitive, Renderer, srgb};

/// Hex cells from the centre to the edge of the grid.
const RADIUS: i32 = 110;
const SIDE: usize = 2 * RADIUS as usize + 1;
const NEIGHBOURS: [(i32, i32); 6] = [(1, 0), (-1, 0), (0, 1), (0, -1), (1, -1), (-1, 1)];
const TILE: u32 = 512;
const BACKGROUND: [u8; 3] = [9, 16, 36];

struct Weather {
    /// How quickly vapour spreads.
    alpha: f32,
    /// Vapour everywhere at the start, and at the grid's edge always.
    beta: f32,
    /// Vapour condensing onto the crystal each step.
    gamma: f32,
}

impl Weather {
    fn from_seed(seed: u64) -> Self {
        let mut state = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ 0x5eed;
        let mut next = || {
            state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = state;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            ((z ^ (z >> 31)) >> 40) as f32 / (1u64 << 24) as f32
        };
        Self {
            alpha: 0.7 + 1.6 * next(),
            beta: 0.3 + 0.6 * next(),
            gamma: if next() < 0.2 {
                0.0
            } else {
                10f32.powf(-4.0 + 1.6 * next())
            },
        }
    }
}

/// Axial cell `(q, r)` with every `|q|`, `|r|`, `|q + r|` within `RADIUS`.
fn index(q: i32, r: i32) -> Option<usize> {
    (q.abs() <= RADIUS && r.abs() <= RADIUS && (q + r).abs() <= RADIUS)
        .then(|| (q + RADIUS) as usize * SIDE + (r + RADIUS) as usize)
}

fn cells() -> impl Iterator<Item = (i32, i32, usize)> {
    (-RADIUS..=RADIUS)
        .flat_map(|q| (-RADIUS..=RADIUS).map(move |r| (q, r)))
        .filter_map(|(q, r)| index(q, r).map(|i| (q, r, i)))
}

/// Water at each cell; a cell holding 1 or more is ice.
type Field = Vec<f32>;

/// Grows a flake until its arms near the grid's edge, offering `frame` each step.
fn grow(weather: &Weather, mut frame: impl FnMut(&Field)) -> Field {
    let Weather { alpha, beta, gamma } = *weather;
    let mut water = vec![beta; SIDE * SIDE];
    water[index(0, 0).unwrap()] = 1.0;
    let mut diffusing = vec![0.0; SIDE * SIDE];
    let mut spread = diffusing.clone();
    let mut held = diffusing.clone();
    let neighbours = |q: i32, r: i32| NEIGHBOURS.map(|(dq, dr)| index(q + dq, r + dr));
    for _ in 0..60_000 {
        for (q, r, i) in cells() {
            let receptive =
                water[i] >= 1.0 || neighbours(q, r).iter().flatten().any(|&n| water[n] >= 1.0);
            (diffusing[i], held[i]) = if receptive {
                (0.0, water[i] + gamma)
            } else {
                (water[i], 0.0)
            };
        }
        let mut reach = 0;
        for (q, r, i) in cells() {
            let around: f32 = neighbours(q, r)
                .iter()
                .map(|n| n.map_or(beta, |n| diffusing[n]))
                .sum();
            spread[i] = diffusing[i] + alpha / 12.0 * (around - 6.0 * diffusing[i]);
            water[i] = spread[i] + held[i];
            if water[i] >= 1.0 {
                reach = reach.max(q.abs().max(r.abs()).max((q + r).abs()));
            }
        }
        frame(&water);
        if reach >= RADIUS - 6 {
            break;
        }
    }
    water
}

/// The farthest ice from the centre, in cells across.
fn extent(water: &Field) -> f32 {
    cells()
        .filter(|&(_, _, i)| water[i] >= 1.0)
        .map(|(q, r, _)| {
            3f32.sqrt() * ((q as f32 + r as f32 / 2.0).powi(2) + (0.75 * (r * r) as f32)).sqrt()
        })
        .fold(1.0, f32::max)
}

/// The flake as stacked translucent layers: a glow, then ice thickening toward white.
fn paint(
    water: &Field,
    reach: f32,
    centre: [f32; 2],
    size: f32,
) -> Vec<(String, PathStyle, [[f32; 4]; 2])> {
    let hexagon = |q: i32, r: i32, path: &mut String| {
        let x = centre[0] + size * 3f32.sqrt() * (q as f32 + r as f32 / 2.0);
        let y = centre[1] + size * 1.5 * r as f32;
        for corner in 0..6 {
            let angle = PI / 3.0 * corner as f32 + PI / 6.0;
            let command = if corner == 0 { 'M' } else { 'L' };
            let _ = write!(
                path,
                "{command}{:.2} {:.2}",
                x + size * angle.cos(),
                y + size * angle.sin()
            );
        }
        path.push('Z');
    };
    let ice: Vec<_> = cells().filter(|&(_, _, i)| water[i] >= 1.0).collect();
    let heaviest = ice.iter().map(|&(_, _, i)| water[i]).fold(1.0, f32::max);
    let thickness = |i: usize| ((water[i] - 1.0) / (heaviest - 1.0).max(1e-6)).sqrt();
    const BANDS: usize = 7;
    let mut layers = Vec::new();
    let mut all = String::new();
    ice.iter().for_each(|&(q, r, _)| hexagon(q, r, &mut all));
    let glow = [0.25, 0.55, 1.0, 0.45];
    layers.push((
        all.clone(),
        PathStyle::Shadow(size * reach * 0.04 + 6.0),
        [glow, glow],
    ));
    layers.push((
        all,
        PathStyle::Fill,
        [[0.32, 0.55, 0.95, 0.55], [0.18, 0.35, 0.80, 0.55]],
    ));
    for band in 1..BANDS {
        let mut path = String::new();
        for &(q, r, i) in &ice {
            if thickness(i) * BANDS as f32 >= band as f32 {
                hexagon(q, r, &mut path);
            }
        }
        if !path.is_empty() {
            layers.push((
                path,
                PathStyle::Fill,
                [[0.95, 0.98, 1.0, 0.24], [0.80, 0.90, 1.0, 0.20]],
            ));
        }
    }
    layers
}

struct Gpu {
    renderer: Renderer,
    texture: wgpu::Texture,
    readback: wgpu::Buffer,
}

impl Gpu {
    fn new() -> Self {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter =
            pollster::block_on(instance.request_adapter(&Default::default())).expect("a GPU");
        let (device, queue) =
            pollster::block_on(adapter.request_device(&Default::default())).expect("a device");
        let format = wgpu::TextureFormat::Rgba8UnormSrgb;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Flake"),
            size: wgpu::Extent3d {
                width: TILE,
                height: TILE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Flake readback"),
            size: u64::from(TILE * TILE * 4),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            renderer: Renderer::new(device, queue, format),
            texture,
            readback,
        }
    }

    /// One tile of sRGB RGBA rows.
    fn render(&mut self, water: &Field, reach: f32) -> image::RgbaImage {
        let size = TILE as f32 * 0.46 / reach;
        let centre = [TILE as f32 / 2.0; 2];
        let layers = paint(water, reach, centre, size);
        let primitives: Vec<_> = layers
            .iter()
            .map(|(data, style, colors)| Primitive::Path {
                data,
                origin: [0.0; 2],
                style: *style,
                colors: *colors,
            })
            .collect();
        let [red, green, blue] = BACKGROUND;
        self.renderer.clear_glyph_cache();
        self.renderer
            .draw(
                &draw::Target::from(self.texture.create_view(&Default::default())),
                [TILE; 2],
                srgb(red, green, blue),
                &[Layer {
                    scale: 1.0,
                    origin: [0.0; 2],
                    clip: None,
                    backdrop: None,
                    round: None,
                    motion: None,
                    primitives: &primitives,
                }],
            )
            .expect("a frame");
        let device = self.renderer.device();
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            self.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &self.readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(TILE * 4),
                    rows_per_image: Some(TILE),
                },
            },
            wgpu::Extent3d {
                width: TILE,
                height: TILE,
                depth_or_array_layers: 1,
            },
        );
        self.renderer.queue().submit([encoder.finish()]);
        let (sender, receiver) = std::sync::mpsc::channel();
        self.readback
            .map_async(wgpu::MapMode::Read, .., move |result| {
                sender.send(result).unwrap()
            });
        device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(Duration::from_secs(10)),
            })
            .unwrap();
        receiver
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .unwrap();
        let pixels = self.readback.get_mapped_range(..).unwrap().to_vec();
        self.readback.unmap();
        image::RgbaImage::from_raw(TILE, TILE, pixels).unwrap()
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let first: u64 = args
        .next()
        .map_or(1, |seed| seed.parse().expect("a numeric seed"));
    let out = PathBuf::from(args.next().unwrap_or_else(|| ".".into()));
    std::fs::create_dir_all(&out).unwrap();

    let seeds: Vec<u64> = (first..first + 12).collect();
    let mut growing = Vec::new();
    let flakes: Vec<Field> = std::thread::scope(|scope| {
        let handles: Vec<_> = seeds
            .iter()
            .map(|&seed| scope.spawn(move || grow(&Weather::from_seed(seed), |_| {})))
            .collect();
        // Snapshots at a widening interval, halved whenever they pass 400.
        let (mut step, mut every) = (0, 1);
        let shown = grow(&Weather::from_seed(first), |water| {
            if step % every == 0 {
                growing.push(water.clone());
                if growing.len() > 400 {
                    growing = growing.iter().step_by(2).cloned().collect();
                    every *= 2;
                }
            }
            step += 1;
        });
        growing.push(shown.clone());
        let mut flakes: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();
        flakes[0] = shown;
        flakes
    });

    let mut gpu = Gpu::new();
    let mut gallery = image::RgbaImage::new(TILE * 4, TILE * 3);
    for (n, (water, seed)) in flakes.iter().zip(&seeds).enumerate() {
        let Weather { alpha, beta, gamma } = Weather::from_seed(*seed);
        println!("seed {seed}: alpha {alpha:.2} beta {beta:.3} gamma {gamma:.5}");
        let tile = gpu.render(water, extent(water));
        image::imageops::overlay(
            &mut gallery,
            &tile,
            i64::from(n as u32 % 4 * TILE),
            i64::from(n as u32 / 4 * TILE),
        );
    }
    gallery.save(out.join("gallery.png")).unwrap();

    // Ninety frames spread over the growth, the finished flake held at the end.
    let reach = extent(growing.last().unwrap());
    let picks: Vec<usize> = (0..90).map(|f| f * (growing.len() - 1) / 89).collect();
    let file = std::fs::File::create(out.join("growing.gif")).unwrap();
    let mut encoder = image::codecs::gif::GifEncoder::new_with_speed(file, 10);
    encoder
        .set_repeat(image::codecs::gif::Repeat::Infinite)
        .unwrap();
    for (n, &pick) in picks.iter().enumerate() {
        let tile = image::imageops::resize(
            &gpu.render(&growing[pick], reach),
            320,
            320,
            image::imageops::FilterType::Triangle,
        );
        let delay = if n + 1 == picks.len() { 2500 } else { 50 };
        encoder
            .encode_frame(image::Frame::from_parts(
                tile,
                0,
                0,
                image::Delay::from_numer_denom_ms(delay, 1),
            ))
            .unwrap();
    }
}
