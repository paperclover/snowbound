use crate::gpu::Viewport;

/// The least and greatest scroll offsets on each axis, in device pixels: how far the
/// view's corner may sit from the page origin.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scroll {
    pub min: [f32; 2],
    pub max: [f32; 2],
}

impl Scroll {
    /// `pad` is the room, in device pixels, OneNote leaves beyond content it scrolls to.
    pub fn new(viewport: Viewport, pad: f32, bounds: impl Iterator<Item = [f32; 4]>) -> Self {
        let mut min = [f32::INFINITY; 2];
        let mut max = [0.0_f32; 2];
        for rect in bounds {
            for axis in 0..2 {
                min[axis] = min[axis].min(rect[axis]);
                max[axis] = max[axis].max(rect[axis + 2]);
            }
        }
        // OneNote stops at the page origin, or `pad` beyond content that reaches past it.
        for axis in 0..2 {
            min[axis] = (min[axis] * viewport.scale - pad).min(0.0);
            max[axis] =
                ((max[axis] + 36.0) * viewport.scale - viewport.size[axis] as f32).max(min[axis]);
        }
        Self { min, max }
    }

    pub fn clamp(&self, viewport: &mut Viewport) {
        for axis in 0..2 {
            viewport.origin[axis] = -(-viewport.origin[axis]).clamp(self.min[axis], self.max[axis]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_include_negative_objects_and_clamp_both_ends() {
        let mut viewport = Viewport {
            size: [800, 600],
            scale: 2.0,
            origin: [9999.0; 2],
        };
        let scroll = Scroll::new(viewport, 11.0, [[-80.0, -20.0, 1000.0, 1200.0]].into_iter());
        scroll.clamp(&mut viewport);
        assert_eq!(viewport.origin, [171.0, 51.0]);
        viewport.origin = [-9999.0; 2];
        scroll.clamp(&mut viewport);
        assert_eq!(viewport.origin, [-1272.0, -1872.0]);
    }
}
