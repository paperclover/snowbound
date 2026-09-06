use canvas::gpu::{Primitive, Viewport};

pub struct Scroll {
    pub min: [f32; 2],
    pub max: [f32; 2],
}

impl Scroll {
    pub fn new(viewport: Viewport, bounds: impl Iterator<Item = [f32; 4]>) -> Self {
        let mut min = [0.0_f32; 2];
        let mut max = [0.0_f32; 2];
        for rect in bounds {
            for axis in 0..2 {
                min[axis] = min[axis].min(rect[axis]);
                max[axis] = max[axis].max(rect[axis + 2]);
            }
        }
        for axis in 0..2 {
            min[axis] = (min[axis] - 36.0) * viewport.scale;
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

    pub fn thumb(&self, viewport: Viewport, dpr: f32, axis: usize) -> Option<[f32; 4]> {
        let range = self.max[axis] - self.min[axis];
        let length = viewport.size[axis] as f32;
        let track = length - 20.0 * dpr;
        if range <= 0.0 || track <= 0.0 {
            return None;
        }
        let size = (track * length / (length + range))
            .max(28.0 * dpr)
            .min(track);
        let position = (-viewport.origin[axis] - self.min[axis]) / range;
        let start = 4.0 * dpr + position.clamp(0.0, 1.0) * (track - size);
        let cross = viewport.size[1 - axis] as f32 - 4.0 * dpr;
        Some(if axis == 0 {
            [start, cross - 6.0 * dpr, start + size, cross]
        } else {
            [cross - 6.0 * dpr, start, cross, start + size]
        })
    }

    pub fn drag(&self, viewport: &mut Viewport, dpr: f32, axis: usize, pointer: f32, grab: f32) {
        let Some(rect) = self.thumb(*viewport, dpr, axis) else {
            return;
        };
        let travel = viewport.size[axis] as f32 - 20.0 * dpr - (rect[axis + 2] - rect[axis]);
        if travel > 0.0 {
            let fraction = ((pointer - grab - 4.0 * dpr) / travel).clamp(0.0, 1.0);
            viewport.origin[axis] =
                -(self.min[axis] + fraction * (self.max[axis] - self.min[axis]));
        }
    }

    pub fn hit_test(&self, viewport: Viewport, dpr: f32, point: [f32; 2]) -> Option<(usize, f32)> {
        (0..2).find_map(|axis| {
            let rect = self.thumb(viewport, dpr, axis)?;
            let inset = 3.0 * dpr;
            ((rect[0] - inset..=rect[2] + inset).contains(&point[0])
                && (rect[1] - inset..=rect[3] + inset).contains(&point[1]))
            .then_some((axis, point[axis] - rect[axis]))
        })
    }

    pub fn append(&self, viewport: Viewport, dpr: f32, primitives: &mut Vec<Primitive<'_>>) {
        for axis in 0..2 {
            if let Some(rect) = self.thumb(viewport, dpr, axis) {
                let start = viewport.document_point([rect[0], rect[1]]);
                let end = viewport.document_point([rect[2], rect[3]]);
                primitives.push(Primitive::RoundedRect {
                    rect: [start[0], start[1], end[0], end[1]],
                    radius: [3.0 * dpr / viewport.scale; 2],
                    stroke: None,
                    color: [0.18, 0.18, 0.18, 0.45],
                });
            }
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
        let scroll = Scroll::new(viewport, [[-80.0, -20.0, 1000.0, 1200.0]].into_iter());
        scroll.clamp(&mut viewport);
        assert_eq!(viewport.origin, [232.0, 112.0]);
        viewport.origin = [-9999.0; 2];
        scroll.clamp(&mut viewport);
        assert_eq!(viewport.origin, [-1272.0, -1872.0]);
    }

    #[test]
    fn small_page_has_no_scrollbars_and_thumb_drag_reaches_endpoints() {
        for dpr in [1.0, 2.0] {
            let mut viewport = Viewport {
                size: [(800.0 * dpr) as u32, (600.0 * dpr) as u32],
                scale: dpr * 4.0 / 3.0,
                origin: [48.0 * dpr; 2],
            };
            let scroll = Scroll::new(viewport, [[0.0, 0.0, 72.0, 14.0]].into_iter());
            assert!(scroll.thumb(viewport, dpr, 0).is_none());
            assert!(scroll.thumb(viewport, dpr, 1).is_none());
            let scroll = Scroll::new(viewport, [[0.0, 0.0, 2000.0, 2000.0]].into_iter());
            for axis in 0..2 {
                scroll.drag(&mut viewport, dpr, axis, -1000.0, 3.0 * dpr);
                assert_eq!(-viewport.origin[axis], scroll.min[axis]);
                let rect = scroll.thumb(viewport, dpr, axis).unwrap();
                assert!((rect[axis] - 4.0 * dpr).abs() < 0.001);
                scroll.drag(&mut viewport, dpr, axis, 10000.0, 3.0 * dpr);
                assert_eq!(-viewport.origin[axis], scroll.max[axis]);
                let rect = scroll.thumb(viewport, dpr, axis).unwrap();
                assert!((rect[axis + 2] - (viewport.size[axis] as f32 - 16.0 * dpr)).abs() < 0.001);
            }
        }
    }
}
