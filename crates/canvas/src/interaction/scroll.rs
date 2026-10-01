use crate::gpu::Viewport;

/// The least and greatest scroll offsets on each axis, in device pixels: how far the
/// view's corner may sit from the page origin.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scroll {
    pub min: [f32; 2],
    pub max: [f32; 2],
}

impl Scroll {
    /// Each content rectangle comes with the room, in device pixels, OneNote leaves beyond
    /// it up and toward the page's start: left, or right on a right-to-left page.
    pub fn new(
        viewport: Viewport,
        bounds: impl Iterator<Item = ([f32; 4], f32)>,
        rtl: bool,
    ) -> Self {
        let mut min = [0.0_f32; 2];
        let mut max = [0.0_f32; 2];
        let mut end = f32::NEG_INFINITY;
        for (rect, pad) in bounds {
            for axis in 0..2 {
                min[axis] = min[axis].min(rect[axis] * viewport.scale - pad);
                max[axis] = max[axis].max(rect[axis + 2]);
            }
            end = end.max(rect[2] * viewport.scale + pad);
        }
        // OneNote stops at the page origin unless content reaches past it; a right-to-left
        // page ends where its content does.
        for axis in 0..2 {
            max[axis] = if rtl && axis == 0 && end.is_finite() {
                end - viewport.size[0] as f32
            } else {
                (max[axis] + 36.0) * viewport.scale - viewport.size[axis] as f32
            }
            .max(min[axis]);
        }
        Self { min, max }
    }

    pub(crate) fn clamp(&self, viewport: &mut Viewport) {
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
        let scroll = Scroll::new(
            viewport,
            [([-80.0, -20.0, 1000.0, 1200.0], 11.0)].into_iter(),
            false,
        );
        scroll.clamp(&mut viewport);
        assert_eq!(viewport.origin, [171.0, 51.0]);
        viewport.origin = [-9999.0; 2];
        scroll.clamp(&mut viewport);
        assert_eq!(viewport.origin, [-1272.0, -1872.0]);
    }

    #[test]
    fn pictures_reach_no_further_than_their_corner() {
        let viewport = Viewport {
            size: [800, 600],
            scale: 2.0,
            origin: [0.0; 2],
        };
        let bounds = [
            ([-0.5, -22.35, 613.0, 141.3], 0.0),
            ([36.0, 86.4, 400.0, 180.0], 11.0),
        ];
        let scroll = Scroll::new(viewport, bounds.into_iter(), false);
        assert_eq!(scroll.min, [-1.0, -44.7]);
    }

    #[test]
    fn right_to_left_pages_end_at_their_content() {
        let viewport = Viewport {
            size: [800, 600],
            scale: 2.0,
            origin: [0.0; 2],
        };
        let bounds = [
            ([10835.0, 14.4, 10962.0, 28.0], 11.0),
            ([11220.0, 84.0, 11334.0, 145.0], 0.0),
        ];
        let scroll = Scroll::new(viewport, bounds.into_iter(), true);
        assert_eq!(scroll.min, [0.0, 0.0]);
        assert_eq!(scroll.max[0], 11334.0 * 2.0 - 800.0);
    }
}
