//! One scene painted by `draw` over wgpu on the dev Mac (`reference`) and over OpenGL on
//! 10.6 (`render-probe`), whose PNGs `reference --compare` diffs. Device pixels, linear
//! colours.
#![allow(dead_code)] // Each binary paints part of it.
use draw::{Layer, Primitive, RasterImage, Stroke};

pub const SIZE: [u32; 2] = [400, 300];
pub const CLEAR: [f32; 4] = [0.85, 0.85, 0.85, 1.0];

/// A 4×4 opaque sRGB checker, stretched and filtered by the image path.
pub fn checker() -> RasterImage {
    let pixels = (0..16)
        .flat_map(|i| if (i % 4 + i / 4) % 2 == 0 { [240, 200, 40, 255] } else { [30, 120, 90, 255] })
        .collect();
    RasterImage::new([4, 4], pixels).unwrap()
}

pub fn primitives(checker: &RasterImage) -> Vec<Primitive<'_>> {
    let red = [0.6, 0.05, 0.05, 1.0];
    let blue = [0.05, 0.1, 0.6, 1.0];
    let green = [0.05, 0.4, 0.1, 1.0];
    let ink = [0.02, 0.02, 0.02, 1.0];
    use Primitive::*;
    vec![
        Rect { rect: [20.0, 20.0, 120.0, 80.0], color: red },
        Gradient { rect: [140.0, 20.0, 260.0, 80.0], radius: [12.0, 12.0], colors: [blue; 2] },
        Gradient { rect: [280.0, 20.5, 380.0, 80.25], radius: [30.0, 12.0], colors: [green; 2] },
        RoundedRect { rect: [20.0, 100.0, 120.0, 160.0], radius: [10.0, 10.0], stroke: Some(Stroke::Solid(3.0)), color: ink },
        RoundedRect { rect: [140.0, 100.0, 260.0, 160.0], radius: [16.0, 16.0], stroke: Some(Stroke::Dashed(2.0)), color: ink },
        Gradient { rect: [280.0, 100.0, 380.0, 160.0], radius: [8.0, 8.0], colors: [[0.9, 0.5, 0.1, 1.0], [0.2, 0.05, 0.4, 1.0]] },
        Image { image: checker, rect: [20.0, 180.0, 120.0, 280.0] },
        Rect { rect: [60.0, 200.0, 200.0, 260.0], color: [0.1, 0.3, 0.8, 0.5] },
        RoundedRect { rect: [220.0, 180.0, 380.0, 280.0], radius: [30.0, 10.0], stroke: Some(Stroke::Dashed(3.0)), color: red },
    ]
}

pub fn layer<'a>(primitives: &'a [Primitive<'a>]) -> Layer<'a> {
    Layer { scale: 1.0, origin: [0.0; 2], clip: None, backdrop: None, round: None, motion: None, primitives }
}

/// Lines of interface text through `ui`: black on white like a Cocoa label, the same in
/// Arial, then the light theme's text on its strip. Logical pixels.
pub const TEXT_SIZE: [u32; 2] = [640, 150];
pub const TEXT: [&str; 2] = [
    "Garden Kitchen Journal Reading Trips Home",
    "Search All Notebooks  Up to date  100%  Calibri",
];

pub fn interface(scale: f32) -> ui::Ui {
    let theme = ui::Theme::light();
    let strip = theme.strip;
    let mut ui = ui::Ui::new(theme, std::time::Duration::from_millis(500));
    ui.begin(TEXT_SIZE.map(|side| side as f32), scale, std::time::Instant::now());
    ui.open("page", ui::Spec { size: [ui::fill(), ui::fill()], axis: ui::Axis::Y, fill: Some([1.0; 4]), pad: [10.0, 6.0], gap: 6.0, ..Default::default() });
    let black = Some([0.0, 0.0, 0.0, 1.0]);
    for (index, text) in TEXT.into_iter().enumerate() {
        ui.leaf(index, ui::Spec { size: [ui::fit(), ui::px(24.0)], text: Some(text), color: black, ..Default::default() });
    }
    ui.leaf("arial", ui::Spec { size: [ui::fit(), ui::px(24.0)], text: Some(TEXT[0]), font: Some("Arial"), color: black, ..Default::default() });
    ui.leaf("strip", ui::Spec { size: [ui::fill(), ui::px(24.0)], text: Some(TEXT[1]), fill: Some(strip), ..Default::default() });
    ui.close();
    ui.end();
    ui
}

pub fn interface_layers<'a>(layers: &'a [ui::Layer<'a>], scale: f32) -> Vec<Layer<'a>> {
    layers
        .iter()
        .map(|layer| match layer {
            ui::Layer::Primitives(primitives) => primitives.layer(scale),
            ui::Layer::Custom { .. } => unreachable!(),
        })
        .collect()
}
