//! A Mac OS X 10.6 window around the app, drawn anywhere: AppKit's textured gradient
//! through the title line, the toolbar's row and the tab row, the title and its document
//! icon in Lucida Grande, the window buttons as sampled from 10.6, the rounded corners and
//! the window's shadow. The app lays itself out as on 10.6 (`aqua::pretend`).

use super::*;

/// The title line's height.
const TITLE_LINE: usize = 22;
/// The textured gradient's greys as 10.6 draws them, key and not: the title line's top
/// highlight, the gradient's first and last rows, and the window below it.
const GREYS: [[f32; 4]; 2] = [[226.0, 208.0, 167.0, 167.0], [244.0, 237.0, 216.0, 216.0]];
/// The window buttons, sampled with `remote.sh art`: three 14 pixels wide, 21 apart from
/// 8 pixels in, 4 down.
const BUTTONS: [&[u8]; 2] = [
    include_bytes!("../../assets/snow-leopard/buttons-key.png"),
    include_bytes!("../../assets/snow-leopard/buttons-other.png"),
];
/// The title's colour, key and not.
const INK: [u8; 2] = [0, 128];
/// The window's shadow, key and not, as 10.6 captures a window: the room around the window
/// either side, above and below, then the shadow's softness as a box blur's radius, how
/// far it falls and how dark it is at its darkest.
const SHADOW: [([usize; 3], usize, usize, f32); 2] =
    [([40, 25, 55], 14, 15, 0.5), ([20, 10, 30], 7, 10, 0.4)];

pub(super) fn screenshot(
    state: &mut State,
    prefix: &Path,
    size: [f32; 2],
) -> Result<(), Box<dyn Error>> {
    let title = state.window.title();
    for (name, key) in [("snow-leopard", true), ("snow-leopard-other", false)] {
        state.window.set_theme(Some(Appearance::Light));
        state.set_appearance(Appearance::Light);
        // Over the textured window's gradient, as `install_backdrop` leaves it on 10.6.
        state.ui.theme = theme(Appearance::Light, state.light_pages, true);
        state.titlebar = [state.ui.theme.strip; 2];
        state.ui.window_focused = key;
        state.settle_frame(size, 1.0)?;
        let content = state.capture()?;
        let bar = title_bar(state, &title, size[0], key)?;
        let [width, height] = [size[0] as usize, size[1] as usize];
        let window = window(&content, &bar, [width, height], key)?;
        let (canvas, shadowed) = shadow(&window, [width, height + TITLE_LINE], key);
        let path = PathBuf::from(format!("{}-{name}.png", prefix.display()));
        write_png(&path, canvas.map(|side| side as u32), &shadowed)?;
    }
    Ok(())
}

/// The title and its document icon, centred as AppKit centres them: premultiplied pixels
/// as the renderer leaves them.
fn title_bar(
    state: &mut State,
    title: &str,
    width: f32,
    key: bool,
) -> Result<Vec<u8>, Box<dyn Error>> {
    let mut chrome = ui::Ui::new(ui::Theme::light(), std::time::Duration::ZERO);
    chrome.set_system_font("Lucida Grande");
    chrome.begin([width, TITLE_LINE as f32], 1.0, Instant::now());
    let text = chrome.measure(title)[0];
    // AppKit centres the icon's 16 pixels, a gap of 4 and the title, 2 pixels to the left.
    let x = ((width - 20.0 - text) / 2.0).round() - 2.0;
    let icon = document_icon();
    chrome.leaf(
        "document",
        Spec {
            flags: Flags::FLOAT,
            size: [px(16.0); 2],
            position: [x, 3.0],
            image: Some(&icon),
            ..Spec::default()
        },
    );
    let ink = INK[usize::from(!key)];
    chrome.leaf(
        "title",
        Spec {
            flags: Flags::FLOAT,
            size: [fit(), px(TITLE_LINE as f32)],
            position: [x + 20.0, -1.0],
            text: Some(title),
            color: Some(draw::srgb(ink, ink, ink)),
            ..Spec::default()
        },
    );
    chrome.end();
    let interface = chrome.layers();
    let layers: Vec<_> = interface
        .iter()
        .map(|layer| match layer {
            ui::Layer::Primitives(primitives) => primitives.layer(1.0),
            ui::Layer::Custom { .. } => unreachable!("The title bar has no custom boxes"),
        })
        .collect();
    let content = state.surface.size;
    state.surface.size = [width as u32, TITLE_LINE as u32];
    let offscreen = state.surface.offscreen(&state.renderer)?;
    let drawn = state
        .renderer
        .draw(&offscreen.target, state.surface.size, [0.0; 4], &layers)
        .map_err(|error| format!("Drawing the title bar failed: {error:?}"));
    let pixels = state.surface.read(&state.renderer, offscreen);
    state.surface.size = content;
    drawn?;
    pixels
}

/// A document's icon at 16 pixels: a page with its corner folded.
fn document_icon() -> draw::RasterImage {
    let pixels = (0..16)
        .flat_map(|y| (0..16).map(move |x| (x, y)))
        .flat_map(|(x, y): (i32, i32)| {
            let [left, right, top, bottom, fold] = [2, 13, 0, 15, 4];
            let folded = x - (right - fold) + (fold - y);
            let grey = if x < left || x > right || y < top || y > bottom || folded > fold {
                None
            } else if x == left || x == right || y == top || y == bottom || folded == fold {
                Some(150)
            } else if folded >= 0 && x > right - fold && y < fold {
                Some(226)
            } else {
                Some(236 + y as u8)
            };
            grey.map_or([0; 4], |grey| [grey, grey, grey, 255])
        })
        .collect();
    draw::RasterImage::new([16, 16], pixels).expect("The icon is a valid image")
}

/// sRGB bytes to and from linear light.
fn decode(value: f32) -> f32 {
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

fn encode(value: f32) -> f32 {
    if value <= 0.0031308 {
        value * 12.92
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    }
}

/// Lays renderer pixels over `under`, straight sRGB, as the window server composites a
/// transparent surface: premultiplied in sRGB.
fn composite(under: &mut [f32; 4], rendered: &[u8]) {
    let alpha = f32::from(rendered[3]) / 255.0;
    if alpha == 0.0 {
        return;
    }
    for channel in 0..3 {
        let linear = decode(f32::from(rendered[channel]) / 255.0) / alpha;
        let over = encode(linear.min(1.0)) * 255.0;
        under[channel] = over * alpha + under[channel] * (1.0 - alpha);
    }
}

/// Straight-alpha sRGB `source` over `under`.
fn over(under: &mut [f32; 4], source: [u8; 4]) {
    let alpha = f32::from(source[3]) / 255.0;
    for channel in 0..3 {
        under[channel] = f32::from(source[channel]) * alpha + under[channel] * (1.0 - alpha);
    }
}

/// The window: its gradient and grey, the app's `content` and the title `bar` over it, the
/// buttons, and its corners cut. Straight sRGB, `size` being the content's.
fn window(
    content: &[u8],
    bar: &[u8],
    [width, height]: [usize; 2],
    key: bool,
) -> Result<Vec<[f32; 4]>, Box<dyn Error>> {
    let [highlight, top, bottom, body] = GREYS[usize::from(!key)];
    // The gradient runs from under the highlight to the top content border's last row.
    let end = TITLE_LINE + (crate::TITLE + crate::TAB_ROW) as usize - 1;
    let total = height + TITLE_LINE;
    let mut pixels: Vec<[f32; 4]> = (0..total)
        .flat_map(|y| {
            let grey = match y {
                0 => highlight,
                _ if y <= end => top + (bottom - top) * (y - 1) as f32 / (end - 1) as f32,
                _ => body,
            };
            std::iter::repeat_n([grey, grey, grey, 255.0], width)
        })
        .collect();
    for (y, row) in content.chunks_exact(width * 4).enumerate() {
        for (x, pixel) in row.chunks_exact(4).enumerate() {
            composite(&mut pixels[(y + TITLE_LINE) * width + x], pixel);
        }
    }
    for (y, row) in bar.chunks_exact(width * 4).enumerate() {
        for (x, pixel) in row.chunks_exact(4).enumerate() {
            composite(&mut pixels[y * width + x], pixel);
        }
    }
    let mut decoder = png::Decoder::new(std::io::Cursor::new(BUTTONS[usize::from(!key)]));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info()?;
    let mut buttons = vec![0; reader.output_buffer_size().ok_or("Buttons too large")?];
    let info = reader.next_frame(&mut buttons)?;
    for (y, row) in buttons.chunks_exact(info.width as usize * 4).enumerate() {
        for (x, pixel) in row.chunks_exact(4).enumerate() {
            let at = (4 + y) * width + 8 + 21 * (x / 14) + x % 14;
            over(&mut pixels[at], pixel.try_into()?);
        }
    }
    // AppKit rounds the top corners by about 4.5 pixels, a textured window's bottom ones by
    // about 3.5.
    let inside = |distance: f32| (0.5 - distance).clamp(0.0, 1.0);
    for (index, pixel) in pixels.iter_mut().enumerate() {
        let [x, y] = [(index % width) as f32 + 0.5, (index / width) as f32 + 0.5];
        let radius = if y < total as f32 / 2.0 { 4.5 } else { 3.5 };
        let [dx, dy] = [
            (x - width as f32 / 2.0).abs() - (width as f32 / 2.0 - radius),
            (y - total as f32 / 2.0).abs() - (total as f32 / 2.0 - radius),
        ];
        let edge = dx.max(0.0).hypot(dy.max(0.0)) + dx.max(dy).min(0.0) - radius;
        pixel[3] *= inside(edge);
    }
    Ok(pixels)
}

/// The window over its shadow on a transparent canvas, and the canvas's size, as straight
/// RGBA bytes.
fn shadow(window: &[[f32; 4]], [width, height]: [usize; 2], key: bool) -> ([usize; 2], Vec<u8>) {
    let (margin, blur, fall, darkest) = SHADOW[usize::from(!key)];
    let canvas = [width + 2 * margin[0], height + margin[1] + margin[2]];
    let mut shade = vec![0.0f32; canvas[0] * canvas[1]];
    for y in 0..height {
        for x in 0..width {
            let [cx, cy] = [x + margin[0], y + margin[1] + fall];
            if cy < canvas[1] {
                shade[cy * canvas[0] + cx] = window[y * width + x][3] / 255.0;
            }
        }
    }
    // Three box blurs each way come close to a Gaussian.
    for _ in 0..3 {
        box_blur(&mut shade, canvas, blur, true);
        box_blur(&mut shade, canvas, blur, false);
    }
    let mut out = Vec::with_capacity(canvas[0] * canvas[1] * 4);
    for y in 0..canvas[1] {
        for x in 0..canvas[0] {
            let shadow = shade[y * canvas[0] + x] * darkest;
            let pixel = x
                .checked_sub(margin[0])
                .zip(y.checked_sub(margin[1]))
                .filter(|&(wx, wy)| wx < width && wy < height)
                .map_or([0.0; 4], |(wx, wy)| window[wy * width + wx]);
            let alpha = pixel[3] / 255.0;
            let total = alpha + shadow * (1.0 - alpha);
            let color = |channel: usize| {
                if total == 0.0 {
                    0
                } else {
                    (pixel[channel] * alpha / total).round() as u8
                }
            };
            out.extend([color(0), color(1), color(2), (total * 255.0).round() as u8]);
        }
    }
    (canvas, out)
}

fn box_blur(values: &mut [f32], [width, height]: [usize; 2], radius: usize, rows: bool) {
    let [lines, length] = if rows {
        [height, width]
    } else {
        [width, height]
    };
    let at = |line: usize, along: usize| {
        if rows {
            line * width + along
        } else {
            along * width + line
        }
    };
    let mut line_values = vec![0.0; length];
    for line in 0..lines {
        for (along, value) in line_values.iter_mut().enumerate() {
            *value = values[at(line, along)];
        }
        let mut sum: f32 = line_values[..radius.min(length)].iter().sum();
        for along in 0..length {
            if along + radius < length {
                sum += line_values[along + radius];
            }
            if along > radius {
                sum -= line_values[along - radius - 1];
            }
            values[at(line, along)] = sum / (2 * radius + 1) as f32;
        }
    }
}
