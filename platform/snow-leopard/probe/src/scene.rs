//! One test scene, painted by `draw` over wgpu (the `reference` crate) and by the GL 2.1
//! painter on 10.6 (`render-probe`), whose PNGs `compare.py` diffs. Device pixels,
//! linear colours.

pub const SIZE: [u32; 2] = [400, 300];
pub const CLEAR: [f32; 4] = [0.85, 0.85, 0.85, 1.0];

pub enum Shape {
    Rect { rect: [f32; 4], color: [f32; 4] },
    /// `stroke` is a width and whether it is dashed.
    Rounded { rect: [f32; 4], radius: [f32; 2], stroke: Option<(f32, bool)>, colors: [[f32; 4]; 2] },
    Image { rect: [f32; 4] },
}

pub fn shapes() -> Vec<Shape> {
    let red = [0.6, 0.05, 0.05, 1.0];
    let blue = [0.05, 0.1, 0.6, 1.0];
    let green = [0.05, 0.4, 0.1, 1.0];
    let ink = [0.02, 0.02, 0.02, 1.0];
    use Shape::*;
    vec![
        Rect { rect: [20.0, 20.0, 120.0, 80.0], color: red },
        Rounded { rect: [140.0, 20.0, 260.0, 80.0], radius: [12.0, 12.0], stroke: None, colors: [blue; 2] },
        Rounded { rect: [280.0, 20.5, 380.0, 80.25], radius: [30.0, 12.0], stroke: None, colors: [green; 2] },
        Rounded { rect: [20.0, 100.0, 120.0, 160.0], radius: [10.0, 10.0], stroke: Some((3.0, false)), colors: [ink; 2] },
        Rounded { rect: [140.0, 100.0, 260.0, 160.0], radius: [16.0, 16.0], stroke: Some((2.0, true)), colors: [ink; 2] },
        Rounded { rect: [280.0, 100.0, 380.0, 160.0], radius: [8.0, 8.0], stroke: None, colors: [[0.9, 0.5, 0.1, 1.0], [0.2, 0.05, 0.4, 1.0]] },
        Image { rect: [20.0, 180.0, 120.0, 280.0] },
        Rect { rect: [60.0, 200.0, 200.0, 260.0], color: [0.1, 0.3, 0.8, 0.5] },
        Rounded { rect: [220.0, 180.0, 380.0, 280.0], radius: [30.0, 10.0], stroke: Some((3.0, true)), colors: [red; 2] },
    ]
}

/// A 4×4 opaque sRGB checker, stretched and filtered by the image path.
pub fn checker() -> ([u32; 2], Vec<u8>) {
    let pixels = (0..16)
        .flat_map(|i| if (i % 4 + i / 4) % 2 == 0 { [240, 200, 40, 255] } else { [30, 120, 90, 255] })
        .collect();
    ([4, 4], pixels)
}
