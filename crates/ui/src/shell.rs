//! Controls a notebook window's chrome is built from: OneNote's section tabs and the
//! toolbar's compact buttons.

use draw::PathStyle;

use crate::{
    Corner, Display, Flags, Id, Section, Shape, Signal, Spec, Ui, children, fill, mix, px,
};
use std::hash::Hash;

/// A menu's downward arrow.
pub const CHEVRON: &[&str] = &[include_str!("../assets/chevron-down.svg")];
/// Rounding of a section tab's top leading corner; the trailing one takes half.
const TAB_ROUNDING: f32 = 4.0;
/// Side of a toolbar button.
pub const TOOL: f32 = 22.0;

/// Section tabs in a row `height` tall, as OneNote draws them: each leans over the next
/// at 45°, and the open one lies on top in `section`'s colours, rising to meet the frame
/// below as its outline and shadow fade in. `tabs` are names and section colours. Returns
/// the tab clicked and the open tab's id, for `tab_base`.
pub fn section_tabs(
    ui: &mut Ui,
    part: impl Hash,
    tabs: &[(&str, [f32; 4])],
    active: usize,
    section: &Section,
    height: f32,
) -> (Option<usize>, Id) {
    let theme = ui.theme.clone();
    ui.open(
        part,
        Spec {
            flags: Flags::CLIP,
            size: [fill(), px(height)],
            ..Spec::default()
        },
    );
    let [low, tallest] = [height - 6.0, height - 2.0];
    let mut left = 0.0;
    let placed: Vec<_> = tabs
        .iter()
        .map(|(name, _)| {
            let width = 10.0 + ui.measure(name)[0] + 4.0 + lean(height);
            left += width;
            (left - width, width)
        })
        .collect();
    let mut clicked = None;
    // Earlier tabs lie over later ones; the open tab over all.
    let order = (0..tabs.len())
        .rev()
        .filter(|index| *index != active)
        .chain((active < tabs.len()).then_some(active));
    for index in order {
        let (name, color) = tabs[index];
        let (x, width) = placed[index];
        let open = ui.animate(ui.id(("open", index)), f32::from(u8::from(index == active)));
        // Whole device pixels, so a rising tab reuses its rasterized outlines.
        let tall = ((low + (tallest - low) * open) * ui.scale()).round() / ui.scale();
        let colors = theme.section(color);
        let fill = mix(colors.tab, section.frame[0], open);
        let fade = |color: [f32; 4], alpha: f32| [color[0], color[1], color[2], color[3] * alpha];
        let signal = ui.leaf(
            ("tab", index),
            Spec {
                flags: Flags::CLICKABLE | Flags::FLOAT,
                size: [px(width), px(tall)],
                position: [x, height - tall],
                text: Some(name),
                color: Some(mix(mix(theme.ink, fill, 0.2), theme.ink, open)),
                fill: Some(mix(fill, [1.0; 4], 0.08)),
                gradient: Some(fill),
                hover_fill: (index != active).then(|| mix(colors.frame[0], [1.0; 4], 0.08)),
                border: Some(fade(section.edge, open)),
                shadow: (open > 0.0).then(|| fade([0.0, 0.0, 0.0, 0.35], open)),
                radius: TAB_ROUNDING,
                shape: Shape::Tab { lean: lean(height) },
                pad: [10.0, 0.0],
                ..Spec::default()
            },
        );
        if signal.clicked && index != active {
            clicked = Some(index);
        }
    }
    let open_tab = ui.id(("tab", active));
    ui.close();
    (clicked, open_tab)
}

/// Where the tab laid out at `tab` in a row `height` tall meets the edge below it: from its
/// leading edge to the foot of its slant.
pub fn tab_base(tab: [f32; 4], height: f32) -> [f32; 2] {
    [tab[0], tab[2] - lean(height) + (tab[3] - tab[1])]
}

/// How far inside a tab's box its slant starts: half the open tab's height.
fn lean(height: f32) -> f32 {
    (height - 2.0) / 2.0
}

/// A square button showing `icon` tinted by `tint`; artwork in its own colours takes white.
pub fn tool_button(
    ui: &mut Ui,
    part: impl Hash,
    icon: &'static [&'static str],
    tint: [f32; 4],
) -> Signal {
    let hover = ui.theme.hover();
    ui.leaf(
        part,
        Spec {
            flags: Flags::CLICKABLE,
            size: [px(TOOL), px(TOOL)],
            icon: Some(icon),
            color: Some(tint),
            hover_fill: Some(hover),
            radius: 4.0,
            center: true,
            ..Spec::default()
        },
    )
}

/// A button showing `icon` with a menu arrow beside it, and optionally the colour it
/// applies as a bar under the icon. Returns the button's and the arrow's signals.
pub fn split_button(
    ui: &mut Ui,
    part: impl Hash,
    icon: &'static [&'static str],
    bar: Option<[f32; 4]>,
) -> [Signal; 2] {
    let theme = ui.theme.clone();
    ui.open(
        part,
        Spec {
            size: [children(), px(TOOL)],
            ..Spec::default()
        },
    );
    let button = ui.open(
        "button",
        Spec {
            flags: Flags::CLICKABLE,
            size: [px(TOOL), px(TOOL)],
            icon: Some(icon),
            color: Some(theme.text),
            hover_fill: Some(theme.hover()),
            radius: 4.0,
            center: true,
            ..Spec::default()
        },
    );
    if let Some(color) = bar {
        // The icon leaves its lowest three units for the bar.
        let top = (TOOL - 16.0) / 2.0;
        ui.mark([top, top + 13.0, top + 16.0, top + 16.0], color);
    }
    ui.close();
    let menu = ui.leaf(
        "menu",
        Spec {
            flags: Flags::CLICKABLE,
            size: [px(10.0), px(TOOL)],
            icon: Some(CHEVRON),
            color: Some(theme.text_dim),
            hover_fill: Some(theme.hover()),
            radius: 4.0,
            center: true,
            ..Spec::default()
        },
    );
    ui.close();
    [ui.signal(button), menu]
}

/// A drop-down box `width` wide showing `text`.
pub fn combo(ui: &mut Ui, part: impl Hash, text: &str, width: f32) -> Signal {
    let theme = ui.theme.clone();
    let id = ui.open(
        part,
        Spec {
            flags: Flags::CLICKABLE,
            size: [px(width), px(TOOL)],
            fill: Some(theme.base),
            hover_border: Some(theme.accent),
            border: Some(theme.chip),
            radius: 4.0,
            pad: [8.0, 0.0],
            ..Spec::default()
        },
    );
    ui.leaf(
        "text",
        Spec {
            size: [fill(), px(TOOL)],
            text: Some(text),
            ..Spec::default()
        },
    );
    ui.leaf(
        "arrow",
        Spec {
            size: [px(12.0), px(TOOL)],
            icon: Some(CHEVRON),
            color: Some(theme.text_dim),
            center: true,
            ..Spec::default()
        },
    );
    ui.close();
    ui.signal(id)
}

impl Ui {
    /// Draws a 1 px border along `points`, each with the radius its corner rounds by,
    /// joining the last back to the first when `closed`. Called between `end` and
    /// `layers`, it paints over the frame's boxes where they were just laid out.
    pub fn border(&mut self, points: &[([f32; 2], f32)], closed: bool, color: [f32; 4]) {
        let count = points.len();
        let corners: Vec<_> = (0..count)
            .map(|index| {
                let open_end = !closed && (index == 0 || index == count - 1);
                (!open_end && points[index].1 > 0.0).then(|| Corner::new(points, index))
            })
            .collect();
        for index in 0..count - usize::from(!closed) {
            let next = (index + 1) % count;
            let from = corners[index]
                .as_ref()
                .map_or(points[index].0, |corner| corner.end);
            let to = corners[next]
                .as_ref()
                .map_or(points[next].0, |corner| corner.start);
            self.display.push(Display::Segment { from, to, color });
        }
        for corner in corners.into_iter().flatten() {
            let start = [
                corner.start[0] - corner.point[0],
                corner.start[1] - corner.point[1],
            ];
            self.display.push(Display::Path {
                data: format!("M{} {}{}", start[0], start[1], corner.curve(corner.point)),
                origin: corner.point,
                style: PathStyle::Stroke(1.0),
                colors: [color; 2],
            });
        }
    }

    /// Rounds the corners of the clockwise outline `points`, as `border` does: paints
    /// `outside(y)` beyond each outward corner at height `y`, and `inside` into each
    /// inward one. Called between `end` and `layers`.
    pub fn round_corners(
        &mut self,
        points: &[([f32; 2], f32)],
        inside: [f32; 4],
        outside: impl Fn(f32) -> [f32; 4],
    ) {
        for index in 0..points.len() {
            if points[index].1 <= 0.0 {
                continue;
            }
            let corner = Corner::new(points, index);
            let start = [
                corner.start[0] - corner.point[0],
                corner.start[1] - corner.point[1],
            ];
            self.display.push(Display::Path {
                data: format!(
                    "M{} {}{}L0 0Z",
                    start[0],
                    start[1],
                    corner.curve(corner.point)
                ),
                origin: corner.point,
                style: PathStyle::Fill,
                colors: [if corner.outward {
                    outside(corner.point[1])
                } else {
                    inside
                }; 2],
            });
        }
    }
}
