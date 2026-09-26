use super::*;
use std::time::Duration;
use winit::keyboard::NamedKey;

const DOUBLE_CLICK: Duration = Duration::from_millis(500);

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
    let (texts, frame) = ui.texts();
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
    ui.event(Event::Modifiers(ModifiersState::SUPER));
    ui.event(typed("a"));
    field_frame(&mut ui, &mut text);
    ui.event(Event::Modifiers(ModifiersState::empty()));
    ui.event(key(NamedKey::Delete));
    let signal = field_frame(&mut ui, &mut text);
    assert!(signal.focused);
    assert_eq!(text, "");
}

#[test]
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
fn popups_fade_in_except_at_a_point() {
    let mut ui = Ui::new(Theme::dark(), DOUBLE_CLICK);
    menu_frame(&mut ui, BELOW, None);
    ui.open_popup(menu_id());
    menu_frame(&mut ui, BELOW, None);
    menu_frame(&mut ui, BELOW, None);
    let reveal = |ui: &Ui| {
        ui.nodes
            .iter()
            .find(|node| node.id == menu_id())
            .unwrap()
            .alpha
    };
    assert!(reveal(&ui) > 0.0 && reveal(&ui) < 1.0);
    assert!(
        ui.rect(menu_id()).unwrap()[1] < 44.0,
        "and slide from the anchor"
    );
    assert!(ui.wants_frame());
    for _ in 0..10 {
        menu_frame(&mut ui, BELOW, None);
    }
    assert!(reveal(&ui) > 0.95, "within about 150 ms");
    ui.close_popup(menu_id());
    menu_frame(&mut ui, Anchor::Point([50.0, 50.0]), None);
    ui.open_popup(menu_id());
    menu_frame(&mut ui, Anchor::Point([50.0, 50.0]), None);
    menu_frame(&mut ui, Anchor::Point([50.0, 50.0]), None);
    assert_eq!(reveal(&ui), 1.0, "a context menu shows at once");
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
    assert_eq!(shown(&ui), [3, 0], "letters together before letters apart");
    ui.event(key(NamedKey::Backspace));
    settle(&mut ui);
    assert_eq!(shown(&ui), [0, 1, 3], "a prefix before a word's start");
    ui.event(key(NamedKey::ArrowDown));
    ui.event(key(NamedKey::Enter));
    assert_eq!(menu_frame(&mut ui, BELOW, Some("Filter")).1, Some(1));
}

#[test]
fn nothing_matching_leaves_nothing_to_choose() {
    let mut ui = open_menu(Some("Filter"));
    ui.event(typed("z"));
    menu_frame(&mut ui, BELOW, Some("Filter"));
    ui.event(key(NamedKey::ArrowDown));
    ui.event(key(NamedKey::Enter));
    let (_, chosen) = menu_frame(&mut ui, BELOW, Some("Filter"));
    assert_eq!(chosen, None);
    assert!(ui.popup_open(menu_id()));
}

#[test]
fn the_palette_centres_across_the_window_and_chooses_the_best_match() {
    let mut ui = Ui::new(Theme::dark(), DOUBLE_CLICK);
    let palette = Id::ROOT.child("palette");
    let commands = items();
    let build = |ui: &mut Ui| {
        let mut chosen = None;
        frame(ui, |ui| chosen = popup::palette(ui, palette, &commands));
        chosen
    };
    build(&mut ui);
    ui.open_popup(palette);
    for _ in 0..40 {
        build(&mut ui);
    }
    let rect = ui.rect(palette).unwrap();
    assert_eq!(rect[0] + rect[2], 400.0, "centred");
    ui.event(typed("a"));
    ui.event(key(NamedKey::Enter));
    assert_eq!(
        build(&mut ui),
        Some(3),
        "`all` begins a word; `Paste` is disabled"
    );
}

#[test]
fn colour_grids_move_in_two_dimensions_and_choose_a_swatch_or_none() {
    let mut ui = Ui::new(Theme::dark(), DOUBLE_CLICK);
    let grid = Id::ROOT.child("colours");
    let swatches: Vec<[f32; 4]> = (0..6).map(|index| [index as f32, 0.0, 0.0, 1.0]).collect();
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
    assert_eq!(build(&mut ui), Some(Some(swatches[4])));
    ui.open_popup(grid);
    for _ in 0..40 {
        build(&mut ui);
    }
    let rect = ui.rect(grid.child(("cell", 0_usize))).unwrap();
    click(&mut ui, [rect[0] + 4.0, rect[1] + 4.0]);
    assert_eq!(build(&mut ui), Some(None));
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
    let moving = row_top(&ui, 500).unwrap();
    assert!(
        moving < at && moving > at - 60.0,
        "the wheel eases on through arrivals"
    );
    settle_list(&mut ui, &rows, &mut selected);
    assert_eq!(row_top(&ui, 500), Some(at - 60.0));
}

fn alpha(ui: &Ui, key: u64) -> Option<f32> {
    let id = list_id().child(key);
    ui.nodes
        .iter()
        .find(|node| node.id == id)
        .map(|node| node.alpha)
}

#[test]
fn rows_keeping_their_order_slide_and_the_rest_fade_through() {
    let mut ui = Ui::new(Theme::dark(), DOUBLE_CLICK);
    let mut selected = None;
    let rows = Keyed::new(0..20);
    settle_list(&mut ui, &rows, &mut selected);
    let rows = Keyed::new([5, 0, 1, 2, 3, 100, 6, 7, 8, 9, 10, 11]);
    list_frame(&mut ui, &rows, &mut selected, &[]);
    let crossing = row_top(&ui, 5).unwrap();
    assert!(crossing < 5.0 * LIST_ROW && crossing > 4.5 * LIST_ROW);
    assert!(alpha(&ui, 5).unwrap() < 1.0, "fading as it sets off");
    assert_eq!(row_top(&ui, 0), Some(0.0), "sliding waits for rows leaving");
    assert!(alpha(&ui, 4).unwrap() < 1.0);
    assert_eq!(alpha(&ui, 100), Some(0.0), "entering waits too");
    for _ in 0..4 {
        list_frame(&mut ui, &rows, &mut selected, &[]);
    }
    assert_eq!(alpha(&ui, 5), Some(0.0), "unseen half-way");
    assert_eq!(alpha(&ui, 4), Some(0.0));
    let sliding = row_top(&ui, 0).unwrap();
    assert!(sliding > 0.0 && sliding < LIST_ROW);
    assert_eq!(
        alpha(&ui, 100),
        Some(0.0),
        "entering waits for rows sliding"
    );
    for _ in 0..5 {
        list_frame(&mut ui, &rows, &mut selected, &[]);
    }
    assert_eq!(row_top(&ui, 5), Some(0.0), "there in about 150 ms");
    assert_eq!(row_top(&ui, 0), Some(LIST_ROW));
    assert_eq!(alpha(&ui, 100), Some(1.0));
    assert_eq!(alpha(&ui, 4), None);
    list_frame(&mut ui, &rows, &mut selected, &[]);
    assert!(!ui.wants_frame());
}

/// Two rows whose text is drawn over each other: their labels, centred in each row, meet
/// within the view while both can be seen.
fn text_overlaps(ui: &mut Ui) -> Option<String> {
    let text = ui.measure("Ag")[1];
    let list = ui.nodes.iter().position(|node| node.id == list_id())?;
    let rows: Vec<_> = ui
        .nodes
        .iter()
        // Below half an 8-bit step, a row changes no pixel.
        .filter(|node| {
            node.parent == list && node.size[1] == px(LIST_ROW) && node.alpha * 255.0 >= 0.5
        })
        .map(|node| {
            let middle = (node.rect[1] + node.rect[3]) / 2.0;
            let [top, bottom] =
                [middle - text / 2.0, middle + text / 2.0].map(|y| y.clamp(0.0, VIEW));
            (top, bottom, node.alpha)
        })
        .collect();
    rows.iter().enumerate().find_map(|(at, a)| {
        rows[at + 1..]
            .iter()
            .find(|b| a.0 < b.1 && b.0 < a.1)
            .map(|b| format!("{a:?} over {b:?}"))
    })
}

#[test]
fn rows_never_draw_text_over_each_other() {
    let orders: [Vec<u64>; 7] = [
        (0..30).collect(),
        (4..12).chain([2, 3, 16]).chain(12..16).collect(),
        (0..30).rev().collect(),
        [7, 3, 12, 0, 1, 25, 26, 2, 4, 5].into(),
        (0..30)
            .filter(|key| !(3..6).contains(key) && *key != 8)
            .collect(),
        (0..3).chain(40..44).chain(3..30).collect(),
        [1, 0, 3, 2, 5, 4, 7, 6, 9, 8, 50].into(),
    ];
    for (at, order) in orders.iter().enumerate() {
        let mut ui = Ui::new(Theme::dark(), DOUBLE_CLICK);
        let mut selected = None;
        settle_list(&mut ui, &Keyed::new(0..30), &mut selected);
        // Each change, and another landing half-way through it.
        let next = &orders[(at + 1) % orders.len()];
        for (frame, rows) in (0..30).map(|frame| {
            let order = if frame < 5 { order } else { next };
            (frame, Keyed::new(order.iter().copied()))
        }) {
            list_frame(&mut ui, &rows, &mut selected, &[]);
            if let Some(overlap) = text_overlaps(&mut ui) {
                panic!("order {at}, frame {frame}: {overlap}");
            }
        }
    }
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
