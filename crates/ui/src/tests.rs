use super::*;
use std::time::Duration;
use winit::keyboard::NamedKey;

const DOUBLE_CLICK: Duration = Duration::from_millis(500);
/// The key that carries shortcuts: Command on macOS, Control elsewhere.
const SHORTCUT: ModifiersState = if cfg!(target_vendor = "apple") {
    ModifiersState::SUPER
} else {
    ModifiersState::CONTROL
};

thread_local! {
    static START: Instant = Instant::now();
    static FRAMES: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// Builds a frame in a 400 by 300 window, 16 ms after this thread's previous one.
fn frame(ui: &mut Ui, build: impl FnOnce(&mut Ui)) {
    sized_frame(ui, [400.0, 300.0], build);
}

/// Builds a frame in a window `size` large, 16 ms after this thread's previous one.
fn sized_frame(ui: &mut Ui, size: [f32; 2], build: impl FnOnce(&mut Ui)) {
    let frames = FRAMES.with(|frames| {
        frames.set(frames.get() + 1);
        frames.get()
    });
    let now = START.with(|start| *start) + Duration::from_millis(16) * frames;
    ui.begin(size, 2.0, now);
    build(ui);
    ui.end();
}

#[test]
fn fill_yields_to_strict_siblings_along_the_flow() {
    let mut ui = Ui::new(Theme::dark(), DOUBLE_CLICK);
    let mut ids = Vec::new();
    frame(&mut ui, |ui| {
        ui.open(
            "row",
            Spec {
                size: [fill(), px(100.0)],
                ..Spec::default()
            },
        );
        ids.push(ui.open(
            "page",
            Spec {
                size: [fill(), fill()],
                ..Spec::default()
            },
        ));
        ui.close();
        ids.push(ui.open(
            "list",
            Spec {
                size: [px(120.0), fill()],
                ..Spec::default()
            },
        ));
        ui.close();
        ui.close();
    });
    assert_eq!(ui.rect(ids[0]), Some([0.0, 0.0, 280.0, 100.0]));
    assert_eq!(ui.rect(ids[1]), Some([280.0, 0.0, 400.0, 100.0]));
}

#[test]
fn padding_wider_than_its_box_leaves_loose_children_empty_not_negative() {
    let mut ui = Ui::new(Theme::dark(), DOUBLE_CLICK);
    let mut child = None;
    frame(&mut ui, |ui| {
        ui.open(
            "cramped",
            Spec {
                size: [px(10.0), px(10.0)],
                pad: [8.0, 8.0],
                ..Spec::default()
            },
        );
        let loose = Extent {
            size: Size::Pixels(20.0),
            strictness: 0.0,
        };
        child = Some(ui.open(
            "child",
            Spec {
                size: [loose; 2],
                ..Spec::default()
            },
        ));
        ui.close();
        ui.close();
    });
    assert_eq!(ui.rect(child.unwrap()), Some([8.0, 8.0, 8.0, 8.0]));
}

#[test]
fn children_sum_with_gaps_and_padding() {
    let mut ui = Ui::new(Theme::dark(), DOUBLE_CLICK);
    let mut column = None;
    let mut second = None;
    frame(&mut ui, |ui| {
        column = Some(ui.open(
            "column",
            Spec {
                axis: Axis::Y,
                pad: [4.0, 6.0],
                gap: 2.0,
                ..Spec::default()
            },
        ));
        ui.leaf(
            1,
            Spec {
                size: [px(50.0), px(10.0)],
                ..Spec::default()
            },
        );
        second = Some(ui.open(
            2,
            Spec {
                size: [px(30.0), px(20.0)],
                ..Spec::default()
            },
        ));
        ui.close();
        ui.close();
    });
    assert_eq!(ui.rect(column.unwrap()), Some([0.0, 0.0, 58.0, 44.0]));
    assert_eq!(ui.rect(second.unwrap()), Some([4.0, 18.0, 34.0, 38.0]));
}

/// A row `width` wide of a space, a box that yields all but a quarter of its 80 pixels, and
/// groups 100 wide or 20 folded at each priority; returns the rectangles of the loose box and
/// of each group's full and folded button.
fn folding_row(ui: &mut Ui, width: f32, priorities: &[u32]) -> Vec<Option<[f32; 4]>> {
    let mut ids = Vec::new();
    frame(ui, |ui| {
        ui.open(
            "row",
            Spec {
                size: [px(width), px(20.0)],
                ..Spec::default()
            },
        );
        ui.leaf(
            "space",
            Spec {
                size: [fill(), px(1.0)],
                ..Spec::default()
            },
        );
        let loose = Extent {
            size: Size::Pixels(80.0),
            strictness: 0.25,
        };
        ids.push(ui.open(
            "loose",
            Spec {
                size: [loose, px(20.0)],
                ..Spec::default()
            },
        ));
        ui.close();
        for priority in priorities {
            let group = ui.open(
                priority,
                Spec {
                    fold: Some(*priority),
                    ..Spec::default()
                },
            );
            for (form, width) in [("full", 100.0), ("folded", 20.0)] {
                ui.open(form, Spec::default());
                ui.leaf(
                    "button",
                    Spec {
                        flags: Flags::CLICKABLE,
                        size: [px(width), px(20.0)],
                        ..Spec::default()
                    },
                );
                ui.close();
            }
            ui.close();
            for form in ["full", "folded"] {
                ids.push(group.child(form).child("button"));
            }
        }
        ui.close();
    });
    ids.into_iter().map(|id| ui.rect(id)).collect()
}

#[test]
fn a_row_folds_its_lowest_priorities_first_until_it_fits() {
    let mut ui = Ui::new(Theme::dark(), DOUBLE_CLICK);
    // 80 + 3 × 100 fits 380 with nothing folded or squeezed.
    let rects = folding_row(&mut ui, 380.0, &[2, 0, 1]);
    assert_eq!(rects[0], Some([0.0, 0.0, 80.0, 20.0]));
    assert_eq!(rects[1], Some([80.0, 0.0, 180.0, 20.0]));
    // At 300, folding priority 0 alone leaves 80 + 100 + 20 + 100; the full form's button
    // stands where the folded one shows.
    let rects = folding_row(&mut ui, 300.0, &[2, 0, 1]);
    assert_eq!(rects[1], Some([80.0, 0.0, 180.0, 20.0]));
    assert_eq!(rects[3..5], [Some([180.0, 0.0, 200.0, 20.0]); 2]);
    assert_eq!(rects[5], Some([200.0, 0.0, 300.0, 20.0]));
}

#[test]
fn a_row_squeezes_its_boxes_only_once_every_group_is_folded() {
    let mut ui = Ui::new(Theme::dark(), DOUBLE_CLICK);
    let rects = folding_row(&mut ui, 130.0, &[0, 1]);
    // 80 + 20 + 20 fits 130 with 10 to spare, taken by the space.
    assert_eq!(rects[0], Some([10.0, 0.0, 90.0, 20.0]));
    assert_eq!(rects[4], Some([110.0, 0.0, 130.0, 20.0]));
    let rects = folding_row(&mut ui, 100.0, &[0, 1]);
    assert_eq!(rects[0], Some([0.0, 0.0, 60.0, 20.0]));
    assert_eq!(rects[4], Some([80.0, 0.0, 100.0, 20.0]));
}

#[test]
fn a_row_is_narrowest_with_every_group_folded_and_every_box_squeezed() {
    let mut ui = Ui::new(Theme::dark(), DOUBLE_CLICK);
    folding_row(&mut ui, 300.0, &[0, 1]);
    let row = Id::ROOT.child("row");
    // The space gives all, the loose box 60 of 80, each group 80 of 100.
    assert_eq!(ui.narrowest(row), Some(60.0));
    let rects = folding_row(&mut ui, 60.0, &[0, 1]);
    assert_eq!(rects[4], Some([40.0, 0.0, 60.0, 20.0]));
}

#[test]
fn a_group_inside_a_box_sized_by_its_children_folds_with_the_row() {
    let mut ui = Ui::new(Theme::dark(), DOUBLE_CLICK);
    let build = |ui: &mut Ui, width: f32| {
        frame(ui, |ui| {
            ui.open(
                "row",
                Spec {
                    size: [px(width), px(20.0)],
                    ..Spec::default()
                },
            );
            ui.open("outer", Spec::default());
            ui.leaf(
                "fixed",
                Spec {
                    size: [px(50.0), px(20.0)],
                    ..Spec::default()
                },
            );
            ui.open(
                "group",
                Spec {
                    fold: Some(0),
                    ..Spec::default()
                },
            );
            for (form, width) in [("full", 100.0), ("folded", 20.0)] {
                ui.open(form, Spec::default());
                ui.leaf(
                    "button",
                    Spec {
                        size: [px(width), px(20.0)],
                        ..Spec::default()
                    },
                );
                ui.close();
            }
            ui.close();
            ui.close();
            ui.open(
                "later",
                Spec {
                    fold: Some(1),
                    ..Spec::default()
                },
            );
            for (form, width) in [("full", 30.0), ("folded", 30.0)] {
                ui.open(form, Spec::default());
                ui.leaf(
                    "button",
                    Spec {
                        size: [px(width), px(20.0)],
                        ..Spec::default()
                    },
                );
                ui.close();
            }
            ui.close();
            ui.close();
        });
    };
    let [outer, after] = ["outer", "later"].map(|part| Id::ROOT.child("row").child(part));
    build(&mut ui, 180.0);
    assert_eq!(ui.rect(outer), Some([0.0, 0.0, 150.0, 20.0]));
    // 50 + 100 + 30 overflows 179, so the group folds before the later one of higher priority,
    // and the box around it narrows to 70.
    build(&mut ui, 179.0);
    assert_eq!(ui.rect(outer), Some([0.0, 0.0, 70.0, 20.0]));
    assert_eq!(ui.rect(after), Some([70.0, 0.0, 100.0, 20.0]));
    assert_eq!(ui.narrowest(Id::ROOT.child("row")), Some(100.0));
}

#[test]
fn a_box_sized_by_its_children_yields_what_they_yield() {
    let mut ui = Ui::new(Theme::dark(), DOUBLE_CLICK);
    let mut combo = None;
    frame(&mut ui, |ui| {
        ui.open(
            "row",
            Spec {
                size: [px(100.0), px(20.0)],
                ..Spec::default()
            },
        );
        ui.open("group", Spec::default());
        let loose = Extent {
            size: Size::Pixels(80.0),
            strictness: 0.5,
        };
        combo = Some(ui.open(
            "combo",
            Spec {
                size: [loose, px(20.0)],
                ..Spec::default()
            },
        ));
        ui.close();
        ui.leaf(
            "button",
            Spec {
                size: [px(40.0), px(20.0)],
                ..Spec::default()
            },
        );
        ui.close();
        ui.close();
    });
    assert_eq!(ui.rect(combo.unwrap()), Some([0.0, 0.0, 60.0, 20.0]));
}

#[test]
fn a_folded_away_form_takes_no_input() {
    let mut ui = Ui::new(Theme::dark(), DOUBLE_CLICK);
    folding_row(&mut ui, 110.0, &[0]);
    let folded = Id::ROOT
        .child("row")
        .child(0_u32)
        .child("folded")
        .child("button");
    assert_eq!(ui.box_at([90.0, 10.0]), Some(folded));
    // Where the full form's button would lie past the folded one.
    assert_eq!(ui.box_at([150.0, 10.0]), None);
}

fn button_frame(ui: &mut Ui) -> Signal {
    let mut signal = Signal::default();
    frame(ui, |ui| {
        signal = ui.leaf(
            "button",
            Spec {
                flags: Flags::CLICKABLE,
                size: [px(100.0), px(40.0)],
                hover_fill: Some([1.0; 4]),
                ..Spec::default()
            },
        );
    });
    signal
}

#[test]
fn clicks_use_the_previous_layout_and_hover_animates() {
    let mut ui = Ui::new(Theme::dark(), DOUBLE_CLICK);
    button_frame(&mut ui);
    let at = Instant::now();
    ui.event(Event::PointerMoved([10.0, 10.0]));
    ui.event(Event::Button {
        button: MouseButton::Left,
        pressed: true,
        at,
    });
    let signal = button_frame(&mut ui);
    assert!(signal.hovered && signal.pressed && signal.dragging && !signal.clicked);
    assert!(ui.wants_frame(), "hover fades in");
    ui.event(Event::Button {
        button: MouseButton::Left,
        pressed: false,
        at,
    });
    ui.event(Event::Button {
        button: MouseButton::Left,
        pressed: true,
        at: at + Duration::from_millis(100),
    });
    let signal = button_frame(&mut ui);
    assert!(signal.clicked && signal.pressed);
    button_frame(&mut ui);
    ui.event(Event::PointerMoved([200.0, 10.0]));
    ui.event(Event::Button {
        button: MouseButton::Left,
        pressed: false,
        at,
    });
    let signal = button_frame(&mut ui);
    assert!(!signal.clicked && !signal.hovered && !signal.dragging);
    assert!(ui.wants_frame(), "hover fades out");
    for _ in 0..40 {
        button_frame(&mut ui);
    }
    assert!(!ui.wants_frame());
}

/// Two boxes swapping places under a still pointer, as dropped tabs reorder, pass the hover
/// to the one that arrives.
#[test]
fn a_box_moving_under_a_still_pointer_takes_the_hover() {
    let build = |ui: &mut Ui, swapped: bool| {
        let mut hovered = [false; 2];
        frame(ui, |ui| {
            for (index, hover) in hovered.iter_mut().enumerate() {
                let x = if (index == 0) != swapped { 0.0 } else { 100.0 };
                *hover = ui
                    .leaf(
                        index,
                        Spec {
                            flags: Flags::CLICKABLE | Flags::FLOAT,
                            size: [px(50.0), px(20.0)],
                            position: [x, 0.0],
                            ..Spec::default()
                        },
                    )
                    .hovered;
            }
        });
        hovered
    };
    let mut ui = Ui::new(Theme::dark(), DOUBLE_CLICK);
    build(&mut ui, false);
    ui.event(Event::PointerMoved([10.0, 10.0]));
    assert_eq!(build(&mut ui, false), [true, false]);
    build(&mut ui, true);
    assert_eq!(build(&mut ui, true), [false, true]);
}

/// A click whose press and release arrive together starts an animation that a box built
/// earlier in the frame reads: the frame that handled the click asks for the next one.
#[test]
fn a_click_within_one_frame_starts_an_animation_read_before_it() {
    let mut ui = Ui::new(Theme::dark(), DOUBLE_CLICK);
    let mut open = false;
    let mut width = 0.0;
    let build = |ui: &mut Ui, open: &mut bool, width: &mut f32| {
        *width = ui.animate(Id::ROOT.child("panel"), if *open { 100.0 } else { 0.0 });
        if button(ui, "toggle", "Toggle").clicked {
            *open = true;
        }
    };
    frame(&mut ui, |ui| build(ui, &mut open, &mut width));
    // The pointer rests on the button until its hover has faded in.
    ui.event(Event::PointerMoved([5.0, 5.0]));
    for _ in 0..40 {
        frame(&mut ui, |ui| build(ui, &mut open, &mut width));
    }
    assert!(!ui.wants_frame());
    let at = Instant::now();
    for pressed in [true, false] {
        ui.event(Event::Button {
            button: MouseButton::Left,
            pressed,
            at,
        });
    }
    frame(&mut ui, |ui| build(ui, &mut open, &mut width));
    assert!(open && width == 0.0);
    assert!(ui.wants_frame(), "the click's frame asks for another");
    frame(&mut ui, |ui| build(ui, &mut open, &mut width));
    frame(&mut ui, |ui| build(ui, &mut open, &mut width));
    assert!(width > 0.0 && ui.wants_frame(), "the panel eases open");
}

#[test]
fn hover_fades_its_own_colour_in_over_a_box_without_a_fill() {
    let mut ui = Ui::new(Theme::dark(), DOUBLE_CLICK);
    button_frame(&mut ui);
    ui.event(Event::PointerMoved([10.0, 10.0]));
    button_frame(&mut ui);
    let fill = ui.display.iter().find_map(|item| match item {
        Display::Rect { fill, .. } => Some(*fill),
        _ => None,
    });
    let [red, green, blue, alpha] = fill.expect("the hovered button paints");
    assert_eq!(
        [red, green, blue],
        [1.0; 3],
        "no darker colour on the way in"
    );
    assert!(alpha > 0.0 && alpha < 1.0);
}

#[test]
fn custom_boxes_receive_their_events_and_a_leave_when_the_pointer_moves_off() {
    let mut ui = Ui::new(Theme::dark(), DOUBLE_CLICK);
    let build = |ui: &mut Ui| {
        let mut signals = Vec::new();
        frame(ui, |ui| {
            ui.open(
                "row",
                Spec {
                    size: [fill(), fill()],
                    ..Spec::default()
                },
            );
            signals.push(ui.leaf(
                "page",
                Spec {
                    flags: Flags::CUSTOM | Flags::FOCUSABLE,
                    size: [fill(), fill()],
                    ..Spec::default()
                },
            ));
            signals.push(ui.leaf(
                "list",
                Spec {
                    flags: Flags::CLICKABLE,
                    size: [px(100.0), fill()],
                    ..Spec::default()
                },
            ));
            ui.close();
        });
        signals
    };
    build(&mut ui);
    let at = Instant::now();
    ui.event(Event::PointerMoved([20.0, 20.0]));
    ui.event(Event::Button {
        button: MouseButton::Left,
        pressed: true,
        at,
    });
    ui.event(Event::Key {
        key: Key::Character("a".into()),
        text: Some("a".into()),
    });
    let signals = build(&mut ui);
    assert!(signals[0].focused);
    assert_eq!(signals[0].events.len(), 3);
    assert!(matches!(signals[0].events[2], Event::Key { .. }));
    assert!(
        ui.cursor().is_none(),
        "the host picks the cursor over its box"
    );
    ui.event(Event::Pressure(Some(0.5)));
    ui.event(Event::PointerMoved([350.0, 20.0]));
    let signals = build(&mut ui);
    assert_eq!(
        signals[0].events,
        [
            Event::Pressure(Some(0.5)),
            Event::PointerMoved([350.0, 20.0])
        ],
        "a drag keeps its box, and the pen's pressure goes with it"
    );
    ui.event(Event::Button {
        button: MouseButton::Left,
        pressed: false,
        at,
    });
    let signals = build(&mut ui);
    assert!(matches!(
        signals[0].events[..],
        [Event::Button { .. }, Event::PointerLeft]
    ));
    assert!(signals[1].hovered);
    ui.event(Event::Modifiers(ModifiersState::SHIFT));
    let signals = build(&mut ui);
    assert_eq!(signals[0].events, [Event::Modifiers(ModifiersState::SHIFT)]);
}

#[test]
fn wheel_scrolls_within_the_content_and_clips_children() {
    let mut ui = Ui::new(Theme::dark(), DOUBLE_CLICK);
    let mut list = None;
    let mut last = None;
    let mut build = |ui: &mut Ui| {
        frame(ui, |ui| {
            list = Some(ui.open(
                "list",
                Spec {
                    flags: Flags::SCROLL | Flags::CLIP,
                    axis: Axis::Y,
                    size: [px(100.0), px(100.0)],
                    ..Spec::default()
                },
            ));
            for row in 0..10 {
                last = Some(ui.open(
                    row,
                    Spec {
                        flags: Flags::CLICKABLE,
                        size: [fill(), px(30.0)],
                        ..Spec::default()
                    },
                ));
                ui.close();
            }
            ui.close();
        });
    };
    build(&mut ui);
    ui.event(Event::PointerMoved([50.0, 50.0]));
    ui.event(Event::Wheel([0.0, -1000.0]));
    build(&mut ui);
    assert_eq!(
        ui.states[&list.unwrap()].scroll,
        200.0,
        "at once, as on the page"
    );
    assert_eq!(ui.rect(last.unwrap()), Some([0.0, 70.0, 100.0, 100.0]));
    let layers = ui.layers();
    assert!(matches!(
        layers[..],
        [Layer::Primitives(Primitives {
            clip: Some([0.0, 0.0, 100.0, 100.0]),
            ..
        })] | []
    ));
}

fn field_frame(ui: &mut Ui, text: &mut String) -> Signal {
    let mut signal = Signal::default();
    frame(ui, |ui| {
        signal = text_field(
            ui,
            field(),
            text,
            "Filter",
            Spec {
                size: [px(200.0), px(26.0)],
                pad: [6.0, 0.0],
                ..Spec::default()
            },
        );
    });
    signal
}

fn field() -> Id {
    Id::ROOT.child("filter")
}

fn key(named: NamedKey) -> Event {
    Event::Key {
        key: Key::Named(named),
        text: None,
    }
}

fn typed(text: &str) -> Event {
    Event::Key {
        key: Key::Character(text.into()),
        text: Some(text.into()),
    }
}

/// A focused field holding `text`.
fn focused_field(text: &str) -> (Ui, String) {
    let mut ui = Ui::new(Theme::dark(), DOUBLE_CLICK);
    let mut text = text.to_owned();
    field_frame(&mut ui, &mut text);
    ui.set_focus(Some(field()));
    (ui, text)
}

/// Moves the pointer over the caret before byte `index` of the field's `text`.
fn point_to(ui: &mut Ui, text: &str, index: usize) {
    let size = ui.theme.font_size;
    let (texts, frame) = (&mut ui.texts, ui.frame);
    let layout = &texts.label(text, size, frame).layout;
    let x = parley::editing::Cursor::from_byte_index(layout, index, parley::Affinity::Downstream)
        .geometry(layout, 1.0)
        .x0 as f32;
    ui.event(Event::PointerMoved([6.0 + x, 13.0]));
}

fn press(ui: &mut Ui, at: Instant, pressed: bool) {
    ui.event(Event::Button {
        button: MouseButton::Left,
        pressed,
        at,
    });
}

#[test]
fn text_fields_edit_with_keys_and_selection() {
    let (mut ui, mut text) = focused_field("café");
    ui.event(key(NamedKey::End));
    ui.event(key(NamedKey::Backspace));
    ui.event(typed("e"));
    field_frame(&mut ui, &mut text);
    assert_eq!(text, "cafe");
    ui.event(Event::Modifiers(ModifiersState::SHIFT));
    ui.event(key(NamedKey::ArrowLeft));
    ui.event(key(NamedKey::ArrowLeft));
    field_frame(&mut ui, &mut text);
    ui.event(Event::Modifiers(ModifiersState::empty()));
    ui.event(Event::Ime(Ime::Commit("é".into())));
    field_frame(&mut ui, &mut text);
    assert_eq!(text, "caé");
    ui.event(Event::Modifiers(SHORTCUT));
    ui.event(typed("a"));
    field_frame(&mut ui, &mut text);
    ui.event(Event::Modifiers(ModifiersState::empty()));
    ui.event(key(NamedKey::Delete));
    let signal = field_frame(&mut ui, &mut text);
    assert!(signal.focused);
    assert_eq!(text, "");
}

#[test]
fn a_field_focused_whole_selects_its_text_once_for_typing_over() {
    let mut ui = Ui::new(Theme::dark(), DOUBLE_CLICK);
    let mut text = "New Section 1".to_owned();
    ui.focus_all(field());
    field_frame(&mut ui, &mut text);
    ui.event(typed("K"));
    field_frame(&mut ui, &mut text);
    ui.event(typed("i"));
    let signal = field_frame(&mut ui, &mut text);
    assert!(signal.focused);
    assert_eq!(text, "Ki");
}

#[test]
#[cfg_attr(not(target_vendor = "apple"), ignore = "macOS editing conventions")]
fn text_field_carets_fade_on_appkit_s_blink_and_restart_when_moved() {
    let (mut ui, mut text) = focused_field("one");
    let start = Instant::now();
    let mut at = |ui: &mut Ui, millis: u64| {
        ui.begin([400.0, 300.0], 2.0, start + Duration::from_millis(millis));
        text_field(
            ui,
            field(),
            &mut text,
            "Filter",
            Spec {
                size: [px(200.0), px(26.0)],
                pad: [6.0, 0.0],
                ..Spec::default()
            },
        );
        ui.end();
        let caret = ui.theme.caret;
        let opacity = ui
            .layers()
            .iter()
            .flat_map(|layer| match layer {
                Layer::Primitives(layer) => layer.primitives.as_slice(),
                Layer::Custom { .. } => &[],
            })
            .find_map(|primitive| match primitive {
                Primitive::RoundedRect { rect, color, .. } if color[..3] == caret[..3] => {
                    assert_eq!(rect[2] - rect[0], draw::edit::CARET_WIDTH);
                    Some(color[3])
                }
                _ => None,
            })
            .unwrap_or(0.0);
        (opacity, ui.wake_at().map(|wake| wake - start))
    };
    assert_eq!(at(&mut ui, 0), (1.0, Some(Duration::from_micros(687_500))));
    let theme = Theme::dark();
    let faded = draw::edit::caret_color(theme.caret, theme.base, 0.75)[3];
    assert_eq!(at(&mut ui, 700).0, faded);
    assert_eq!(at(&mut ui, 900).0, 0.0);
    ui.event(key(NamedKey::ArrowRight));
    assert_eq!(
        at(&mut ui, 910),
        (1.0, Some(Duration::from_micros(1_597_500)))
    );
    ui.window_focused = false;
    assert_eq!(
        at(&mut ui, 920),
        (0.0, None),
        "an inactive window shows no caret"
    );
}

#[test]
#[cfg_attr(not(target_vendor = "apple"), ignore = "macOS editing conventions")]
fn text_fields_move_and_select_by_word() {
    let (mut ui, mut text) = focused_field("one two three");
    ui.event(key(NamedKey::End));
    ui.event(Event::Modifiers(ModifiersState::ALT));
    ui.event(key(NamedKey::ArrowLeft));
    ui.event(key(NamedKey::ArrowLeft));
    field_frame(&mut ui, &mut text);
    ui.event(Event::Modifiers(
        ModifiersState::ALT | ModifiersState::SHIFT,
    ));
    ui.event(key(NamedKey::ArrowRight));
    field_frame(&mut ui, &mut text);
    ui.event(Event::Modifiers(ModifiersState::empty()));
    ui.event(typed("2"));
    field_frame(&mut ui, &mut text);
    assert_eq!(text, "one 2 three");
}

#[test]
#[cfg_attr(
    target_vendor = "apple",
    ignore = "GTK and Windows editing conventions"
)]
fn text_fields_follow_gtk_and_windows_chords() {
    let (mut ui, mut text) = focused_field("one two three");
    ui.event(key(NamedKey::End));
    ui.event(Event::Modifiers(ModifiersState::CONTROL));
    ui.event(key(NamedKey::ArrowLeft));
    ui.event(key(NamedKey::ArrowLeft));
    field_frame(&mut ui, &mut text);
    ui.event(Event::Modifiers(
        ModifiersState::CONTROL | ModifiersState::SHIFT,
    ));
    ui.event(key(NamedKey::ArrowRight));
    field_frame(&mut ui, &mut text);
    ui.event(Event::Modifiers(ModifiersState::empty()));
    ui.event(typed("2"));
    field_frame(&mut ui, &mut text);
    assert_eq!(text, "one 2 three");
    ui.event(Event::Modifiers(ModifiersState::CONTROL));
    ui.event(key(NamedKey::Backspace));
    field_frame(&mut ui, &mut text);
    assert_eq!(text, "one  three");
    ui.event(Event::Modifiers(ModifiersState::empty()));
    ui.event(key(NamedKey::Home));
    ui.event(typed("<"));
    ui.event(key(NamedKey::End));
    ui.event(typed(">"));
    field_frame(&mut ui, &mut text);
    assert_eq!(text, "<one  three>");
    ui.event(Event::Modifiers(ModifiersState::CONTROL));
    ui.event(typed("a"));
    field_frame(&mut ui, &mut text);
    ui.event(Event::Modifiers(ModifiersState::empty()));
    ui.event(key(NamedKey::Delete));
    field_frame(&mut ui, &mut text);
    assert_eq!(text, "");
}

#[test]
#[cfg_attr(not(target_vendor = "apple"), ignore = "macOS editing conventions")]
fn text_fields_delete_by_word_and_line_chords() {
    let (mut ui, mut text) = focused_field("one two three");
    ui.event(key(NamedKey::End));
    ui.event(Event::Modifiers(ModifiersState::ALT));
    ui.event(key(NamedKey::Backspace));
    field_frame(&mut ui, &mut text);
    assert_eq!(text, "one two ");
    ui.event(Event::Modifiers(ModifiersState::CONTROL));
    ui.event(typed("b"));
    ui.event(typed("k"));
    field_frame(&mut ui, &mut text);
    assert_eq!(text, "one two");
    ui.event(Event::Modifiers(ModifiersState::SUPER));
    ui.event(key(NamedKey::Backspace));
    field_frame(&mut ui, &mut text);
    assert_eq!(text, "");
}

#[test]
fn text_fields_double_press_selects_a_word_and_drags_by_words() {
    let (mut ui, mut text) = focused_field("one two three");
    let at = Instant::now();
    point_to(&mut ui, &text, 5);
    press(&mut ui, at, true);
    press(&mut ui, at, false);
    field_frame(&mut ui, &mut text);
    press(&mut ui, at + Duration::from_millis(100), true);
    field_frame(&mut ui, &mut text);
    assert_eq!(ui.states[&field()].selection.text_range(), 4..7);
    point_to(&mut ui, &text, 10);
    field_frame(&mut ui, &mut text);
    assert_eq!(ui.states[&field()].selection.text_range(), 4..13);
    point_to(&mut ui, &text, 1);
    field_frame(&mut ui, &mut text);
    press(&mut ui, at + Duration::from_millis(200), false);
    field_frame(&mut ui, &mut text);
    let selection = ui.states[&field()].selection;
    assert_eq!(
        (selection.anchor().index(), selection.focus().index()),
        (7, 0),
        "reversing keeps the pressed word"
    );
}

#[test]
fn text_fields_drag_select_and_shift_press_extends() {
    let (mut ui, mut text) = focused_field("one two three");
    let at = Instant::now();
    point_to(&mut ui, &text, 4);
    press(&mut ui, at, true);
    field_frame(&mut ui, &mut text);
    point_to(&mut ui, &text, 7);
    field_frame(&mut ui, &mut text);
    press(&mut ui, at, false);
    ui.event(Event::Modifiers(ModifiersState::SHIFT));
    field_frame(&mut ui, &mut text);
    assert_eq!(ui.states[&field()].selection.text_range(), 4..7);
    point_to(&mut ui, &text, 13);
    press(&mut ui, at + Duration::from_secs(1), true);
    press(&mut ui, at + Duration::from_secs(1), false);
    field_frame(&mut ui, &mut text);
    assert_eq!(ui.states[&field()].selection.text_range(), 4..13);
    ui.event(Event::Modifiers(ModifiersState::empty()));
    field_frame(&mut ui, &mut text);
    ui.event(key(NamedKey::ArrowLeft));
    ui.event(typed("_"));
    field_frame(&mut ui, &mut text);
    assert_eq!(
        text, "one _two three",
        "Left collapses to the selection's start"
    );
}

#[test]
fn scrollbar_thumbs_track_the_offset_and_drags_reach_both_ends() {
    let mut ui = Ui::new(Theme::dark(), DOUBLE_CLICK);
    let mut offset = 0.0;
    let build = |ui: &mut Ui, offset: &mut f32| {
        frame(ui, |ui| {
            ui.open(
                "view",
                Spec {
                    flags: Flags::CUSTOM,
                    size: [px(200.0), px(218.0)],
                    ..Spec::default()
                },
            );
            if let Some(chosen) = scrollbar(
                ui,
                "bar",
                Axis::Y,
                *offset,
                [0.0, 1000.0],
                400.0,
                true,
                [0.5; 4],
            ) {
                *offset = chosen;
            }
            ui.close();
        });
    };
    build(&mut ui, &mut offset);
    build(&mut ui, &mut offset);
    let thumb = Id::ROOT.child("view").child("bar");
    let rect = ui.rect(thumb).unwrap();
    assert_eq!(
        rect,
        [190.0, 4.0, 196.0, 61.0],
        "a 400 of 1400 share of a 200 track"
    );
    let at = Instant::now();
    ui.event(Event::PointerMoved([193.0, 10.0]));
    ui.event(Event::Button {
        button: MouseButton::Left,
        pressed: true,
        at,
    });
    build(&mut ui, &mut offset);
    assert_eq!(offset, 0.0, "a press alone keeps the offset");
    ui.event(Event::PointerMoved([193.0, 500.0]));
    build(&mut ui, &mut offset);
    assert_eq!(offset, 1000.0);
    build(&mut ui, &mut offset);
    assert_eq!(ui.rect(thumb).unwrap(), [190.0, 147.0, 196.0, 204.0]);
    ui.event(Event::PointerMoved([193.0, -500.0]));
    build(&mut ui, &mut offset);
    assert_eq!(offset, 0.0);
}

#[test]
fn a_scrollbar_without_a_corner_ends_as_far_from_its_end_as_from_its_start() {
    let mut ui = Ui::new(Theme::dark(), DOUBLE_CLICK);
    for _ in 0..2 {
        frame(&mut ui, |ui| {
            ui.open(
                "view",
                Spec {
                    flags: Flags::CUSTOM,
                    size: [px(200.0), px(208.0)],
                    ..Spec::default()
                },
            );
            scrollbar(
                ui,
                "bar",
                Axis::Y,
                1000.0,
                [0.0, 1000.0],
                400.0,
                false,
                [0.5; 4],
            );
            ui.close();
        });
    }
    let rect = ui.rect(Id::ROOT.child("view").child("bar")).unwrap();
    assert_eq!(rect[3], 204.0);
}

#[test]
fn system_scrollers_step_page_and_drag_by_the_parts_the_platform_paints() {
    let mut ui = Ui::new(Theme::dark(), DOUBLE_CLICK);
    // Arrows together at the far end, as 10.6 places them.
    ui.scrollers = Some(Scrollers {
        thickness: 15.0,
        paint: Box::new(|request: &Scroller| {
            let slot = request.length - 30.0;
            let knob = slot * request.proportion;
            let start = request.value * (slot - knob);
            PaintedScroller {
                image: RasterImage::new(
                    [15, request.length as u32],
                    vec![0; 60 * request.length as usize],
                )
                .unwrap(),
                knob: [start, start + knob],
                slot: [0.0, slot],
                decrement: [slot, slot + 15.0],
                increment: [slot + 15.0, request.length],
            }
        }),
    });
    let mut offset = 0.0;
    let build = |ui: &mut Ui, offset: &mut f32| {
        frame(ui, |ui| {
            ui.open(
                "view",
                Spec {
                    flags: Flags::CUSTOM,
                    size: [px(200.0), px(215.0)],
                    ..Spec::default()
                },
            );
            if let Some(chosen) = scrollbar(
                ui,
                "bar",
                Axis::Y,
                *offset,
                [0.0, 1000.0],
                400.0,
                true,
                [0.5; 4],
            ) {
                *offset = chosen;
            }
            ui.close();
        });
    };
    build(&mut ui, &mut offset);
    build(&mut ui, &mut offset);
    let bar = Id::ROOT.child("view").child("bar");
    assert_eq!(
        ui.rect(bar).unwrap(),
        [185.0, 0.0, 200.0, 200.0],
        "the corner stays clear"
    );
    let at = Instant::now();
    let click = |ui: &mut Ui, y: f32, pressed: bool| {
        ui.event(Event::PointerMoved([190.0, y]));
        ui.event(Event::Button {
            button: MouseButton::Left,
            pressed,
            at,
        });
    };
    click(&mut ui, 190.0, true);
    build(&mut ui, &mut offset);
    assert_eq!(
        offset, 20.0,
        "the increment arrow steps a twentieth of the view"
    );
    click(&mut ui, 190.0, false);
    build(&mut ui, &mut offset);
    // Below the knob, 170 × 400 / 1400 long, the track pages.
    click(&mut ui, 160.0, true);
    build(&mut ui, &mut offset);
    assert_eq!(offset, 400.0);
    click(&mut ui, 160.0, false);
    build(&mut ui, &mut offset);
    click(&mut ui, 60.0, true);
    build(&mut ui, &mut offset);
    ui.event(Event::PointerMoved([190.0, 500.0]));
    build(&mut ui, &mut offset);
    assert_eq!(offset, 1000.0, "the knob drags to the end");
}

fn items() -> Vec<popup::Item<'static>> {
    let item = |text| popup::Item {
        text,
        ..popup::Item::default()
    };
    vec![
        item("Cut"),
        item("Copy"),
        popup::Item {
            disabled: true,
            ..item("Paste")
        },
        popup::Item {
            separated: true,
            ..item("Select all")
        },
    ]
}

fn menu_id() -> Id {
    Id::ROOT.child("menu")
}

/// Builds a frame with a button filling the window under the menu, and returns the
/// button's signal and the item chosen.
fn menu_frame(ui: &mut Ui, anchor: Anchor, filter: Option<&str>) -> (Signal, Option<usize>) {
    let mut result = (Signal::default(), None);
    frame(ui, |ui| {
        let under = ui.leaf(
            "under",
            Spec {
                flags: Flags::CLICKABLE,
                size: [fill(), fill()],
                ..Spec::default()
            },
        );
        let chosen = popup::menu(ui, menu_id(), anchor, &items(), filter);
        result = (under, chosen);
    });
    result
}

const BELOW: Anchor = Anchor::Below([20.0, 20.0, 100.0, 40.0]);

/// A window with the menu open and faded in below `BELOW`.
fn open_menu(filter: Option<&str>) -> Ui {
    let mut ui = Ui::new(Theme::dark(), DOUBLE_CLICK);
    menu_frame(&mut ui, BELOW, filter);
    ui.open_popup(menu_id());
    for _ in 0..40 {
        menu_frame(&mut ui, BELOW, filter);
    }
    ui
}

/// The centre of the menu's row for `item`, from the latest layout.
fn row(ui: &Ui, item: usize) -> [f32; 2] {
    let rect = ui.rect(menu_id().child("rows").child(item)).unwrap();
    [(rect[0] + rect[2]) / 2.0, (rect[1] + rect[3]) / 2.0]
}

fn click(ui: &mut Ui, point: [f32; 2]) {
    let at = Instant::now();
    ui.event(Event::PointerMoved(point));
    press(ui, at, true);
    press(ui, at, false);
}

/// Two tool buttons with tooltips, the second at the window's trailing edge; returns
/// their tooltips' rectangles.
fn tip_frame(ui: &mut Ui) -> [Option<[f32; 4]>; 2] {
    let mut tips = [Id::ROOT; 2];
    frame(ui, |ui| {
        for (index, x) in [0.0, 380.0].into_iter().enumerate() {
            let id = ui.open(
                ("tool", index),
                Spec {
                    flags: Flags::CLICKABLE | Flags::FLOAT,
                    size: [px(20.0), px(20.0)],
                    position: [x, 0.0],
                    ..Spec::default()
                },
            );
            ui.close();
            popup::tooltip(ui, "Bold", "Ctrl+B", Some("Makes the selected text bold."));
            tips[index] = id.child("tooltip");
        }
    });
    tips.map(|tip| ui.rect(tip))
}

/// Frames the pointer rests through for `millis`, returning the tooltips as they end.
fn rest(ui: &mut Ui, millis: u32) -> [Option<[f32; 4]>; 2] {
    let mut tips = [None; 2];
    for _ in 0..millis / 16 {
        tips = tip_frame(ui);
    }
    tips
}

#[test]
fn tooltips_wait_then_switch_at_once_and_hide_on_press_until_left() {
    let mut ui = Ui::new(Theme::dark(), DOUBLE_CLICK);
    tip_frame(&mut ui);
    ui.event(Event::PointerMoved([10.0, 10.0]));
    assert_eq!(rest(&mut ui, 400), [None; 2]);
    assert!(ui.wake_at().is_some(), "the delay wakes the host");
    let [Some(first), None] = rest(&mut ui, 200) else {
        panic!("the first tooltip shows after its delay");
    };
    assert_eq!(
        first[..2],
        [POPUP_MARGIN, 20.0 + popup::PAD],
        "off the window's edge"
    );

    ui.event(Event::PointerMoved([390.0, 10.0]));
    let [None, Some(second)] = tip_frame(&mut ui) else {
        panic!("moving to another shows its tooltip at once");
    };
    assert_eq!(second[2], 400.0 - POPUP_MARGIN, "kept inside the window");

    press(&mut ui, Instant::now(), true);
    press(&mut ui, Instant::now(), false);
    assert_eq!(
        rest(&mut ui, 1000),
        [None; 2],
        "a press hides it while hovered"
    );
    ui.event(Event::PointerMoved([200.0, 10.0]));
    rest(&mut ui, 100);
    ui.event(Event::PointerMoved([10.0, 10.0]));
    assert_eq!(
        rest(&mut ui, 100),
        [None; 2],
        "after a press it waits again"
    );
    assert!(rest(&mut ui, 500)[0].is_some());
}

#[test]
fn popups_open_beside_their_anchor_and_flip_to_stay_in_the_window() {
    let mut ui = open_menu(None);
    let rect = ui.rect(menu_id()).unwrap();
    assert_eq!(
        [rect[0], rect[1]],
        [20.0, 44.0],
        "under the anchor, a gap below"
    );
    assert_eq!(rect[2] - rect[0], 140.0, "no narrower than a menu");
    let height = rect[3] - rect[1];
    menu_frame(&mut ui, Anchor::Below([300.0, 260.0, 380.0, 280.0]), None);
    menu_frame(&mut ui, Anchor::Below([300.0, 260.0, 380.0, 280.0]), None);
    assert_eq!(
        ui.rect(menu_id()),
        Some([240.0, 256.0 - height, 380.0, 256.0]),
        "above, level with the anchor's far edge"
    );
    menu_frame(&mut ui, Anchor::Right([350.0, 100.0, 390.0, 120.0]), None);
    menu_frame(&mut ui, Anchor::Right([350.0, 100.0, 390.0, 120.0]), None);
    assert_eq!(
        ui.rect(menu_id()),
        Some([210.0, 96.0, 350.0, 96.0 + height]),
        "a submenu that cannot fit to the right opens to the left, its first row level"
    );
}

#[test]
fn popups_too_large_for_a_narrow_window_shrink_inside_its_margin() {
    let inside = |rect: [f32; 4], window: [f32; 2]| {
        rect[0] >= POPUP_MARGIN
            && rect[1] >= POPUP_MARGIN
            && rect[2] <= window[0] - POPUP_MARGIN
            && rect[3] <= window[1] - POPUP_MARGIN
    };
    let mut ui = Ui::new(Theme::dark(), DOUBLE_CLICK);
    let narrow = [100.0, 300.0];
    let menu = |ui: &mut Ui| {
        sized_frame(ui, narrow, |ui| {
            popup::menu(ui, menu_id(), BELOW, &items(), None);
        });
    };
    menu(&mut ui);
    ui.open_popup(menu_id());
    for _ in 0..40 {
        menu(&mut ui);
    }
    let rect = ui.rect(menu_id()).unwrap();
    assert!(inside(rect, narrow), "{rect:?} leaves the window's margin");
    assert_eq!(
        rect[2] - rect[0],
        100.0 - 2.0 * POPUP_MARGIN,
        "as wide as fits"
    );
    let row = ui.rect(menu_id().child("rows").child(0_u64)).unwrap();
    assert!(row[2] <= rect[2], "its rows narrow with it");

    // A dialog taller than a short window scrolls what it holds.
    let short = [400.0, 120.0];
    let dialog = Id::ROOT.child("dialog");
    let mut ui = Ui::new(Theme::dark(), DOUBLE_CLICK);
    let build = |ui: &mut Ui| {
        sized_frame(ui, short, |ui| {
            ui.open_as(
                dialog,
                Spec {
                    axis: Axis::Y,
                    size: [px(600.0), children()],
                    anchor: Some(Anchor::Dialog),
                    ..Spec::default()
                },
            );
            for line in 0..10 {
                ui.leaf(
                    line,
                    Spec {
                        size: [fill(), px(30.0)],
                        text: Some("Line"),
                        // Wider than the squeezed dialog leaves it.
                        pad: [250.0, 0.0],
                        ..Spec::default()
                    },
                );
            }
            ui.close();
        });
    };
    build(&mut ui);
    ui.open_popup(dialog);
    for _ in 0..40 {
        build(&mut ui);
    }
    let rect = ui.rect(dialog).unwrap();
    assert!(inside(rect, short), "{rect:?} leaves the window's margin");
    let first = ui.rect(dialog.child(0)).unwrap();
    ui.event(Event::PointerMoved([200.0, 60.0]));
    ui.event(Event::Wheel([0.0, -100.0]));
    for _ in 0..40 {
        build(&mut ui);
    }
    let scrolled = ui.rect(dialog.child(0)).unwrap();
    assert!(scrolled[1] < first[1], "the wheel scrolls it");
    for layer in ui.layers() {
        let Layer::Primitives(layer) = layer else {
            continue;
        };
        for primitive in &layer.primitives {
            if let draw::Primitive::Text {
                clip: Some(clip), ..
            } = primitive
            {
                assert!(clip[0] <= clip[2], "{clip:?} is inverted");
            }
        }
    }
}

#[test]
fn popups_show_at_once_by_their_anchor() {
    let mut ui = Ui::new(Theme::dark(), DOUBLE_CLICK);
    menu_frame(&mut ui, BELOW, None);
    ui.open_popup(menu_id());
    menu_frame(&mut ui, BELOW, None);
    menu_frame(&mut ui, BELOW, None);
    assert_eq!(
        ui.rect(menu_id()).unwrap()[1],
        44.0,
        "right under the anchor"
    );
    ui.close_popup(menu_id());
    menu_frame(&mut ui, Anchor::Point([50.0, 50.0]), None);
    ui.open_popup(menu_id());
    menu_frame(&mut ui, Anchor::Point([50.0, 50.0]), None);
    menu_frame(&mut ui, Anchor::Point([50.0, 50.0]), None);
    assert_eq!(ui.rect(menu_id()).unwrap()[..2], [50.0, 50.0]);
}

#[test]
fn a_press_outside_dismisses_without_reaching_what_is_beneath() {
    let mut ui = open_menu(None);
    click(&mut ui, [300.0, 250.0]);
    let (under, chosen) = menu_frame(&mut ui, BELOW, None);
    assert!(!ui.popup_open(menu_id()) && chosen.is_none());
    assert!(!under.pressed && !under.clicked);
    click(&mut ui, [300.0, 250.0]);
    let (under, _) = menu_frame(&mut ui, BELOW, None);
    assert!(under.clicked, "once closed, beneath takes presses again");
}

#[test]
fn escape_dismisses_and_returns_the_focus() {
    let mut ui = Ui::new(Theme::dark(), DOUBLE_CLICK);
    let page = Id::ROOT.child("under");
    menu_frame(&mut ui, BELOW, None);
    ui.set_focus(Some(page));
    ui.open_popup(menu_id());
    menu_frame(&mut ui, BELOW, None);
    assert_eq!(ui.focused(), Some(menu_id()));
    ui.event(key(NamedKey::Escape));
    menu_frame(&mut ui, BELOW, None);
    assert!(!ui.popup_open(menu_id()));
    assert_eq!(ui.focused(), Some(page));
}

#[test]
fn input_over_a_popup_stays_with_it() {
    let mut ui = open_menu(None);
    ui.event(Event::PointerMoved(row(&ui, 1)));
    let (under, _) = menu_frame(&mut ui, BELOW, None);
    assert!(!under.hovered);
    let point = row(&ui, 1);
    click(&mut ui, point);
    let (under, chosen) = menu_frame(&mut ui, BELOW, None);
    assert_eq!(chosen, Some(1));
    assert!(!under.pressed && !under.clicked);
    assert!(!ui.popup_open(menu_id()), "choosing closes it");
}

#[test]
fn disabled_items_ignore_the_pointer() {
    let mut ui = open_menu(None);
    let point = row(&ui, 2);
    click(&mut ui, point);
    let (_, chosen) = menu_frame(&mut ui, BELOW, None);
    assert_eq!(chosen, None);
    assert!(ui.popup_open(menu_id()));
}

#[test]
fn keys_move_the_highlight_past_disabled_items() {
    let mut ui = open_menu(None);
    for (keys, highlight) in [
        (vec![NamedKey::ArrowDown], 0),
        (vec![NamedKey::ArrowDown, NamedKey::ArrowDown], 3),
        (vec![NamedKey::ArrowDown], 3),
        (vec![NamedKey::ArrowUp], 1),
        (vec![NamedKey::End], 3),
        (vec![NamedKey::Home], 0),
        (vec![NamedKey::PageDown], 3),
    ] {
        for named in keys {
            ui.event(key(named));
        }
        menu_frame(&mut ui, BELOW, None);
        assert_eq!(ui.popups[0].highlight, Some(highlight));
    }
    ui.event(key(NamedKey::Enter));
    assert_eq!(menu_frame(&mut ui, BELOW, None).1, Some(3));
}

#[test]
fn the_pointer_moves_the_highlight_only_when_it_moves() {
    let mut ui = open_menu(None);
    ui.event(Event::PointerMoved(row(&ui, 1)));
    menu_frame(&mut ui, BELOW, None);
    ui.event(key(NamedKey::ArrowDown));
    menu_frame(&mut ui, BELOW, None);
    menu_frame(&mut ui, BELOW, None);
    assert_eq!(
        ui.popups[0].highlight,
        Some(3),
        "a still pointer keeps its row"
    );
}

#[test]
fn headings_are_never_chosen_and_hide_while_filtered() {
    let mut ui = open_menu(Some("Filter"));
    let mut items = items();
    items.insert(
        3,
        popup::Item {
            text: "Selection",
            heading: true,
            separated: true,
            ..popup::Item::default()
        },
    );
    items[4].separated = false;
    let build = |ui: &mut Ui| {
        let mut chosen = None;
        frame(ui, |ui| {
            chosen = popup::menu(ui, menu_id(), BELOW, &items, Some("Filter"));
        });
        chosen
    };
    build(&mut ui);
    ui.event(key(NamedKey::End));
    build(&mut ui);
    ui.event(key(NamedKey::ArrowUp));
    build(&mut ui);
    assert_eq!(
        ui.popups[0].highlight,
        Some(1),
        "keys skip the heading and Paste"
    );
    ui.event(typed("sel"));
    build(&mut ui);
    ui.event(key(NamedKey::Enter));
    assert_eq!(
        build(&mut ui),
        Some(4),
        "the query matches the item, not its heading"
    );
}

#[test]
fn typing_filters_to_the_best_matches_first() {
    let mut ui = open_menu(Some("Filter"));
    assert_eq!(ui.focused(), Some(menu_id().child("filter")));
    for text in ["c", "t"] {
        ui.event(typed(text));
    }
    let settle = |ui: &mut Ui| {
        for _ in 0..30 {
            menu_frame(ui, BELOW, Some("Filter"));
        }
    };
    settle(&mut ui);
    assert_eq!(ui.popups[0].query, "ct");
    let shown = |ui: &Ui| {
        let mut shown: Vec<_> = (0..4_usize)
            .filter_map(|item| {
                let rect = ui.rect(menu_id().child("rows").child(item))?;
                Some((rect[1] as i32, item))
            })
            .collect();
        shown.sort();
        shown.into_iter().map(|(_, item)| item).collect::<Vec<_>>()
    };
    assert_eq!(
        shown(&ui),
        [0, 3],
        "a word's start before the middle of one"
    );
    ui.event(key(NamedKey::Backspace));
    settle(&mut ui);
    assert_eq!(shown(&ui), [0, 1, 3], "equal matches keep their order");
    ui.event(key(NamedKey::ArrowDown));
    ui.event(key(NamedKey::Enter));
    assert_eq!(menu_frame(&mut ui, BELOW, Some("Filter")).1, Some(1));
}

#[test]
fn nothing_matching_leaves_nothing_to_choose_and_enter_leaves_the_menu() {
    let mut ui = open_menu(Some("Filter"));
    ui.event(typed("z"));
    menu_frame(&mut ui, BELOW, Some("Filter"));
    ui.event(key(NamedKey::ArrowDown));
    let (_, chosen) = menu_frame(&mut ui, BELOW, Some("Filter"));
    assert_eq!(chosen, None);
    assert!(ui.popup_open(menu_id()));
    ui.event(key(NamedKey::Enter));
    let (_, chosen) = menu_frame(&mut ui, BELOW, Some("Filter"));
    assert_eq!(chosen, None);
    assert!(!ui.popup_open(menu_id()));
}

#[test]
fn the_palette_centres_near_the_windows_top_and_chooses_the_best_match() {
    let mut ui = Ui::new(Theme::dark(), DOUBLE_CLICK);
    let palette = Id::ROOT.child("palette");
    let commands = items();
    let build = |ui: &mut Ui| {
        let mut chosen = None;
        frame(ui, |ui| {
            chosen = popup::palette(ui, palette, &[("", &commands)], "Search commands")
        });
        match chosen {
            Some(popup::Pick::Run(_, index)) => Some(index),
            _ => None,
        }
    };
    build(&mut ui);
    ui.open_popup(palette);
    for _ in 0..40 {
        build(&mut ui);
    }
    let rect = ui.rect(palette).unwrap();
    assert_eq!(rect[0] + rect[2], 400.0, "centred");
    assert_eq!(rect[1], 300.0 / 8.0, "an eighth down");
    ui.event(typed("a"));
    ui.event(key(NamedKey::Enter));
    assert_eq!(
        build(&mut ui),
        Some(3),
        "`all` begins a word; `Paste` is disabled"
    );
}

#[test]
fn a_prefix_typed_in_the_palette_switches_what_it_lists() {
    let mut ui = Ui::new(Theme::dark(), DOUBLE_CLICK);
    let palette = Id::ROOT.child("palette");
    let [commands, places] = [items(), fonts()];
    let build = |ui: &mut Ui| {
        let mut chosen = None;
        frame(ui, |ui| {
            let modes: [(&str, &[popup::Item]); 2] = [(">", &commands), ("", &places)];
            chosen = popup::palette(ui, palette, &modes, "Go to");
        });
        chosen
    };
    build(&mut ui);
    popup::open_with(&mut ui, palette, ">");
    build(&mut ui);
    ui.event(typed("sel"));
    ui.event(key(NamedKey::Enter));
    assert_eq!(
        build(&mut ui),
        Some(popup::Pick::Run(0, 3)),
        "typed after the prefix"
    );
    popup::open_with(&mut ui, palette, ">");
    build(&mut ui);
    ui.event(key(NamedKey::Backspace));
    build(&mut ui);
    assert_eq!(popup::query(&ui, palette), Some(""));
    ui.event(typed("tnr"));
    ui.event(key(NamedKey::Enter));
    assert_eq!(
        build(&mut ui),
        Some(popup::Pick::Run(1, 42)),
        "back among the places"
    );
}

#[test]
fn the_palette_opens_an_items_actions_on_command_k_or_a_right_click() {
    let mut ui = Ui::new(Theme::dark(), DOUBLE_CLICK);
    let palette = Id::ROOT.child("palette");
    let actions = popup::actions(palette);
    let mut places = items();
    // The first repeats an item further on, as a recent page does.
    places[0].repeated = true;
    let build = |ui: &mut Ui| {
        let mut picked = None;
        frame(ui, |ui| {
            picked = popup::palette(ui, palette, &[("", &places)], "Go to");
            popup::menu(
                ui,
                actions,
                Anchor::Point([0.0; 2]),
                &items(),
                Some("Actions"),
            );
        });
        picked
    };
    build(&mut ui);
    ui.open_popup(palette);
    build(&mut ui);
    ui.event(key(NamedKey::ArrowDown));
    build(&mut ui);
    ui.event(Event::Modifiers(SHORTCUT));
    ui.event(typed("k"));
    assert_eq!(build(&mut ui), Some(popup::Pick::Actions(0, 0)));
    assert!(ui.popup_open(actions));
    assert_eq!(
        popup::query(&ui, palette),
        Some(""),
        "the chord types nothing"
    );
    ui.event(Event::Modifiers(ModifiersState::empty()));
    build(&mut ui);
    ui.event(key(NamedKey::Escape));
    build(&mut ui);
    assert!(
        ui.popup_open(palette) && !ui.popup_open(actions),
        "Escape goes back to the palette"
    );
    let row = ui.rect(palette.child("rows").child(3_u64)).unwrap();
    ui.event(Event::PointerMoved([row[0] + 4.0, row[1] + 4.0]));
    ui.event(Event::Button {
        button: MouseButton::Right,
        pressed: true,
        at: Instant::now(),
    });
    assert_eq!(build(&mut ui), Some(popup::Pick::Actions(0, 3)));
    assert!(ui.popup_open(actions));
    ui.event(key(NamedKey::Escape));
    build(&mut ui);
    ui.event(typed("c"));
    build(&mut ui);
    assert_eq!(
        ui.rect(palette.child("rows").child(0_u64)),
        None,
        "a repeated item shows only while unfiltered"
    );
}

#[test]
fn colour_grids_move_in_two_dimensions_and_choose_a_swatch_or_none() {
    let mut ui = Ui::new(Theme::dark(), DOUBLE_CLICK);
    let grid = Id::ROOT.child("colours");
    let swatches: Vec<([f32; 4], &str)> = (0..6)
        .map(|index| ([index as f32, 0.0, 0.0, 1.0], ""))
        .collect();
    let build = |ui: &mut Ui| {
        let mut chosen = None;
        frame(ui, |ui| {
            chosen = popup::colors(ui, grid, BELOW, "Automatic", &swatches, 3);
        });
        chosen
    };
    build(&mut ui);
    ui.open_popup(grid);
    build(&mut ui);
    for named in [
        NamedKey::ArrowDown,
        NamedKey::ArrowDown,
        NamedKey::ArrowDown,
        NamedKey::ArrowRight,
        NamedKey::Enter,
    ] {
        ui.event(key(named));
    }
    assert_eq!(build(&mut ui), Some(Some(swatches[4].0)));
    ui.open_popup(grid);
    for _ in 0..40 {
        build(&mut ui);
    }
    let rect = ui.rect(grid.child(("cell", 0_usize))).unwrap();
    click(&mut ui, [rect[0] + 4.0, rect[1] + 4.0]);
    assert_eq!(build(&mut ui), Some(None));
}

#[test]
fn table_pickers_choose_columns_and_rows_by_keys_or_a_click() {
    let mut ui = Ui::new(Theme::dark(), DOUBLE_CLICK);
    let grid = Id::ROOT.child("table");
    let build = |ui: &mut Ui| {
        let mut chosen = None;
        frame(ui, |ui| {
            chosen = popup::table_picker(ui, grid, BELOW, [4, 3]);
        });
        chosen
    };
    build(&mut ui);
    ui.open_popup(grid);
    build(&mut ui);
    for named in [
        NamedKey::ArrowDown,
        NamedKey::ArrowRight,
        NamedKey::ArrowRight,
        NamedKey::ArrowDown,
        NamedKey::ArrowDown,
        NamedKey::Enter,
    ] {
        ui.event(key(named));
    }
    assert_eq!(build(&mut ui), Some([3, 3]));
    ui.open_popup(grid);
    for _ in 0..40 {
        build(&mut ui);
    }
    let rect = ui.rect(grid.child(("cell", 5_usize))).unwrap();
    click(&mut ui, [rect[0] + 4.0, rect[1] + 4.0]);
    assert_eq!(build(&mut ui), Some([2, 2]));
}

#[test]
fn popups_opened_within_a_popup_stay_above_it() {
    let mut ui = open_menu(None);
    let submenu = Id::ROOT.child("submenu");
    let build = |ui: &mut Ui, open: bool| {
        frame(ui, |ui| {
            ui.open_as(
                menu_id(),
                Spec {
                    size: [px(100.0), px(100.0)],
                    anchor: Some(BELOW),
                    ..Spec::default()
                },
            );
            if open {
                ui.open_popup(submenu);
            }
            ui.close();
            popup::menu(
                ui,
                submenu,
                Anchor::Right([100.0, 50.0, 120.0, 70.0]),
                &items(),
                None,
            );
        })
    };
    build(&mut ui, true);
    build(&mut ui, false);
    assert!(ui.popup_open(menu_id()) && ui.popup_open(submenu));
    ui.event(key(NamedKey::Escape));
    build(&mut ui, false);
    assert!(ui.popup_open(menu_id()) && !ui.popup_open(submenu));
}

#[test]
fn a_submenu_opens_beside_its_row_once_the_pointer_rests_and_closes_with_its_choice() {
    let submenu = Id::ROOT.child("submenu");
    let mut entries = items();
    entries[1].submenu = true;
    let build = |ui: &mut Ui| {
        let mut chosen = None;
        frame(ui, |ui| {
            chosen = popup::menu(ui, menu_id(), BELOW, &entries, None);
            popup::submenus(ui, menu_id(), &entries, |item| {
                (item == 1).then_some(submenu)
            });
            popup::menu(ui, submenu, BELOW, &items(), None);
        });
        chosen
    };
    let mut ui = Ui::new(Theme::dark(), DOUBLE_CLICK);
    build(&mut ui);
    ui.open_popup(menu_id());
    for _ in 0..40 {
        build(&mut ui);
    }
    ui.event(Event::PointerMoved(row(&ui, 1)));
    for _ in 0..10 {
        build(&mut ui);
    }
    assert!(!ui.popup_open(submenu), "not while the pointer passes");
    for _ in 0..30 {
        build(&mut ui);
    }
    assert!(ui.popup_open(submenu), "once it rests");
    let [menu, beside] = [menu_id(), submenu].map(|id| ui.rect(id).unwrap());
    assert!(
        beside[0] >= menu[2] - 1.0,
        "beside the menu, not below the anchor"
    );
    ui.event(key(NamedKey::Escape));
    for _ in 0..40 {
        build(&mut ui);
    }
    assert!(
        ui.popup_open(menu_id()) && !ui.popup_open(submenu),
        "Escape closes only the submenu, which stays shut while the pointer rests"
    );
    let point = row(&ui, 1);
    click(&mut ui, point);
    assert_eq!(build(&mut ui), None, "a click opens it, not a choice");
    build(&mut ui);
    assert!(ui.popup_open(menu_id()) && ui.popup_open(submenu));
    let first = ui.rect(submenu.child("rows").child(0_u64)).unwrap();
    click(&mut ui, [first[0] + 4.0, first[1] + 4.0]);
    build(&mut ui);
    assert!(
        !ui.popup_open(menu_id()) && !ui.popup_open(submenu),
        "a choice ends both"
    );
}

/// Keys in order, counting the lookups a list makes.
struct Keyed {
    keys: Vec<u64>,
    at: HashMap<u64, usize>,
    lookups: std::cell::Cell<usize>,
}

impl Keyed {
    fn new(keys: impl IntoIterator<Item = u64>) -> Self {
        let keys: Vec<u64> = keys.into_iter().collect();
        let at = keys
            .iter()
            .enumerate()
            .map(|(index, key)| (*key, index))
            .collect();
        Self {
            keys,
            at,
            lookups: Default::default(),
        }
    }
}

impl Rows for Keyed {
    fn count(&self) -> usize {
        self.keys.len()
    }

    fn key(&self, index: usize) -> u64 {
        self.lookups.set(self.lookups.get() + 1);
        self.keys[index]
    }

    fn find(&self, key: u64) -> Option<usize> {
        self.lookups.set(self.lookups.get() + 1);
        self.at.get(&key).copied()
    }
}

const LIST_ROW: f32 = 26.0;
const VIEW: f32 = 10.0 * LIST_ROW;

fn list_id() -> Id {
    Id::ROOT.child("list")
}

/// Builds a frame of a list over `rows`, ten rows in view.
fn list_frame(
    ui: &mut Ui,
    rows: &Keyed,
    selected: &mut Option<u64>,
    keys: &[NamedKey],
) -> Option<usize> {
    let mut clicked = None;
    frame(ui, |ui| {
        let list = List {
            rows,
            row: LIST_ROW,
            keys,
            hover_selects: false,
        };
        let spec = Spec {
            size: [px(200.0), px(VIEW)],
            ..Spec::default()
        };
        clicked = crate::list(ui, list_id(), spec, list, selected, |ui, row| {
            ui.leaf(
                "label",
                Spec {
                    size: [fill(), fill()],
                    fill: row.selected.then_some([1.0; 4]),
                    ..Spec::default()
                },
            );
        });
    });
    clicked
}

fn row_top(ui: &Ui, key: u64) -> Option<f32> {
    ui.rect(list_id().child(key)).map(|rect| rect[1])
}

/// Frames until the list's animations end.
fn settle_list(ui: &mut Ui, rows: &Keyed, selected: &mut Option<u64>) {
    for _ in 0..40 {
        list_frame(ui, rows, selected, &[]);
    }
    assert!(!ui.wants_frame());
}

#[test]
fn lists_build_only_the_rows_in_view() {
    let mut boxes = Vec::new();
    for count in [20, 200_000] {
        let mut ui = Ui::new(Theme::dark(), DOUBLE_CLICK);
        let rows = Keyed::new(0..count);
        let mut selected = None;
        settle_list(&mut ui, &rows, &mut selected);
        rows.lookups.set(0);
        list_frame(&mut ui, &rows, &mut selected, &[]);
        assert!(rows.lookups.get() < 100, "{} lookups", rows.lookups.get());
        boxes.push(ui.nodes.len());
    }
    assert_eq!(boxes[0], boxes[1]);
}

#[test]
fn lists_hold_the_selection_still_as_items_arrive_and_leave_above() {
    let mut ui = Ui::new(Theme::dark(), DOUBLE_CLICK);
    let mut rows = Keyed::new(0..1000);
    let mut selected = Some(500);
    settle_list(&mut ui, &rows, &mut selected);
    let at = row_top(&ui, 500).unwrap();
    assert!(
        (0.0..VIEW).contains(&at),
        "a new selection scrolls into view"
    );
    rows = Keyed::new((10_000..10_100).chain(0..1000));
    list_frame(&mut ui, &rows, &mut selected, &[]);
    assert_eq!(row_top(&ui, 500), Some(at), "a hundred rows arrived above");
    rows = Keyed::new((0..1000).filter(|key| key % 2 == 1 || *key >= 500));
    list_frame(&mut ui, &rows, &mut selected, &[]);
    assert_eq!(row_top(&ui, 500), Some(at), "half the rows above left");
    ui.event(Event::PointerMoved([50.0, 50.0]));
    ui.event(Event::Wheel([0.0, -60.0]));
    list_frame(&mut ui, &rows, &mut selected, &[]);
    rows = Keyed::new((20_000..20_050).chain(rows.keys.iter().copied()));
    list_frame(&mut ui, &rows, &mut selected, &[]);
    assert_eq!(
        row_top(&ui, 500),
        Some(at - 60.0),
        "the wheel moves the rows at once, and they hold through arrivals"
    );
    settle_list(&mut ui, &rows, &mut selected);
    assert_eq!(row_top(&ui, 500), Some(at - 60.0));
}

#[test]
fn keys_move_the_selection_and_the_view_eases_after_it() {
    let mut ui = Ui::new(Theme::dark(), DOUBLE_CLICK);
    let rows = Keyed::new(0..1000);
    let mut selected = None;
    settle_list(&mut ui, &rows, &mut selected);
    list_frame(&mut ui, &rows, &mut selected, &[NamedKey::ArrowDown]);
    assert_eq!(selected, Some(0), "from nothing, the first row in view");
    list_frame(&mut ui, &rows, &mut selected, &[NamedKey::PageDown]);
    assert_eq!(selected, Some(10));
    list_frame(&mut ui, &rows, &mut selected, &[NamedKey::End]);
    assert_eq!(selected, Some(999));
    assert!(
        ui.wants_frame() && row_top(&ui, 999) != Some(VIEW - LIST_ROW),
        "the view eases towards it"
    );
    settle_list(&mut ui, &rows, &mut selected);
    assert_eq!(
        row_top(&ui, 999),
        Some(VIEW - LIST_ROW),
        "the last row at the bottom"
    );
    list_frame(
        &mut ui,
        &rows,
        &mut selected,
        &[NamedKey::PageUp, NamedKey::ArrowUp],
    );
    assert_eq!(selected, Some(988));
    list_frame(&mut ui, &rows, &mut selected, &[NamedKey::Home]);
    settle_list(&mut ui, &rows, &mut selected);
    assert_eq!((selected, row_top(&ui, 0)), (Some(0), Some(0.0)));
}

#[test]
fn a_menu_runs_to_the_window_edge_before_it_scrolls() {
    let names: Vec<String> = (0..20).map(|index| format!("Item {index}")).collect();
    let items: Vec<_> = names
        .iter()
        .map(|text| popup::Item {
            text,
            ..popup::Item::default()
        })
        .collect();
    let build = |ui: &mut Ui, height: f32| {
        sized_frame(ui, [400.0, height], |ui| {
            popup::menu(ui, menu_id(), BELOW, &items, None);
        })
    };
    let mut ui = Ui::new(Theme::dark(), DOUBLE_CLICK);
    build(&mut ui, 800.0);
    ui.open_popup(menu_id());
    for _ in 0..3 {
        build(&mut ui, 800.0);
    }
    let bar = |ui: &Ui| ui.rect(menu_id().child("rows").child("bar"));
    assert!(bar(&ui).is_none(), "twenty rows fit a tall window whole");
    assert!(ui.rect(menu_id().child("rows").child(19u64)).is_some());
    for _ in 0..3 {
        build(&mut ui, 300.0);
    }
    assert!(bar(&ui).is_some(), "a short window scrolls them");
}

#[test]
fn a_long_menu_scrolls_by_dragging_its_thumb() {
    let names: Vec<String> = (0..40).map(|index| format!("Item {index}")).collect();
    let items: Vec<_> = names
        .iter()
        .map(|text| popup::Item {
            text,
            ..popup::Item::default()
        })
        .collect();
    let build = |ui: &mut Ui| {
        frame(ui, |ui| {
            popup::menu(ui, menu_id(), BELOW, &items, None);
        })
    };
    let mut ui = Ui::new(Theme::dark(), DOUBLE_CLICK);
    build(&mut ui);
    ui.open_popup(menu_id());
    build(&mut ui);
    build(&mut ui);
    let thumb = ui.rect(menu_id().child("rows").child("bar")).unwrap();
    let top = ui
        .rect(menu_id().child("rows").child(0u64))
        .map(|rect| rect[1]);
    let at = Instant::now();
    let x = (thumb[0] + thumb[2]) / 2.0;
    ui.event(Event::PointerMoved([x, thumb[1] + 2.0]));
    press(&mut ui, at, true);
    build(&mut ui);
    ui.event(Event::PointerMoved([x, thumb[1] + 60.0]));
    build(&mut ui);
    build(&mut ui);
    assert!(ui.popup_open(menu_id()));
    assert_ne!(
        ui.rect(menu_id().child("rows").child(0u64))
            .map(|rect| rect[1]),
        top
    );
}

#[test]
fn a_long_menu_fades_the_ends_its_rows_are_cut_at_and_its_thumb_reaches_its_end() {
    let names: Vec<String> = (0..40).map(|index| format!("Item {index}")).collect();
    let items: Vec<_> = names
        .iter()
        .map(|text| popup::Item {
            text,
            ..popup::Item::default()
        })
        .collect();
    let build = |ui: &mut Ui| {
        frame(ui, |ui| {
            popup::menu(ui, menu_id(), BELOW, &items, None);
        })
    };
    let rows = menu_id().child("rows");
    let fade = |ui: &Ui| ui.nodes.iter().find(|node| node.id == rows).unwrap().fade;
    let mut ui = Ui::new(Theme::dark(), DOUBLE_CLICK);
    build(&mut ui);
    ui.open_popup(menu_id());
    build(&mut ui);
    build(&mut ui);
    assert_eq!(fade(&ui), [0.0, 12.0], "cut only at the bottom");
    let thumb = ui.rect(rows.child("bar")).unwrap();
    let x = (thumb[0] + thumb[2]) / 2.0;
    ui.event(Event::PointerMoved([x, thumb[1] + 2.0]));
    press(&mut ui, Instant::now(), true);
    build(&mut ui);
    ui.event(Event::PointerMoved([x, 1000.0]));
    build(&mut ui);
    build(&mut ui);
    assert_eq!(fade(&ui), [12.0, 0.0], "cut only at the top");
    let list = ui.rect(rows).unwrap();
    let thumb = ui.rect(rows.child("bar")).unwrap();
    assert_eq!(
        thumb[3],
        list[3] - 4.0,
        "the thumb ends as far from the bottom as it starts"
    );
}

#[test]
fn a_command_menu_opens_at_its_top_and_a_picker_on_its_current_value() {
    let names: Vec<String> = (0..40).map(|index| format!("Item {index}")).collect();
    let rows = menu_id().child("rows");
    for picker in [false, true] {
        let items: Vec<_> = names
            .iter()
            .enumerate()
            .map(|(index, text)| popup::Item {
                text,
                checked: Some(index == 30),
                current: picker && index == 30,
                ..popup::Item::default()
            })
            .collect();
        let build = |ui: &mut Ui| {
            frame(ui, |ui| {
                popup::menu(ui, menu_id(), BELOW, &items, None);
            })
        };
        let mut ui = Ui::new(Theme::dark(), DOUBLE_CLICK);
        build(&mut ui);
        ui.open_popup(menu_id());
        for _ in 0..3 {
            build(&mut ui);
        }
        assert_eq!(ui.popups[0].highlight, picker.then_some(30));
        let top = ui.rect(rows).unwrap()[1];
        let first = ui.rect(rows.child(0u64)).map(|rect| rect[1]);
        assert_eq!(first == Some(top), !picker, "{first:?} under {top}");
    }
}

const FONTS: [&str; 46] = [
    "American Typewriter",
    "Andale Mono",
    "Apple Braille",
    "Apple Chancery",
    "Arial",
    "Arial Black",
    "Arial Hebrew",
    "Arial Narrow",
    "Arial Rounded MT Bold",
    "Arial Unicode MS",
    "Avenir",
    "Baskerville",
    "Big Caslon",
    "Brush Script MT",
    "Chalkboard",
    "Charter",
    "Cochin",
    "Comic Sans MS",
    "Courier",
    "Courier New",
    "Didot",
    "Futura",
    "Geneva",
    "Georgia",
    "Gill Sans",
    "Helvetica",
    "Helvetica Neue",
    "Herculanum",
    "Hoefler Text",
    "Impact",
    "Lucida Grande",
    "Marker Felt",
    "Menlo",
    "Monaco",
    "Optima",
    "Palatino",
    "Papyrus",
    "Rockwell",
    "Savoye LET",
    "Skia",
    "Tahoma",
    "Times",
    "Times New Roman",
    "Trebuchet MS",
    "Verdana",
    "Zapfino",
];

fn fonts() -> Vec<popup::Item<'static>> {
    FONTS
        .iter()
        .map(|text| popup::Item {
            text,
            ..popup::Item::default()
        })
        .collect()
}

fn ranked(items: &[popup::Item], query: &str) -> Vec<&'static str> {
    let order = popup::Matches::new(items, query, 9.0).order;
    order.into_iter().map(|index| FONTS[index]).collect()
}

#[test]
fn fuzzy_matches_rank_whole_words_first_and_ties_keep_their_order() {
    let fonts = fonts();
    let family = [
        "Arial",
        "Arial Black",
        "Arial Hebrew",
        "Arial Narrow",
        "Arial Rounded MT Bold",
        "Arial Unicode MS",
    ];
    assert_eq!(ranked(&fonts, "Arial"), family);
    assert_eq!(ranked(&fonts, "rial"), family, "case aside, mid-word");
    let ari = ranked(&fonts, "ari");
    assert_eq!(ari[..6], family, "letters together before letters apart");
    assert_eq!(
        ari[6..],
        ["American Typewriter", "Apple Braille", "Baskerville"]
    );
    assert_eq!(
        ranked(&fonts, "tnr"),
        ["Times New Roman"],
        "words' initials"
    );
    assert_eq!(
        ranked(&fonts, "ar bl")[0],
        "Arial Black",
        "words in any order"
    );
    assert_eq!(
        ranked(&fonts, "ms"),
        [
            "Arial Unicode MS",
            "Comic Sans MS",
            "Trebuchet MS",
            "Times",
            "Times New Roman"
        ]
    );
    assert!(ranked(&fonts, "Cafe").is_empty());
}

#[test]
fn a_fallback_shows_only_when_a_query_leaves_nothing_else() {
    let mut items = fonts();
    let last = items.len() - 1;
    items[last].fallback = true;
    let order = |query| popup::Matches::new(&items, query, 9.0).order;
    assert!(!order("").contains(&last), "unfiltered");
    assert!(!order("Arial").contains(&last), "others match");
    assert_eq!(order("Cafe"), [last], "nothing else matches");
}

#[test]
fn typing_more_never_reveals_what_a_shorter_query_filtered_out() {
    let fonts = fonts();
    for query in [
        "arial black",
        "times new roman",
        "helvetica neue",
        "comic sans ms",
        "tnr",
    ] {
        let mut shown = ranked(&fonts, "");
        for end in 1..=query.len() {
            let narrower = ranked(&fonts, &query[..end]);
            for font in &narrower {
                assert!(shown.contains(font), "{:?} revealed {font}", &query[..end]);
            }
            shown = narrower;
        }
        assert!(!shown.is_empty(), "{query}");
    }
}

#[test]
fn a_filtered_menu_shows_its_new_results_at_once_from_the_top() {
    let fonts = fonts();
    let build = |ui: &mut Ui| {
        frame(ui, |ui| {
            popup::menu(ui, menu_id(), BELOW, &fonts, Some("Font"));
        })
    };
    let mut ui = Ui::new(Theme::dark(), DOUBLE_CLICK);
    build(&mut ui);
    ui.open_popup(menu_id());
    for _ in 0..12 {
        build(&mut ui);
    }
    let results = menu_id().child("rows");
    let height = |ui: &Ui| {
        let rect = ui.rect(results).unwrap();
        rect[3] - rect[1]
    };
    assert!(
        !ui.wants_frame() && height(&ui) > 5.0 * popup::MENU_ROW,
        "open at its full height"
    );
    ui.event(key(NamedKey::End));
    for _ in 0..40 {
        build(&mut ui);
    }
    // Each query shows its results where they settle in the frame it is typed.
    for query in ["t", "nr"] {
        ui.event(typed(query));
        build(&mut ui);
        let shown = [ui.rect(results), ui.rect(results.child(42_u64))];
        build(&mut ui);
        assert!(!ui.wants_frame(), "nothing eases");
        assert_eq!([ui.rect(results), ui.rect(results.child(42_u64))], shown);
    }
    let row = ui.rect(results.child(42_u64)).unwrap();
    assert_eq!(
        row[1],
        ui.rect(results).unwrap()[1],
        "Times New Roman at the top"
    );
    assert_eq!(height(&ui), popup::MENU_ROW);
}

/// A column `width` wide holding one label: its box, and the label as laid out.
fn fitted(
    ui: &mut Ui,
    text: &str,
    width: f32,
    overflow: Overflow,
    center: bool,
) -> ([f32; 4], Rc<Label>) {
    let mut id = None;
    frame(ui, |ui| {
        ui.open(
            "column",
            Spec {
                axis: Axis::Y,
                size: [px(width), children()],
                ..Spec::default()
            },
        );
        id = Some(ui.open(
            "label",
            Spec {
                size: [fill(), fit()],
                text: Some(text),
                overflow,
                center,
                pad: [4.0, 2.0],
                ..Spec::default()
            },
        ));
        ui.close();
        ui.close();
    });
    let node = ui.nodes.iter().find(|node| Some(node.id) == id).unwrap();
    (node.rect, node.label.clone().unwrap())
}

fn advances(label: &Label) -> Vec<f32> {
    label
        .layout
        .lines()
        .map(|line| line.metrics().advance)
        .collect()
}

#[test]
fn wrapped_labels_break_to_their_width_and_the_box_grows_to_their_height() {
    let mut ui = Ui::new(Theme::light(), DOUBLE_CLICK);
    let text = "Conflicting changes are highlighted in red. This page cannot be edited.";
    let (_, label) = fitted(&mut ui, text, 1000.0, Overflow::Wrap, false);
    assert_eq!(
        advances(&label).len(),
        1,
        "a label that fits stays one line"
    );
    let line = label.size[1];
    let mut previous = 2;
    for width in [320.0, 200.0, 120.0, 60.0] {
        let (rect, label) = fitted(&mut ui, text, width, Overflow::Wrap, false);
        let lines = advances(&label);
        assert!(lines.len() >= previous, "{width}: {lines:?}");
        assert!(
            lines.iter().all(|line| *line <= width - 8.0 + 0.01),
            "{width}: {lines:?}"
        );
        assert!((rect[3] - rect[1] - 4.0 - label.size[1]).abs() <= 0.5);
        assert!((label.size[1] / line - lines.len() as f32).abs() < 0.1);
        assert_eq!(rect[2] - rect[0], width);
        previous = lines.len();
    }
}

#[test]
fn wrapping_breaks_cjk_emoji_and_words_longer_than_the_line() {
    let mut ui = Ui::new(Theme::light(), DOUBLE_CLICK);
    for text in [
        "東京都の天気は晴れのち曇り、明日は雨が降るでしょう",
        "🎉🎈🎂🎁🎊🎉🎈🎂🎁🎊🎉🎈🎂🎁🎊",
        "Supercalifragilisticexpialidocious-antidisestablishmentarianism",
    ] {
        let lines = advances(&fitted(&mut ui, text, 90.0, Overflow::Wrap, false).1);
        assert!(lines.len() > 2, "{text}: {lines:?}");
        assert!(lines.iter().all(|line| *line <= 82.01), "{text}: {lines:?}");
    }
}

#[test]
fn centred_wrapped_lines_centre_across_the_box() {
    let mut ui = Ui::new(Theme::light(), DOUBLE_CLICK);
    let (_, label) = fitted(
        &mut ui,
        "A long line that wraps, then short",
        150.0,
        Overflow::Wrap,
        true,
    );
    let last = label.layout.lines().last().unwrap();
    let Some(parley::PositionedLayoutItem::GlyphRun(run)) = last.items().next() else {
        panic!("no glyphs");
    };
    let room = 150.0 - 8.0 - last.metrics().advance;
    assert!(
        room > 20.0 && (run.offset() - room / 2.0).abs() < 1.0,
        "{}",
        run.offset()
    );
}

#[test]
fn ellipsis_shortens_a_label_to_its_box_and_leaves_one_that_fits() {
    let mut ui = Ui::new(Theme::light(), DOUBLE_CLICK);
    let text = "Meeting notes from the quarterly planning review";
    let (whole, label) = fitted(&mut ui, text, 1000.0, Overflow::Ellipsis, false);
    assert_eq!(label.key.0, text);
    for width in [200.0, 90.0, 30.0] {
        let (rect, label) = fitted(&mut ui, text, width, Overflow::Ellipsis, false);
        let (shown, lines) = (&label.key.0, advances(&label));
        assert!(shown.ends_with('…') && !shown.ends_with(" …"), "{shown}");
        assert!(text.starts_with(shown.trim_end_matches('…')));
        assert_eq!(lines.len(), 1);
        assert!(lines[0] <= width - 8.0 + 0.01, "{width}: {shown} {lines:?}");
        assert_eq!(rect[3] - rect[1], whole[3] - whole[1]);
    }
    let (_, label) = fitted(
        &mut ui,
        "東京都の天気は晴れのち曇り",
        80.0,
        Overflow::Ellipsis,
        false,
    );
    assert!(
        label.key.0.ends_with('…') && label.key.0.chars().count() > 1,
        "{}",
        label.key.0
    );
}

#[test]
fn popups_ease_open_and_closed_then_stop_asking_for_frames() {
    let mut ui = Ui::new(Theme::light(), DOUBLE_CLICK);
    let id = Id::ROOT.child("dialog");
    let build = |ui: &mut Ui| {
        if ui.popup_open(id) {
            ui.leaf(
                "dialog",
                Spec {
                    size: [px(100.0), px(50.0)],
                    fill: Some([1.0; 4]),
                    anchor: Some(Anchor::Dialog),
                    ..Spec::default()
                },
            );
        }
    };
    let motion = |ui: &Ui| {
        ui.layers().iter().find_map(|layer| match layer {
            Layer::Primitives(primitives) => primitives.motion,
            Layer::Custom { .. } => None,
        })
    };
    frame(&mut ui, |ui| {
        ui.open_popup(id);
        build(ui);
    });
    let first = motion(&ui).unwrap();
    assert!(first.opacity < 0.1 && first.zoom < 0.96 && first.tilt > 0.15);
    assert_eq!(
        first.pivot,
        [200.0, 300.0 / 8.0],
        "the dialog swings from its top centre, an eighth down"
    );
    for _ in 0..16 {
        frame(&mut ui, build);
    }
    assert_eq!(motion(&ui).map(|motion| motion.opacity), Some(1.0));
    assert!(!ui.wants_frame());
    ui.close_popup(id);
    frame(&mut ui, build);
    frame(&mut ui, build);
    let closing = motion(&ui).unwrap();
    assert!(closing.opacity < 1.0 && closing.opacity > 0.0);
    assert!(ui.wants_frame());
    for _ in 0..10 {
        frame(&mut ui, build);
    }
    assert_eq!(motion(&ui), None);
    assert!(!ui.wants_frame());
}

#[test]
fn a_dragged_box_lands_past_the_middles_it_crossed_and_the_rest_slide_aside() {
    let spans = [[0.0, 40.0], [40.0, 60.0], [100.0, 20.0], [120.0, 50.0]];
    // Still over its own place, box 1 stays; past box 2's middle, it lands after it.
    assert_eq!(drop_slot(&spans, 1, 70.0), 1);
    assert_eq!(drop_slot(&spans, 1, 111.0), 2);
    assert_eq!(drop_slot(&spans, 1, 500.0), 3);
    assert_eq!(drop_slot(&spans, 3, -10.0), 0);
    assert_eq!(
        (0..4)
            .map(|index| slide(index, 1, 3, 60.0))
            .collect::<Vec<_>>(),
        [0.0, 0.0, -60.0, -60.0]
    );
    assert_eq!(
        (0..4)
            .map(|index| slide(index, 3, 1, 50.0))
            .collect::<Vec<_>>(),
        [0.0, 50.0, 50.0, 0.0]
    );
}

/// Clicks the middle of `id`'s box as laid out now, where it must be hit.
fn click_box(ui: &mut Ui, id: Id) {
    let rect = ui.rect(id).unwrap();
    let middle = [(rect[0] + rect[2]) / 2.0, (rect[1] + rect[3]) / 2.0];
    assert_eq!(
        ui.box_at(middle),
        Some(id),
        "the box is hit where it is laid out"
    );
    click(ui, middle);
}

/// What a dialog's controls reported in a frame.
#[derive(Default)]
struct Reported {
    checked: bool,
    ok: bool,
    scheme: Option<usize>,
}

/// A dialog holding a check box, a button, a field and a combo opening a menu of schemes.
fn dialog_frame(ui: &mut Ui, text: &mut String) -> Reported {
    let mut reported = Reported::default();
    frame(ui, |ui| {
        let dialog = Id::ROOT.child("dialog");
        if !ui.popup_open(dialog) {
            return;
        }
        ui.open_as(
            dialog,
            Spec {
                axis: Axis::Y,
                size: [px(240.0), children()],
                fill: Some([1.0; 4]),
                pad: [8.0, 8.0],
                anchor: Some(Anchor::Dialog),
                ..Spec::default()
            },
        );
        reported.checked = check_box(ui, "check", "Dark pages", false).clicked;
        reported.ok = button(ui, "ok", "OK").clicked;
        text_field(
            ui,
            Id::ROOT.child("name"),
            text,
            "",
            Spec {
                size: [fill(), px(26.0)],
                ..Spec::default()
            },
        );
        let combo = ui.id("combo");
        shell::combo(
            ui,
            "combo",
            "Color scheme",
            "Light",
            120.0,
            Id::ROOT.child("schemes"),
            true,
        );
        let items = ["System", "Light", "Dark"].map(|text| popup::Item {
            text,
            ..popup::Item::default()
        });
        let anchor = Anchor::Below(ui.rect(combo).unwrap_or_default());
        reported.scheme = popup::menu(ui, Id::ROOT.child("schemes"), anchor, &items, None);
        ui.close();
    });
    reported
}

#[test]
fn every_control_in_an_open_dialog_takes_clicks_where_it_rests() {
    let mut ui = Ui::new(Theme::light(), DOUBLE_CLICK);
    let mut text = String::new();
    let dialog = Id::ROOT.child("dialog");
    dialog_frame(&mut ui, &mut text);
    ui.open_popup(dialog);
    let settle = |ui: &mut Ui, text: &mut String| {
        for _ in 0..20 {
            dialog_frame(ui, text);
        }
        assert!(!ui.wants_frame());
    };
    settle(&mut ui, &mut text);
    click_box(&mut ui, dialog.child("check"));
    assert!(dialog_frame(&mut ui, &mut text).checked);
    click_box(&mut ui, dialog.child("ok"));
    assert!(dialog_frame(&mut ui, &mut text).ok);
    click_box(&mut ui, Id::ROOT.child("name"));
    dialog_frame(&mut ui, &mut text);
    assert_eq!(ui.focused(), Some(Id::ROOT.child("name")));
    ui.event(typed("a"));
    dialog_frame(&mut ui, &mut text);
    assert_eq!(text, "a");
    click_box(&mut ui, dialog.child("combo"));
    dialog_frame(&mut ui, &mut text);
    let schemes = Id::ROOT.child("schemes");
    assert!(ui.popup_open(schemes) && ui.popup_open(dialog));
    settle(&mut ui, &mut text);
    click_box(&mut ui, schemes.child("rows").child(2_u64));
    assert_eq!(dialog_frame(&mut ui, &mut text).scheme, Some(2));
    assert!(!ui.popup_open(schemes) && ui.popup_open(dialog));
}

#[test]
fn a_combo_s_field_widens_over_rows_laid_out_where_they_end_and_its_rows_take_clicks() {
    let mut ui = Ui::new(Theme::light(), DOUBLE_CLICK);
    let fonts = fonts();
    // Inset, as a popup keeps off the window's edges.
    let combo = Id::ROOT.child("inset").child("combo");
    let mut chosen = None;
    let mut build = |ui: &mut Ui| {
        frame(ui, |ui| {
            ui.open(
                "inset",
                Spec {
                    size: [children(), children()],
                    pad: [10.0, 10.0],
                    ..Spec::default()
                },
            );
            shell::combo(ui, "combo", "Font", "Calibri", 120.0, menu_id(), true);
            ui.close();
            let anchor = Anchor::Over(ui.rect(combo).unwrap_or_default());
            chosen = popup::menu(ui, menu_id(), anchor, &fonts, Some("Font"));
        });
        chosen
    };
    build(&mut ui);
    ui.open_popup(menu_id());
    build(&mut ui);
    let field = menu_id().child("filter");
    let [box_rect, first] = [ui.rect(combo).unwrap(), ui.rect(field).unwrap()];
    assert_eq!(first[..2], box_rect[..2], "the field starts on the box");
    assert!(first[2] - first[0] < box_rect[2] - box_rect[0] + 20.0);
    let shown = ui.rect(menu_id()).unwrap();
    let row = menu_id().child("rows").child(3_u64);
    let opening = ui.rect(row).unwrap();
    assert!(
        opening[2] > shown[2],
        "the rows lie out past the popup as it opens"
    );
    let layers = ui.layers();
    let moving: Vec<_> = layers
        .iter()
        .filter_map(|layer| match layer {
            Layer::Primitives(layer) => Some(layer),
            Layer::Custom { .. } => None,
        })
        .skip_while(|layer| layer.motion.is_none())
        .collect();
    let split = moving
        .iter()
        .position(|layer| layer.motion.is_none())
        .unwrap();
    assert!(
        moving[split..].iter().all(|layer| layer.motion.is_none()),
        "the field paints over the popup, which appears as one"
    );
    assert!(
        moving[..split]
            .iter()
            .filter(|layer| layer.round.is_some())
            .all(|layer| layer.round == Some((shown, ui.theme.menu().radius))),
        "the rows show only inside the popup's outline"
    );
    drop(layers);
    for _ in 0..20 {
        build(&mut ui);
    }
    assert!(!ui.wants_frame());
    let [panel, last] = [ui.rect(menu_id()).unwrap(), ui.rect(field).unwrap()];
    assert_eq!(last[2], panel[2] - 4.0, "the field widens across the popup");
    assert_eq!(ui.rect(row), Some(opening), "the rows never move");
    click_box(&mut ui, menu_id().child("rows").child(3_u64));
    assert_eq!(build(&mut ui), Some(3));
}

#[test]
fn a_popup_over_a_box_by_the_window_s_edge_widens_away_from_it() {
    let mut ui = Ui::new(Theme::light(), DOUBLE_CLICK);
    let fonts = fonts();
    let anchor = Anchor::Over([330.0, 10.0, 390.0, 30.0]);
    let build = |ui: &mut Ui| {
        frame(ui, |ui| {
            _ = popup::menu(ui, menu_id(), anchor, &fonts, Some("Font"))
        })
    };
    build(&mut ui);
    ui.open_popup(menu_id());
    build(&mut ui);
    let field = menu_id().child("filter");
    let edges =
        |ui: &Ui| [ui.rect(field).unwrap(), ui.rect(menu_id()).unwrap()].map(|rect| rect[2]);
    let first = ui.rect(field).unwrap();
    let row = menu_id().child("rows").child(3_u64);
    let opening = ui.rect(row).unwrap();
    assert_eq!(
        edges(&ui),
        [390.0, 394.0],
        "the trailing edges start on the box"
    );
    assert!(first[0] > 290.0, "the field starts near the box");
    for _ in 0..20 {
        build(&mut ui);
    }
    assert_eq!(edges(&ui), [390.0, 394.0], "the trailing edges hold");
    assert!(
        ui.rect(field).unwrap()[0] < first[0] - 30.0,
        "the field widens leftward"
    );
    assert_eq!(ui.rect(row), Some(opening), "the rows never move");
}

fn built(ui: &Ui, id: Id) -> &Built {
    ui.nodes.iter().find(|node| node.id == id).unwrap()
}

/// A split button at (100, 100) opening `split_menu()`; its signal.
fn split_frame(ui: &mut Ui) -> Signal {
    let mut signal = Signal::default();
    frame(ui, |ui| {
        ui.open(
            "bar",
            Spec {
                flags: Flags::FLOAT,
                size: [px(200.0), px(shell::TOOL)],
                position: [100.0, 100.0],
                ..Spec::default()
            },
        );
        signal = shell::split_button(ui, "split", "Split", CHECK_ICON, None, None, split_menu());
        let item = popup::Item {
            text: "Item",
            ..Default::default()
        };
        popup::menu(ui, split_menu(), BELOW, &[item], None);
        ui.close();
    });
    signal
}

const CHECK_ICON: &[&str] = popup::CHECK;

fn split_menu() -> Id {
    Id::ROOT.child("split menu")
}

fn split_part(part: &str) -> Id {
    Id::ROOT.child("bar").child("split").child(part)
}

fn center(rect: [f32; 4]) -> [f32; 2] {
    [(rect[0] + rect[2]) / 2.0, (rect[1] + rect[3]) / 2.0]
}

#[test]
fn a_split_button_fills_its_button_alone_and_outlines_both_halves_from_its_arrow() {
    let mut ui = Ui::new(Theme::light(), DOUBLE_CLICK);
    split_frame(&mut ui);
    let [button, arrow] = [split_part("button"), split_part("menu")];
    let face = button.child("face");
    let ring = split_part("ring");
    let settle = |ui: &mut Ui| {
        for _ in 0..60 {
            split_frame(ui);
        }
    };
    ui.event(Event::PointerMoved(center(ui.rect(button).unwrap())));
    settle(&mut ui);
    // The fill rounds past the button's clip, so it meets the arrow square.
    let [left, top, right, bottom] = ui.rect(button).unwrap();
    assert!(built(&ui, face).fill.is_some());
    assert_eq!(ui.rect(face), Some([left, top, right + 4.0, bottom]));
    assert!(built(&ui, button).flags.contains(Flags::CLIP));
    assert!(built(&ui, ring).border.is_none());
    ui.event(Event::PointerMoved(center(ui.rect(arrow).unwrap())));
    settle(&mut ui);
    assert!(built(&ui, face).fill.is_none());
    assert!(built(&ui, ring).border.is_some());
    assert_eq!(
        ui.rect(ring),
        Some([left, top, ui.rect(arrow).unwrap()[2], bottom])
    );
    // The arrow opens the menu, and the outline stays while it is open.
    let point = center(ui.rect(arrow).unwrap());
    click(&mut ui, point);
    assert!(!split_frame(&mut ui).clicked);
    assert!(ui.popup_open(split_menu()));
    ui.event(Event::PointerMoved([390.0, 290.0]));
    settle(&mut ui);
    assert!(built(&ui, ring).border.is_some());
    ui.close_popup(split_menu());
    settle(&mut ui);
    let point = center(ui.rect(button).unwrap());
    click(&mut ui, point);
    assert!(split_frame(&mut ui).clicked);
    assert!(!ui.popup_open(split_menu()));
}

#[test]
fn a_menu_button_opens_its_menu_with_the_icons_under_its_own() {
    let mut ui = Ui::new(Theme::light(), DOUBLE_CLICK);
    let items = [
        popup::Item {
            text: "Align left",
            icon: Some(CHECK_ICON),
            ..Default::default()
        },
        popup::Item {
            text: "Center",
            icon: Some(CHECK_ICON),
            current: true,
            ..Default::default()
        },
    ];
    let build = |ui: &mut Ui| {
        frame(ui, |ui| {
            ui.open(
                "bar",
                Spec {
                    flags: Flags::FLOAT,
                    size: [px(200.0), px(shell::TOOL)],
                    position: [100.0, 100.0],
                    ..Spec::default()
                },
            );
            let anchor = shell::menu_button(ui, "align", CHECK_ICON, None, menu_id());
            popup::menu(ui, menu_id(), anchor, &items, None);
            ui.close();
        });
    };
    build(&mut ui);
    let button = ui.rect(Id::ROOT.child("bar").child("align")).unwrap();
    // One target: the arrow opens the menu as the icon does.
    click(&mut ui, [button[2] - 3.0, center(button)[1]]);
    for _ in 0..40 {
        build(&mut ui);
    }
    assert!(ui.popup_open(menu_id()));
    let icon = ui
        .rect(
            menu_id()
                .child("rows")
                .child(0_usize)
                .child("item")
                .child("icon"),
        )
        .unwrap();
    assert_eq!(icon[0], button[0] + (shell::TOOL - ICON) / 2.0);
}

#[test]
fn galleries_choose_across_their_groups_by_keys_and_clicks() {
    let mut ui = Ui::new(Theme::dark(), DOUBLE_CLICK);
    let gallery = Id::ROOT.child("gallery");
    let groups = [("Recent", 2), ("Library", 5)].map(|(heading, cells)| popup::Group {
        heading,
        ruled: false,
        cells,
        columns: 3,
        size: [30.0, 30.0],
    });
    let build = |ui: &mut Ui| {
        let mut chosen = None;
        frame(ui, |ui| {
            chosen = popup::gallery(ui, gallery, BELOW, &groups, &[3], |ui, index| {
                ui.leaf(
                    "label",
                    Spec {
                        size: [fill(), fill()],
                        text: Some(["a", "b", "c", "d", "e", "f", "g"][index]),
                        ..Spec::default()
                    },
                );
            });
        });
        chosen
    };
    build(&mut ui);
    ui.open_popup(gallery);
    build(&mut ui);
    // Keys start from the current cell, the library's second.
    for named in [NamedKey::ArrowDown, NamedKey::ArrowLeft, NamedKey::Enter] {
        ui.event(key(named));
    }
    assert_eq!(build(&mut ui), Some(5));
    ui.open_popup(gallery);
    for _ in 0..40 {
        build(&mut ui);
    }
    let cell = |index: usize| ui.rect(gallery.child(("cell", index))).unwrap();
    // The library starts a row of its own under its heading.
    assert_eq!(cell(2)[0], cell(0)[0]);
    assert!(cell(2)[1] > cell(0)[3]);
    let point = center(cell(1));
    click(&mut ui, point);
    assert_eq!(build(&mut ui), Some(1));
}

#[test]
fn desktop_menus_cut_or_fade_without_growing() {
    let motion = |ui: &Ui| {
        ui.layers().iter().find_map(|layer| match layer {
            Layer::Primitives(primitives) => primitives.motion,
            Layer::Custom { .. } => None,
        })
    };
    let at = Anchor::Point([50.0, 50.0]);
    let styled = |motion| {
        let mut theme = Theme::light();
        theme.desktop_menu = Some(Menu {
            motion,
            ..theme.menu()
        });
        let mut ui = Ui::new(theme, DOUBLE_CLICK);
        menu_frame(&mut ui, at, None);
        ui.open_popup(menu_id());
        menu_frame(&mut ui, at, None);
        ui
    };

    let mut ui = styled(PopupMotion::Cut);
    let shown = motion(&ui).unwrap();
    assert_eq!([shown.opacity, shown.zoom, shown.tilt], [1.0, 1.0, 0.0]);
    ui.close_popup(menu_id());
    menu_frame(&mut ui, at, None);
    assert_eq!(motion(&ui), None, "gone the frame it closes");
    assert!(!ui.wants_frame());

    let mut ui = styled(PopupMotion::Fade([0.15, 0.6]));
    let first = motion(&ui).unwrap();
    assert!(first.opacity < 0.2 && first.zoom == 1.0 && first.tilt == 0.0);
    for _ in 0..10 {
        menu_frame(&mut ui, at, None);
    }
    assert_eq!(motion(&ui).map(|motion| motion.opacity), Some(1.0));
    ui.close_popup(menu_id());
    // 96 ms into the fade out, OutQuart leaves about half.
    for _ in 0..7 {
        menu_frame(&mut ui, at, None);
    }
    let closing = motion(&ui).unwrap();
    assert!((closing.opacity - (1.0 - 0.096 / 0.6_f32).powi(4)).abs() < 0.01);
    assert!(ui.wants_frame());
}

/// A row 200 wide of twelve section tabs with `active` open; returns the row's id.
fn tab_row(ui: &mut Ui, active: usize) -> Id {
    let names = [
        "One", "Two", "Three", "Four", "Five", "Six", "Seven", "Eight", "Nine", "Ten", "Eleven",
        "Twelve",
    ];
    let tabs: Vec<_> = names
        .iter()
        .map(|name| (*name, [0.5, 0.6, 0.9, 1.0], false))
        .collect();
    let section = ui.theme.section([0.5, 0.6, 0.9, 1.0]);
    let row = Id::ROOT.child("tabs");
    frame(ui, |ui| {
        ui.open(
            "tab row",
            Spec {
                size: [px(200.0), px(28.0)],
                ..Spec::default()
            },
        );
        shell::section_tabs(ui, row, &tabs, active, None, None, &section, 28.0, [0.0; 4]);
        ui.close();
    });
    row
}

/// Frames until the tabs' easing settles.
fn settle_tabs(ui: &mut Ui, active: usize) -> Id {
    let mut row = tab_row(ui, active);
    for _ in 0..60 {
        row = tab_row(ui, active);
    }
    row
}

#[test]
fn overflowing_tabs_scroll_the_open_one_into_view() {
    let mut ui = Ui::new(Theme::dark(), DOUBLE_CLICK);
    let row = settle_tabs(&mut ui, 0);
    let view = ui.rect(row).unwrap();
    assert_eq!(view[2] - view[0], 200.0);
    let row = settle_tabs(&mut ui, 11);
    let last = ui.rect(shell::tab_id(row, 11)).unwrap();
    assert!(
        last[0] >= view[0] && last[2] <= view[2],
        "{last:?} in {view:?}"
    );
}

#[test]
fn the_wheel_scrolls_overflowing_tabs_sideways_either_way_it_turns() {
    let mut ui = Ui::new(Theme::dark(), DOUBLE_CLICK);
    let row = settle_tabs(&mut ui, 0);
    let first = ui.rect(shell::tab_id(row, 0)).unwrap()[0];
    ui.event(Event::PointerMoved([100.0, 14.0]));
    ui.event(Event::Wheel([0.0, -40.0]));
    tab_row(&mut ui, 0);
    tab_row(&mut ui, 0);
    let scrolled = ui.rect(shell::tab_id(row, 0)).unwrap()[0];
    assert_eq!(first - scrolled, 40.0, "at once, as on the page");
    ui.event(Event::Wheel([30.0, 0.0]));
    tab_row(&mut ui, 0);
    tab_row(&mut ui, 0);
    let back = ui.rect(shell::tab_id(row, 0)).unwrap()[0];
    assert_eq!(back - scrolled, 30.0);
}

#[test]
fn a_colour_picker_applies_its_colour_and_keeps_it_through_hsl() {
    let mut ui = Ui::new(Theme::light(), DOUBLE_CLICK);
    let picker = Id::ROOT.child("picker");
    let build = |ui: &mut Ui| {
        let mut chosen = None;
        frame(ui, |ui| {
            chosen = popup::color_picker(
                ui,
                picker,
                BELOW,
                "Custom Color",
                [0xd4, 0xf9, 0xf2],
                |_| [1.0; 4],
            );
        });
        chosen
    };
    build(&mut ui);
    ui.open_popup(picker);
    assert_eq!(build(&mut ui), None);
    ui.event(key(NamedKey::Enter));
    assert_eq!(build(&mut ui), Some([0xd4, 0xf9, 0xf2]));
    assert!(!ui.popup_open(picker));
}

#[test]
fn password_fields_edit_the_characters_behind_their_bullets() {
    let mut ui = Ui::new(Theme::dark(), DOUBLE_CLICK);
    let mut text = String::new();
    let edit = |ui: &mut Ui, text: &mut String| {
        frame(ui, |ui| {
            password_field(
                ui,
                field(),
                text,
                "",
                Spec {
                    size: [px(200.0), px(26.0)],
                    pad: [6.0, 0.0],
                    ..Spec::default()
                },
            );
        })
    };
    edit(&mut ui, &mut text);
    ui.set_focus(Some(field()));
    for typed_text in ["p", "ä", "ß"] {
        ui.event(typed(typed_text));
    }
    edit(&mut ui, &mut text);
    ui.event(key(NamedKey::ArrowLeft));
    ui.event(key(NamedKey::ArrowLeft));
    ui.event(typed("x"));
    ui.event(key(NamedKey::Delete));
    edit(&mut ui, &mut text);
    assert_eq!(text, "pxß");
    ui.event(key(NamedKey::Backspace));
    edit(&mut ui, &mut text);
    assert_eq!(text, "pß");
}

#[test]
fn a_loose_dialog_fits_its_contents_to_the_window_and_its_side_stretches_to_its_list() {
    let dialog = Id::ROOT.child("dialog");
    let side = dialog.child("body").child("side");
    let list = dialog.child("body").child("list");
    let loose = Extent {
        size: Size::Children,
        strictness: 0.0,
    };
    let build = |ui: &mut Ui, rows: usize| {
        frame(ui, |ui| {
            ui.open_as(
                dialog,
                Spec {
                    axis: Axis::Y,
                    size: [px(300.0), loose],
                    anchor: Some(Anchor::Dialog),
                    ..Spec::default()
                },
            );
            ui.leaf(
                "header",
                Spec {
                    size: [fill(), px(30.0)],
                    ..Spec::default()
                },
            );
            ui.open(
                "body",
                Spec {
                    size: [fill(), loose],
                    ..Spec::default()
                },
            );
            ui.leaf(
                "side",
                Spec {
                    size: [px(100.0), fill()],
                    ..Spec::default()
                },
            );
            ui.open(
                "list",
                Spec {
                    flags: Flags::SCROLL | Flags::CLIP,
                    axis: Axis::Y,
                    size: [fill(), loose],
                    ..Spec::default()
                },
            );
            for row in 0..rows {
                ui.leaf(
                    row,
                    Spec {
                        size: [fill(), px(30.0)],
                        ..Spec::default()
                    },
                );
            }
            ui.close();
            ui.close();
            ui.leaf(
                "footer",
                Spec {
                    size: [fill(), px(30.0)],
                    ..Spec::default()
                },
            );
            ui.close();
        });
    };
    let height = |ui: &Ui, id: Id| ui.rect(id).map(|rect| rect[3] - rect[1]).unwrap();
    let mut ui = Ui::new(Theme::dark(), DOUBLE_CLICK);
    build(&mut ui, 2);
    ui.open_popup(dialog);
    for _ in 0..40 {
        build(&mut ui, 2);
    }
    assert_eq!(height(&ui, dialog), 120.0, "as tall as what it holds");
    assert_eq!(height(&ui, side), 60.0, "the side as tall as the list");

    for _ in 0..40 {
        build(&mut ui, 20);
    }
    assert_eq!(
        height(&ui, dialog),
        225.0,
        "no taller than the window lets a dialog be"
    );
    assert_eq!(height(&ui, list), 165.0, "the list gives way");
    assert_eq!(height(&ui, side), 165.0);

    ui.scroll_to(list, list.child(10_usize));
    for _ in 0..60 {
        build(&mut ui, 20);
    }
    let [top, row] = [list, list.child(10_usize)].map(|id| ui.rect(id).unwrap()[1]);
    assert!(
        (row - top).abs() < 0.5,
        "row 10 at {row}, the list's top at {top}"
    );
}

#[test]
fn a_box_squeezed_past_its_inset_paints_empty_not_inverted() {
    let mut ui = Ui::new(Theme::light(), DOUBLE_CLICK);
    frame(&mut ui, |ui| {
        ui.leaf(
            "row",
            Spec {
                size: [px(100.0), px(4.0)],
                fill: Some([1.0; 4]),
                gradient: Some([0.5, 0.5, 0.5, 1.0]),
                inset: [0.0, 0.0, 0.0, 10.0],
                ..Spec::default()
            },
        );
    });
    let rect = ui.display.iter().find_map(|item| match item {
        Display::Rect { rect, .. } => Some(*rect),
        _ => None,
    });
    let [left, top, right, bottom] = rect.expect("the row paints");
    assert!(
        right >= left && bottom >= top,
        "{:?}",
        [left, top, right, bottom]
    );
}

#[test]
fn a_box_has_no_rectangle_until_it_is_first_laid_out() {
    let mut ui = Ui::new(Theme::light(), DOUBLE_CLICK);
    let build = |ui: &mut Ui| {
        ui.leaf(
            "new",
            Spec {
                size: [px(40.0), px(20.0)],
                ..Spec::default()
            },
        );
        ui.rect(Id::ROOT.child("new"))
    };
    let mut seen = Vec::new();
    frame(&mut ui, |ui| seen.push(build(ui)));
    frame(&mut ui, |ui| seen.push(build(ui)));
    assert_eq!(
        seen[0], None,
        "a box placed against nothing would sit at the origin"
    );
    assert_eq!(seen[1], Some([0.0, 0.0, 40.0, 20.0]));
}
