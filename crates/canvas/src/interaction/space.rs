//! Insert Space's feedback, as OneNote 2010 draws it: lines across the view where the drag
//! began and where it is, an arrow between them in the middle of the view, and a tint where
//! what moves lands.

use crate::gpu::colorref;
use draw::Primitive;

/// Appends the feedback for lines at `from` and `to` along `axis` (0 for x), with `moved` the
/// extents of what moves, over `view`, the page's visible rectangle; `pixel` is page units
/// per pixel.
pub(super) fn append(
    axis: usize,
    [from, to]: [f32; 2],
    moved: &[[f32; 4]],
    view: [f32; 4],
    pixel: f32,
    primitives: &mut Vec<Primitive<'_>>,
) {
    let across = 1 - axis;
    let rect = |along: [f32; 2], side: [f32; 2]| {
        let [a0, a1] = [along[0].min(along[1]), along[0].max(along[1])];
        let [s0, s1] = side;
        if axis == 1 {
            [s0, a0, s1, a1]
        } else {
            [a0, s0, a1, s1]
        }
    };
    let point = |along: f32, side: f32| {
        let mut point = [side; 2];
        point[axis] = along;
        point
    };
    let delta = to - from;
    let mut tint = colorref(0x00f7e4cc);
    tint[3] = 0.6;
    for extent in moved {
        let mut extent = *extent;
        extent[axis] += delta;
        extent[axis + 2] += delta;
        primitives.push(Primitive::Rect {
            rect: extent,
            color: tint,
        });
    }
    if delta.abs() >= pixel {
        let (fill, edge) = (colorref(0x00fff2e5), colorref(0x00e8a460));
        let middle = (view[across] + view[across + 2]) / 2.0;
        let [shaft, head] = [36.5 * pixel, 73.0 * pixel];
        let length = (31.0 * pixel).min(delta.abs());
        let base = to - length * delta.signum();
        primitives.push(Primitive::Rect {
            rect: rect([from, base], [middle - shaft, middle + shaft]),
            color: fill,
        });
        let rows = (length / pixel).ceil().max(1.0);
        for row in 0..rows as u32 {
            let row = row as f32;
            let half = head * (1.0 - (row + 0.5) / rows);
            let near = base + delta.signum() * row * pixel;
            let far = base + delta.signum() * ((row + 1.0) * pixel).min(length);
            primitives.push(Primitive::Rect {
                rect: rect([near, far], [middle - half, middle + half]),
                color: fill,
            });
        }
        let segments = [
            (point(from, middle - shaft), point(base, middle - shaft)),
            (point(from, middle + shaft), point(base, middle + shaft)),
            (point(base, middle - shaft), point(base, middle - head)),
            (point(base, middle + shaft), point(base, middle + head)),
            (point(base, middle - head), point(to, middle)),
            (point(base, middle + head), point(to, middle)),
        ];
        for (from, to) in segments {
            primitives.push(Primitive::Segment {
                from,
                to,
                width: pixel,
                round: false,
                color: edge,
            });
        }
    }
    for at in [from, to] {
        primitives.push(Primitive::Rect {
            rect: rect(
                [at - pixel / 2.0, at + pixel / 2.0],
                [view[across], view[across + 2]],
            ),
            color: colorref(0x00df862d),
        });
    }
}
