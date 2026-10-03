//! Controls a notebook window's chrome is built from: OneNote's section tabs and the
//! toolbar's compact buttons.

use draw::{PathStyle, edit::SelectionUnit};

use crate::{
    Anchor, Corner, Display, Extent, Flags, Id, Section, Shape, Signal, Spec, Ui, children, fill,
    mix, px,
};
use accesskit::{HasPopup, Role};
use std::hash::Hash;

/// A menu's downward arrow.
pub const CHEVRON: &[&str] = &[include_str!("../assets/chevron-down.svg")];
/// Rounding of a section tab's top leading corner; the trailing one takes half.
const TAB_ROUNDING: f32 = 4.0;
/// Side of a toolbar button.
pub const TOOL: f32 = 22.0;
/// Width of a menu arrow beside a toolbar button.
const ARROW: f32 = 10.0;
/// Width of a button showing only a menu arrow, which ends its group as tight as an icon does.
const MORE: f32 = 14.0;
/// Where a section tab's label starts inside it.
pub const TAB_PAD: f32 = 10.0;
/// Side of the dot marking something unread, as Mail marks unread mail.
pub const DOT: f32 = 6.0;

/// The dot marking something unread, in the accent, floating in the box being built at
/// `position`, its top left corner.
pub fn unread_dot(ui: &mut Ui, part: impl std::hash::Hash, position: [f32; 2]) {
    ui.leaf(
        part,
        Spec {
            flags: Flags::FLOAT,
            size: [px(DOT), px(DOT)],
            position,
            fill: Some(ui.theme.accent),
            radius: DOT / 2.0,
            ..Spec::default()
        },
    );
}
/// How far section tabs fade out at an end of their row they are cut at, and how near it a
/// dragged tab scrolls the row on.
const EDGE: f32 = 20.0;

/// Section tabs in a row `height` tall, as OneNote draws them: each leans over the next
/// at 45°, and the open one lies on top in `section`'s colours, rising to meet the frame
/// below as its outline and shadow fade in. `tabs` are names and section colours; `lit`
/// shows as hovered, as a tab something is dragged onto. A `dragged` tab follows the
/// pointer, lifted over the others, which slide aside to open the gap it would land in;
/// each eases to its place once let go, and near the row's ends the row scrolls on.
///
/// The row is as wide as the tabs, and gives up all of that where its own row lacks room.
/// Then the tabs scroll sideways under the wheel, either way it turns, fading out at the
/// ends they are cut at into the row's `fill`, and the open tab scrolls into view as it
/// opens. A tab given `true` after its colour shows the unread dot in its leading pad. The tab
/// `renaming` holds what its builder builds inside it, given the tab's height.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn section_tabs(
    ui: &mut Ui,
    row: Id,
    tabs: &[(&str, [f32; 4], bool)],
    active: usize,
    lit: Option<usize>,
    dragged: Option<Dragged>,
    mut renaming: Option<(usize, &mut dyn FnMut(&mut Ui, f32))>,
    section: &Section,
    height: f32,
    fill: [f32; 4],
) -> Tabs {
    let theme = ui.theme.clone();
    // Room for the first tab's outline and shadow, which the row would clip.
    let mut left = crate::SHADOW[0];
    let placed: Vec<_> = tabs
        .iter()
        .map(|&(name, ..)| {
            let label = ui
                .texts
                .styled(name, ui.theme.font_size, false, false, None, ui.frame);
            let width = TAB_PAD + label.size[0] + 4.0 + lean(height);
            left += width;
            (left - width, width)
        })
        .collect();
    // The last tab's slant and shadow reach past its box.
    let wide = left + lean(height) + crate::SHADOW[0];
    let view = ui.laid_out(row).map_or(wide, |rect| rect[2] - rect[0]);
    let most = (wide - view).max(0.0);
    let [scroll, shown, shift] = [row.child("scroll"), row.child("shown"), row.child("offset")];
    let held_value = |ui: &Ui, id: Id| ui.states.get(&id).and_then(|state| state.tween);
    let mut target = held_value(ui, scroll).map_or(0.0, |[_, target]| target);
    // The wheel moves the tabs at once from where they show, as on the page; only jumps ease.
    let mut wheeled = false;
    for event in ui.signal(row).events {
        if let crate::Event::Wheel([x, y]) = event {
            if !wheeled {
                target = held_value(ui, shift).map_or(target, |[value, _]| value);
            }
            target -= x + y;
            wheeled = true;
        }
    }
    if held_value(ui, shown).is_none_or(|[_, was]| was != active as f32)
        && let Some((x, width)) = placed.get(active)
    {
        target = target.min(*x).max(x + width + lean(height) - view);
    }
    ui.hold(shown, active as f32);
    // A tab dragged near either end scrolls the row on, as long as it is held there.
    let start = dragged.and_then(|dragged| {
        let start = dragged.start?;
        let width = placed[dragged.index].1;
        let push = (start - EDGE).min(0.0) + (start + width - view + EDGE).max(0.0);
        if push != 0.0 && (target > 0.0 || push > 0.0) && (target < most || push < 0.0) {
            target += push * ui.dt * 8.0;
            ui.animating = true;
        }
        Some(start)
    });
    let target = ui.hold(scroll, target.clamp(0.0, most));
    let offset = if wheeled {
        ui.hold(shift, target)
    } else {
        ui.animate(shift, target)
    };
    let fade = |cut: bool| if cut { EDGE } else { 0.0 };
    ui.open_as(
        row,
        Spec {
            flags: Flags::CLIP | Flags::SCROLL,
            size: [
                Extent {
                    size: crate::Size::Pixels(wide),
                    strictness: 0.0,
                },
                px(height),
            ],
            fill: Some(fill),
            fade: [fade(offset > 0.5), fade(offset < most - 0.5)],
            role: Some(Role::TabList),
            ..Spec::default()
        },
    );
    let [low, tallest] = [height - 6.0, height - 2.0];
    let (mut clicked, mut context, mut held, mut renamed) = (None, None, None, None);
    let spans: Vec<[f32; 2]> = placed.iter().map(|(x, width)| [*x, *width]).collect();
    // The dragged tab's place along the tabs, however far they are scrolled.
    let start = start.map(|start| start + offset);
    let slot = dragged.and_then(|dragged| {
        let width = placed[dragged.index].1;
        Some(crate::drop_slot(
            &spans,
            dragged.index,
            start? + width / 2.0,
        ))
    });
    let lifted = dragged.map(|dragged| dragged.index);
    let mut settled = true;
    // Earlier tabs lie over later ones; the open tab over them, and a dragged one over all.
    let order = (0..tabs.len())
        .rev()
        .filter(|index| *index != active && Some(*index) != lifted)
        .chain((active < tabs.len() && Some(active) != lifted).then_some(active))
        .chain(lifted);
    for index in order {
        let (name, color, unread) = tabs[index];
        let (x, width) = placed[index];
        // Keyed by name, so a tab eases from where it stood when the tabs are reordered.
        let key = ui.id(("x", name));
        let x = match (dragged, slot, start) {
            (_, _, Some(start)) if lifted == Some(index) => ui.hold(key, start),
            (Some(dragged), Some(slot), _) => ui.animate(
                key,
                x + crate::slide(index, dragged.index, slot, placed[dragged.index].1),
            ),
            _ => {
                let eased = ui.animate(key, x);
                settled &= lifted != Some(index) || (eased - x).abs() < 0.5;
                eased
            }
        };
        let lift = f32::from(u8::from(lifted == Some(index)));
        let open = ui.animate(
            ui.id(("open", name)),
            f32::from(u8::from(index == active || lifted == Some(index))),
        );
        // Whole device pixels, so a rising tab reuses its rasterized outlines.
        let tall = ((low + (tallest - low) * open) * ui.scale()).round() / ui.scale();
        let colors = theme.section(color);
        // A lifted tab rises in its own colour; only the open one takes the frame's.
        let top = if index == active { section } else { &colors };
        let fill = mix(colors.tab, top.frame[0], open);
        let fade = |color: [f32; 4], alpha: f32| [color[0], color[1], color[2], color[3] * alpha];
        let id = ui.open(
            ("tab", index),
            Spec {
                flags: Flags::CLICKABLE | Flags::FLOAT,
                size: [px(width), px(tall)],
                position: [x - offset, height - tall],
                text: Some(name),
                color: Some(mix(mix(theme.ink, fill, 0.2), theme.ink, open)),
                fill: Some(if lit == Some(index) {
                    mix(colors.frame[0], [1.0; 4], 0.08)
                } else {
                    mix(fill, [1.0; 4], 0.08)
                }),
                gradient: Some(fill),
                hover_fill: (index != active).then(|| mix(colors.frame[0], [1.0; 4], 0.08)),
                // A tab not open is outlined in its own hue, fainter than the open one's.
                border: Some(mix(mix(colors.tab, colors.edge, 0.55), top.edge, open)),
                shadow: (open > 0.0).then(|| fade([0.0, 0.0, 0.0, 0.35 + 0.15 * lift], open)),
                radius: TAB_ROUNDING,
                shape: Shape::Tab { lean: lean(height) },
                pad: [TAB_PAD, 0.0],
                role: Some(Role::Tab),
                ..Spec::default()
            },
        );
        if let Some((_, build)) = renaming.as_mut().filter(|(at, _)| *at == index) {
            build(ui, tall);
        }
        ui.close();
        let signal = ui.signal(id);
        if unread {
            let position = [
                x - offset + (TAB_PAD - DOT) / 2.0,
                height - tall + (tall - DOT) / 2.0,
            ];
            unread_dot(ui, ("unread", index), position);
        }
        if let Some(node) = ui.access(tab_id(row, index)) {
            node.set_selected(index == active);
        }
        if signal.clicked && index != active {
            clicked = Some(index);
        }
        if let Some(point) = signal.context {
            context = Some((index, point));
        }
        if signal.dragging {
            held = Some(index);
        }
        if signal.pressed && signal.unit != SelectionUnit::Grapheme {
            renamed = Some(index);
        }
    }
    let open_tab = ui.id(("tab", active));
    ui.close();
    Tabs {
        clicked,
        context,
        held,
        renamed,
        slot,
        settled,
        open: open_tab,
    }
}

/// A section tab being dragged: its index, and its leading edge along the row while it
/// follows the pointer; none once let go, as it eases into its place over the others.
#[derive(Clone, Copy, Debug)]
pub struct Dragged {
    pub index: usize,
    pub start: Option<f32>,
}

/// The id of tab `index` of the section tabs built as `row`.
pub fn tab_id(row: Id, index: usize) -> Id {
    row.child(("tab", index))
}

/// What the section tabs were asked this frame.
pub struct Tabs {
    pub clicked: Option<usize>,
    /// The tab whose context menu was asked for, and where.
    pub context: Option<(usize, [f32; 2])>,
    /// The tab held down, which a drag moves.
    pub held: Option<usize>,
    /// The tab pressed twice in a row, which OneNote renames in place.
    pub renamed: Option<usize>,
    /// Where the dragged tab would land, as an index in the tabs' new order.
    pub slot: Option<usize>,
    /// The dragged tab, let go, stands in its place.
    pub settled: bool,
    /// The open tab's id, for `tab_base`.
    pub open: Id,
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

/// A square button showing `icon` tinted by `tint`, a toggle lit while `on` is `Some(true)`
/// or a plain button where it is `None`; artwork in its own colours takes white. A tooltip
/// names it.
pub fn tool_button(
    ui: &mut Ui,
    part: impl Hash,
    icon: &'static [&'static str],
    tint: [f32; 4],
    on: Option<bool>,
) -> Signal {
    let hover = ui.theme.hover();
    let id = ui.open(
        part,
        Spec {
            flags: Flags::CLICKABLE,
            size: [px(TOOL), px(TOOL)],
            icon: Some(icon),
            color: Some(tint),
            fill: (on == Some(true)).then_some(hover).or(rest(&ui.theme)),
            hover_fill: Some(hover),
            radius: 4.0,
            center: true,
            role: Some(Role::Button),
            ..Spec::default()
        },
    );
    press_state(ui, id, on);
    ui.close();
    ui.signal(id)
}

/// A tool button's face at rest, where the theme gives it one.
fn rest(theme: &crate::Theme) -> Option<[f32; 4]> {
    (theme.tool[3] > 0.0).then_some(theme.tool)
}

/// Shows button `id` as a toggle, pressed or not, where `on` is given.
fn press_state(ui: &mut Ui, id: Id, on: Option<bool>) {
    if let Some(on) = on
        && let Some(node) = ui.access(id)
    {
        node.set_toggled(on.into());
    }
}

/// Marks control `id` as opening popup `menu`, and whether it is open.
fn opens(ui: &mut Ui, id: Id, menu: Id) {
    let open = ui.popup_open(menu);
    if let Some(node) = ui.access(id) {
        node.set_has_popup(HasPopup::Menu);
        node.set_expanded(open);
    }
}

/// `color` as a control that does not apply now shows it.
fn faded([red, green, blue, alpha]: [f32; 4]) -> [f32; 4] {
    [red, green, blue, alpha * 0.35]
}

/// A tool button, with a menu arrow where `menu`, whose command does not apply now: `icon`
/// tinted by `tint` shows faded, and it takes no clicks.
pub fn unavailable(
    ui: &mut Ui,
    part: impl Hash,
    icon: &'static [&'static str],
    tint: [f32; 4],
    menu: bool,
) {
    let arrow = faded(ui.theme.text_dim);
    let id = ui.open(
        part,
        Spec {
            size: [children(), px(TOOL)],
            role: Some(Role::Button),
            ..Spec::default()
        },
    );
    if let Some(node) = ui.access(id) {
        node.set_disabled();
        if menu {
            node.set_has_popup(HasPopup::Menu);
        }
    }
    ui.leaf(
        "icon",
        Spec {
            size: [px(TOOL), px(TOOL)],
            icon: Some(icon),
            color: Some(faded(tint)),
            center: true,
            ..Spec::default()
        },
    );
    if menu {
        ui.leaf(
            "arrow",
            Spec {
                size: [px(ARROW), px(TOOL)],
                icon: Some(CHEVRON),
                color: Some(arrow),
                center: true,
                ..Spec::default()
            },
        );
    }
    ui.close();
}

/// A button `name`d and showing `icon`, a toggle lit while `on` as `tool_button`'s, joined
/// to an arrow that opens popup `menu`, with the colour it applies as a bar under the icon
/// when given. Hovering the button fills it alone; hovering the arrow, or its menu being
/// open, outlines both as one control. The arrow is named "`name` Options", as Office names
/// its split buttons' arrows. Returns the button's signal.
#[allow(clippy::too_many_arguments)]
pub fn split_button(
    ui: &mut Ui,
    part: impl Hash,
    name: &str,
    icon: &'static [&'static str],
    bar: Option<[f32; 4]>,
    on: Option<bool>,
    menu: Id,
) -> Signal {
    const RADIUS: f32 = 4.0;
    let hover = ui.theme.hover();
    let fade =
        |alpha: f32| (alpha > 0.0).then_some([hover[0], hover[1], hover[2], hover[3] * alpha]);
    let split = ui.id(part);
    let [button, arrow] = [split.child("button"), split.child("menu")];
    let lit = ui.animate(
        split.child("lit"),
        f32::from(u8::from(on == Some(true) || ui.hover == Some(button))),
    );
    let outlined = ui.hover == Some(arrow) || ui.popup_open(menu);
    let ring = ui.animate(split.child("ring"), f32::from(u8::from(outlined)));
    ui.open_as(
        split,
        Spec {
            size: [children(), px(TOOL)],
            ..Spec::default()
        },
    );
    ui.open_as(
        button,
        Spec {
            flags: Flags::CLICKABLE | Flags::CLIP,
            size: [px(TOOL), px(TOOL)],
            role: Some(Role::Button),
            ..Spec::default()
        },
    );
    press_state(ui, button, on);
    if let Some(node) = ui.access(button) {
        node.set_label(name);
    }
    // Rounded past the clip, so the fill meets the arrow square.
    ui.leaf(
        "face",
        Spec {
            flags: Flags::FLOAT,
            size: [px(TOOL + RADIUS), px(TOOL)],
            fill: fade(lit).or(rest(&ui.theme)),
            radius: RADIUS,
            ..Spec::default()
        },
    );
    ui.open(
        "icon",
        Spec {
            size: [px(TOOL), px(TOOL)],
            icon: Some(icon),
            color: Some(ui.theme.text),
            center: true,
            ..Spec::default()
        },
    );
    if let Some(color) = bar {
        // The icon leaves its lowest three units for the bar.
        let top = (TOOL - 16.0) / 2.0;
        ui.mark([top, top + 13.0, top + 16.0, top + 16.0], color, 0.0);
    }
    ui.close();
    ui.close();
    ui.open_as(
        arrow,
        Spec {
            flags: Flags::CLICKABLE,
            size: [px(ARROW), px(TOOL)],
            icon: Some(CHEVRON),
            color: Some(ui.theme.text_dim),
            center: true,
            role: Some(Role::Button),
            ..Spec::default()
        },
    );
    opens(ui, arrow, menu);
    if let Some(node) = ui.access(arrow) {
        node.set_label(format!("{name} Options"));
    }
    ui.close();
    if ui.signal(arrow).pressed {
        ui.open_popup(menu);
    }
    ui.leaf(
        "ring",
        Spec {
            flags: Flags::FLOAT,
            size: [px(TOOL + ARROW), px(TOOL)],
            border: fade(ring),
            radius: RADIUS,
            ..Spec::default()
        },
    );
    ui.close();
    ui.signal(button)
}

/// A narrow button showing only a menu arrow, which opens popup `menu` of more of a group's
/// commands at the group's end; where not `enabled` it shows faded and takes no clicks.
pub fn more_button(ui: &mut Ui, part: impl Hash, menu: Id, enabled: bool) -> Anchor {
    let theme = ui.theme.clone();
    let open = ui.popup_open(menu);
    let id = ui.open(
        part,
        Spec {
            flags: if enabled {
                Flags::CLICKABLE
            } else {
                Flags::default()
            },
            size: [px(MORE), px(TOOL)],
            icon: Some(CHEVRON),
            color: Some(if enabled {
                theme.text_dim
            } else {
                faded(theme.text_dim)
            }),
            fill: open.then(|| theme.hover()).or(rest(&theme)),
            hover_fill: enabled.then(|| theme.hover()),
            radius: 4.0,
            center: true,
            role: Some(Role::Button),
            ..Spec::default()
        },
    );
    if let Some(node) = ui.access(id) {
        node.set_has_popup(HasPopup::Menu);
        node.set_expanded(open);
        if !enabled {
            node.set_disabled();
        }
    }
    ui.close();
    if enabled && ui.signal(id).pressed {
        ui.open_popup(menu);
    }
    Anchor::Below(id)
}

/// A button showing `icon` and a menu arrow, one control that opens popup `menu`, a toggle
/// lit while `on` as `tool_button`'s. Returns where the menu opens so its icons line up
/// under the button's.
pub fn menu_button(
    ui: &mut Ui,
    part: impl Hash,
    icon: &'static [&'static str],
    on: Option<bool>,
    menu: Id,
) -> Anchor {
    let theme = ui.theme.clone();
    let id = ui.open(
        part,
        Spec {
            flags: Flags::CLICKABLE,
            size: [px(TOOL + ARROW), px(TOOL)],
            fill: (ui.popup_open(menu) || on == Some(true))
                .then(|| theme.hover())
                .or(rest(&theme)),
            hover_fill: Some(theme.hover()),
            radius: 4.0,
            role: Some(Role::Button),
            ..Spec::default()
        },
    );
    opens(ui, id, menu);
    press_state(ui, id, on);
    for (part, icon, width, color) in [
        ("icon", icon, TOOL, theme.text),
        ("arrow", CHEVRON, ARROW, theme.text_dim),
    ] {
        ui.leaf(
            part,
            Spec {
                size: [px(width), px(TOOL)],
                icon: Some(icon),
                color: Some(color),
                center: true,
                ..Spec::default()
            },
        );
    }
    // The menu opens from the button shifted so its icons line up under the button's.
    let style = ui.theme.menu();
    let from = ui.id("menu");
    ui.leaf(
        "menu",
        Spec {
            flags: Flags::FLOAT,
            size: [px(TOOL + ARROW), px(TOOL)],
            position: [(TOOL - crate::ICON) / 2.0 - style.pad - style.row_pad, 0.0],
            ..Spec::default()
        },
    );
    ui.close();
    if ui.signal(id).pressed {
        ui.open_popup(menu);
    }
    Anchor::Below(from)
}

/// A drop-down box `name`d, `width` wide, showing `text`, that opens popup `menu`; where
/// not `enabled` it shows faded and takes no clicks or focus, as `unavailable` buttons do.
pub fn combo(
    ui: &mut Ui,
    part: impl Hash,
    name: &str,
    text: &str,
    width: impl Into<Extent>,
    menu: Id,
    enabled: bool,
) {
    let theme = ui.theme.clone();
    let faded = |color| if enabled { color } else { faded(color) };
    let id = ui.open(
        part,
        Spec {
            flags: if enabled {
                Flags::CLICKABLE
            } else {
                Flags::default()
            },
            size: [width.into(), px(TOOL)],
            fill: Some(theme.base),
            hover_border: enabled.then_some(theme.accent),
            border: Some(faded(theme.chip)),
            radius: 4.0,
            pad: [8.0, 0.0],
            role: Some(Role::ComboBox),
            ..Spec::default()
        },
    );
    opens(ui, id, menu);
    if let Some(node) = ui.access(id) {
        node.set_label(name);
        node.set_value(text);
        if !enabled {
            node.set_disabled();
        }
    }
    ui.leaf(
        "text",
        Spec {
            size: [fill(), px(TOOL)],
            text: Some(text),
            color: Some(faded(theme.text)),
            ..Spec::default()
        },
    );
    ui.leaf(
        "arrow",
        Spec {
            size: [px(12.0), px(TOOL)],
            icon: Some(CHEVRON),
            color: Some(faded(theme.text_dim)),
            center: true,
            ..Spec::default()
        },
    );
    ui.close();
    if enabled && ui.signal(id).pressed {
        ui.open_popup(menu);
    }
}

impl Ui {
    /// Paints `item` over the frame's boxes but under its popups.
    fn beneath_popups(&mut self, item: Display) {
        self.display.insert(self.popups_painted, item);
        self.popups_painted += 1;
    }

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
            self.beneath_popups(Display::Segment { from, to, color });
        }
        for corner in corners.into_iter().flatten() {
            let start = [
                corner.start[0] - corner.point[0],
                corner.start[1] - corner.point[1],
            ];
            self.beneath_popups(Display::Path {
                data: format!("M{} {}{}", start[0], start[1], corner.curve(corner.point)),
                origin: corner.point,
                style: PathStyle::Stroke(1.0),
                colors: [color; 2],
            });
        }
    }

    /// Rounds the corners of the clockwise outline `points`, as `border` does: paints
    /// `outside(y)` beyond each outward corner at height `y`, and `inside` into each
    /// inward one; a transparent colour cuts the corner out to the window's backdrop.
    /// Called between `end` and `layers`.
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
            let color = if corner.outward {
                outside(corner.point[1])
            } else {
                inside
            };
            let (style, color) = if color[3] == 0.0 {
                (PathStyle::Erase, [0.0, 0.0, 0.0, 1.0])
            } else {
                (PathStyle::Fill, color)
            };
            self.beneath_popups(Display::Path {
                data: format!(
                    "M{} {}{}L0 0Z",
                    start[0],
                    start[1],
                    corner.curve(corner.point)
                ),
                origin: corner.point,
                style,
                colors: [color; 2],
            });
        }
    }
}
