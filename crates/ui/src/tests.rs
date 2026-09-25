use super::*;
use std::time::Duration;
use winit::keyboard::NamedKey;

thread_local! {
    static START: Instant = Instant::now();
    static FRAMES: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// Builds a frame 16 ms after this thread's previous one.
fn frame(ui: &mut Ui, build: impl FnOnce(&mut Ui)) {
    let frames = FRAMES.with(|frames| {
        frames.set(frames.get() + 1);
        frames.get()
    });
    let now = START.with(|start| *start) + Duration::from_millis(16) * frames;
    ui.begin([400.0, 300.0], 2.0, now);
    build(ui);
    ui.end();
}

#[test]
fn fill_yields_to_strict_siblings_along_the_flow() {
    let mut ui = Ui::new(Theme::dark());
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
fn children_sum_with_gaps_and_padding() {
    let mut ui = Ui::new(Theme::dark());
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
    let mut ui = Ui::new(Theme::dark());
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

#[test]
fn custom_boxes_receive_their_events_and_a_leave_when_the_pointer_moves_off() {
    let mut ui = Ui::new(Theme::dark());
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
    ui.event(Event::PointerMoved([350.0, 20.0]));
    let signals = build(&mut ui);
    assert_eq!(
        signals[0].events,
        [Event::PointerMoved([350.0, 20.0])],
        "a drag keeps its box"
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
    let mut ui = Ui::new(Theme::dark());
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
    for _ in 0..60 {
        build(&mut ui);
    }
    assert_eq!(ui.states[&list.unwrap()].scroll, 200.0);
    assert_eq!(ui.rect(last.unwrap()), Some([0.0, 70.0, 100.0, 100.0]));
    let layers = ui.layers();
    assert!(matches!(
        layers[..],
        [Layer::Primitives {
            clip: Some([0.0, 0.0, 100.0, 100.0]),
            ..
        }] | []
    ));
}

#[test]
fn text_fields_edit_with_keys_and_selection() {
    let mut ui = Ui::new(Theme::dark());
    let mut text = String::from("café");
    let build = |ui: &mut Ui, text: &mut String| {
        let mut signal = Signal::default();
        frame(ui, |ui| {
            signal = text_field(
                ui,
                Id::ROOT.child("filter"),
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
    };
    build(&mut ui, &mut text);
    let id = Id::ROOT.child("filter");
    ui.set_focus(Some(id));
    let key = |named| Event::Key {
        key: Key::Named(named),
        text: None,
    };
    ui.event(Event::Key {
        key: Key::Named(NamedKey::End),
        text: None,
    });
    ui.event(key(NamedKey::Backspace));
    ui.event(Event::Key {
        key: Key::Character("e".into()),
        text: Some("e".into()),
    });
    build(&mut ui, &mut text);
    assert_eq!(text, "cafe");
    ui.event(Event::Modifiers(ModifiersState::SHIFT));
    ui.event(key(NamedKey::ArrowLeft));
    ui.event(key(NamedKey::ArrowLeft));
    build(&mut ui, &mut text);
    ui.event(Event::Modifiers(ModifiersState::empty()));
    ui.event(Event::Ime(Ime::Commit("é".into())));
    build(&mut ui, &mut text);
    assert_eq!(text, "caé");
    ui.event(Event::Modifiers(ModifiersState::SUPER));
    ui.event(Event::Key {
        key: Key::Character("a".into()),
        text: Some("a".into()),
    });
    build(&mut ui, &mut text);
    ui.event(Event::Modifiers(ModifiersState::empty()));
    ui.event(key(NamedKey::Delete));
    let signal = build(&mut ui, &mut text);
    assert!(signal.focused);
    assert_eq!(text, "");
}

#[test]
fn scrollbar_thumbs_track_the_offset_and_drags_reach_both_ends() {
    let mut ui = Ui::new(Theme::dark());
    let mut offset = 0.0;
    let build = |ui: &mut Ui, offset: &mut f32| {
        frame(ui, |ui| {
            ui.open(
                "view",
                Spec {
                    flags: Flags::CUSTOM,
                    size: [px(200.0), px(220.0)],
                    ..Spec::default()
                },
            );
            if let Some(chosen) =
                scrollbar(ui, "bar", Axis::Y, *offset, [0.0, 1000.0], 400.0, [0.5; 4])
            {
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
