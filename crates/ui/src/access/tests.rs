use crate::{shell, *};
use accesskit::{Action, ActionRequest, NodeId, Role, TreeId};
use accesskit_consumer::{NodeRef, Tree, common_filter};
use std::time::Duration;
use winit::keyboard::NamedKey;

thread_local! {
    static START: Instant = Instant::now();
    static FRAMES: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

fn new_ui() -> Ui {
    Ui::new(Theme::dark(), Duration::from_millis(500))
}

/// Builds a frame 16 ms after this thread's previous one.
fn frame(ui: &mut Ui, build: impl FnOnce(&mut Ui)) {
    let frames = FRAMES.with(|frames| {
        frames.set(frames.get() + 1);
        frames.get()
    });
    let now = START.with(|start| *start) + Duration::from_millis(16) * frames;
    ui.begin([600.0, 300.0], 2.0, now);
    build(ui);
    ui.end();
}

/// The tree as the platform sees it, generic containers left out: a line per node of its
/// role, name, value, states, shortcut and actions, with the focus marked.
fn snapshot(tree: &Tree) -> String {
    fn write(node: &NodeRef, depth: usize, out: &mut String) {
        let data = node.data();
        let mut line = format!("{}{:?}", "  ".repeat(depth), node.role());
        if let Some(label) = data.label() {
            line += &format!(" {label:?}");
        }
        if let Some(value) = data.value() {
            line += &format!(" = {value:?}");
        }
        if let Some(placeholder) = data.placeholder() {
            line += &format!(" ({placeholder})");
        }
        for (state, on) in [
            ("toggled", data.toggled() == Some(accesskit::Toggled::True)),
            (
                "untoggled",
                data.toggled() == Some(accesskit::Toggled::False),
            ),
            ("expanded", data.is_expanded() == Some(true)),
            ("collapsed", data.is_expanded() == Some(false)),
            ("selected", data.is_selected() == Some(true)),
            ("disabled", data.is_disabled()),
            ("focused", node.is_focused()),
        ] {
            if on {
                line += &format!(" [{state}]");
            }
        }
        if let Some(keys) = data.keyboard_shortcut() {
            line += &format!(" <{keys}>");
        }
        if let Some(description) = data.description() {
            line += &format!(" -- {description}");
        }
        let actions: Vec<_> = [
            (Action::Click, "click"),
            (Action::Focus, "focus"),
            (Action::SetValue, "set"),
        ]
        .into_iter()
        .filter(|(action, _)| data.supports_action(*action))
        .map(|(_, name)| name)
        .collect();
        if !actions.is_empty() {
            line += &format!(" {{{}}}", actions.join(" "));
        }
        out.push_str(&line);
        out.push('\n');
        for child in node.filtered_children(common_filter) {
            write(&child, depth + 1, out);
        }
    }
    let mut out = String::new();
    write(&tree.state().root(), 0, &mut out);
    out
}

/// The tree built from scratch, as assistive technology first asks for it.
fn tree(ui: &mut Ui) -> Tree {
    ui.deactivate_accessibility();
    Tree::new(ui.accessibility("Notes", 2.0), true)
}

fn key(ui: &mut Ui, named: NamedKey) {
    ui.event(Event::Key {
        key: winit::keyboard::Key::Named(named),
        text: None,
    });
}

fn toolbar_id() -> Id {
    Id::ROOT.child("toolbar")
}

fn font_menu() -> Id {
    Id::ROOT.child("font menu")
}

fn more_menu() -> Id {
    Id::ROOT.child("more menu")
}

/// A toolbar of the kit's controls, as the app builds its own; returns what was clicked.
fn toolbar(ui: &mut Ui, bold: bool) -> Vec<&'static str> {
    let mut clicked = Vec::new();
    let text = ui.theme.text;
    ui.open_as(
        toolbar_id(),
        Spec {
            size: [fill(), px(30.0)],
            gap: 4.0,
            role: Some(Role::Toolbar),
            ..Spec::default()
        },
    );
    if shell::tool_button(ui, "undo", shell::CHEVRON, text, None).clicked {
        clicked.push("undo");
    }
    popup::tooltip(ui, "Undo", "⌘Z", None);
    shell::unavailable(ui, "cut", shell::CHEVRON, text, false);
    popup::tooltip(ui, "Cut", "⌘X", None);
    if shell::tool_button(ui, "bold", shell::CHEVRON, text, Some(bold)).clicked {
        clicked.push("bold");
    }
    popup::tooltip(ui, "Bold", "⌘B", Some("Makes the selected text bold."));
    if shell::split_button(
        ui,
        "paste",
        "Paste",
        shell::CHEVRON,
        None,
        None,
        more_menu(),
    )
    .clicked
    {
        clicked.push("paste");
    }
    popup::tooltip(ui, "Paste", "⌘V", None);
    let combo = ui.id("font");
    shell::combo(ui, "font", "Font", "Calibri", 120.0, font_menu(), true);
    let level = ui.open(
        "level",
        Spec {
            flags: Flags::CLICKABLE,
            size: [px(44.0), px(22.0)],
            text: Some("100%"),
            role: Some(Role::Button),
            ..Spec::default()
        },
    );
    ui.close();
    popup::tooltip(ui, "Actual Size", "", None);
    if let Some(node) = ui.access(level) {
        node.set_value("100%");
    }
    ui.close();
    let items = [
        popup::Item {
            text: "Recent",
            heading: true,
            ..popup::Item::default()
        },
        popup::Item {
            text: "Calibri",
            checked: Some(true),
            current: true,
            ..popup::Item::default()
        },
        popup::Item {
            text: "Arial",
            ..popup::Item::default()
        },
        popup::Item {
            text: "Wingdings",
            disabled: true,
            ..popup::Item::default()
        },
    ];
    let anchor = Anchor::Over(ui.rect(combo).unwrap_or_default());
    popup::menu(ui, font_menu(), anchor, &items, Some("Font"));
    let paste = [popup::Item {
        text: "Keep Text Only",
        shortcut: "⌘⇧V",
        ..popup::Item::default()
    }];
    let anchor = Anchor::Below(ui.rect(ui.id("paste")).unwrap_or_default());
    popup::menu(ui, more_menu(), anchor, &paste, None);
    clicked
}

#[test]
fn the_toolbar_names_its_controls_from_their_tooltips() {
    let mut ui = new_ui();
    for _ in 0..2 {
        frame(&mut ui, |ui| {
            toolbar(ui, true);
        });
    }
    assert_eq!(
        snapshot(&tree(&mut ui)),
        r#"Window "Notes" [focused]
  Toolbar
    Button "Undo" <⌘Z> {click focus}
    Button "Cut" [disabled] <⌘X>
    Button "Bold" [toggled] <⌘B> -- Makes the selected text bold. {click focus}
    Button "Paste" <⌘V> {click focus}
    Button "Paste Options" [collapsed] {click focus}
    ComboBox "Font" = "Calibri" [collapsed] {click focus}
    Button "Actual Size" = "100%" {click focus}
"#
    );
}

#[test]
fn each_toolbar_control_has_a_name_of_its_own() {
    let mut ui = new_ui();
    for _ in 0..2 {
        frame(&mut ui, |ui| {
            toolbar(ui, true);
        });
    }
    let tree = tree(&mut ui);
    let toolbar = tree
        .state()
        .node_by_tree_local_id(toolbar_id().node(), TreeId::ROOT)
        .unwrap();
    let names: Vec<_> = toolbar
        .filtered_children(common_filter)
        .map(|node| node.label().unwrap_or_default())
        .collect();
    for (at, name) in names.iter().enumerate() {
        assert!(
            !name.is_empty() && !names[at + 1..].contains(name),
            "{names:?}"
        );
    }
}

#[test]
fn a_disabled_combo_shows_its_value_but_takes_no_clicks_or_focus() {
    let mut ui = new_ui();
    let build = |ui: &mut Ui| {
        frame(ui, |ui| {
            shell::combo(ui, "font", "Font", "Calibri", 120.0, font_menu(), false);
        });
    };
    build(&mut ui);
    let [left, top, right, bottom] = ui.rect(ui.id("font")).unwrap();
    let middle = [(left + right) / 2.0, (top + bottom) / 2.0];
    ui.event(Event::PointerMoved(middle));
    for pressed in [true, false] {
        ui.event(Event::Button {
            button: winit::event::MouseButton::Left,
            pressed,
            at: Instant::now(),
        });
        build(&mut ui);
    }
    assert!(!ui.popup_open(font_menu()));
    key(&mut ui, NamedKey::Tab);
    build(&mut ui);
    assert_ne!(ui.focused(), Some(ui.id("font")));
    assert!(
        snapshot(&tree(&mut ui)).contains(r#"ComboBox "Font" = "Calibri" [collapsed] [disabled]"#),
        "{}",
        snapshot(&tree(&mut ui))
    );
}

#[test]
fn a_toggle_reads_as_one_while_off_and_a_plain_button_as_none() {
    let mut ui = new_ui();
    for _ in 0..2 {
        frame(&mut ui, |ui| {
            toolbar(ui, false);
        });
    }
    let snapshot = snapshot(&tree(&mut ui));
    assert!(
        snapshot.contains(r#"Button "Bold" [untoggled] <⌘B>"#),
        "{snapshot}"
    );
    assert!(
        snapshot.contains(r#"Button "Undo" <⌘Z> {click focus}"#),
        "{snapshot}"
    );
}

#[test]
fn a_row_opening_a_menu_says_so_instead_of_naming_keys() {
    let mut ui = new_ui();
    let menu = Id::ROOT.child("menu");
    let items = [
        popup::Item {
            text: "Bullet Library",
            submenu: true,
            ..popup::Item::default()
        },
        popup::Item {
            text: "Bold",
            shortcut: "⌘B",
            checked: Some(false),
            ..popup::Item::default()
        },
    ];
    ui.open_popup(menu);
    for _ in 0..2 {
        frame(&mut ui, |ui| {
            popup::menu(ui, menu, Anchor::Below([0.0; 4]), &items, None);
        });
    }
    let tree = tree(&mut ui);
    let snapshot = snapshot(&tree);
    assert!(
        snapshot.contains(r#"MenuItem "Bullet Library" [collapsed]"#),
        "{snapshot}"
    );
    assert!(!snapshot.contains('›'), "{snapshot}");
    assert!(
        snapshot.contains(r#"MenuItemCheckBox "Bold" [untoggled]"#),
        "{snapshot}"
    );
    let update = ui.accessibility_tree("Notes", 2.0);
    let (_, row) = update
        .nodes
        .iter()
        .find(|(_, node)| node.label() == Some("Bullet Library"))
        .expect("the row");
    assert_eq!(row.has_popup(), Some(accesskit::HasPopup::Menu));
}

#[test]
fn swatches_are_named_by_their_colour_names_or_hex() {
    let mut ui = new_ui();
    let grid = Id::ROOT.child("colours");
    let swatches = [([1.0, 1.0, 0.0, 1.0], "Yellow"), ([0.0, 0.0, 0.0, 1.0], "")];
    ui.open_popup(grid);
    for _ in 0..2 {
        frame(&mut ui, |ui| {
            popup::colors(ui, grid, Anchor::Below([0.0; 4]), "No Color", &swatches, 2);
        });
    }
    let snapshot = snapshot(&tree(&mut ui));
    assert!(snapshot.contains(r#"MenuItem "Yellow""#), "{snapshot}");
    assert!(snapshot.contains(r##"MenuItem "#000000""##), "{snapshot}");
}

#[test]
fn an_open_menu_takes_the_focus_and_its_highlight_is_the_focused_item() {
    let mut ui = new_ui();
    frame(&mut ui, |ui| {
        toolbar(ui, false);
    });
    // Tab enters the toolbar, and the arrows reach the font box, which Space opens.
    key(&mut ui, NamedKey::Tab);
    for _ in 0..4 {
        key(&mut ui, NamedKey::ArrowRight);
    }
    frame(&mut ui, |ui| {
        toolbar(ui, false);
    });
    assert_eq!(ui.focused(), Some(toolbar_id().child("font")));
    key(&mut ui, NamedKey::Space);
    for _ in 0..3 {
        frame(&mut ui, |ui| {
            toolbar(ui, false);
        });
    }
    assert!(ui.popup_open(font_menu()));
    let snapshot = snapshot(&tree(&mut ui));
    assert!(
        snapshot.contains(r#"ComboBox "Font" = "Calibri" [expanded]"#),
        "{snapshot}"
    );
    assert!(
        snapshot.ends_with(
            r#"  Menu
    SearchInput = "" (Font) {focus set}
    Heading "Recent"
    MenuItemCheckBox "Calibri" [toggled] [selected] [focused] {click focus}
    MenuItem "Arial" {click focus}
    MenuItem "Wingdings" [disabled]
"#
        ),
        "the checked item starts highlighted, and is the focus within the filter:\n{snapshot}"
    );
    key(&mut ui, NamedKey::ArrowDown);
    frame(&mut ui, |ui| {
        toolbar(ui, false);
    });
    let snapshot = self::snapshot(&tree(&mut ui));
    assert!(
        snapshot.contains(r#"MenuItem "Arial" [selected] [focused]"#),
        "{snapshot}"
    );
    // Escape closes it, and the focus returns to the box it opened from.
    key(&mut ui, NamedKey::Escape);
    frame(&mut ui, |ui| {
        toolbar(ui, false);
    });
    assert!(!ui.popup_open(font_menu()));
    assert_eq!(ui.focused(), Some(toolbar_id().child("font")));
    let snapshot = self::snapshot(&tree(&mut ui));
    assert!(
        snapshot.contains(r#"ComboBox "Font" = "Calibri" [collapsed] [focused]"#),
        "{snapshot}"
    );
}

#[test]
fn keys_step_through_the_toolbar_press_its_buttons_and_return() {
    let mut ui = new_ui();
    let page = Id::ROOT.child("page");
    let build = |ui: &mut Ui| {
        let clicked = toolbar(ui, false);
        ui.leaf(
            "page",
            Spec {
                flags: Flags::CUSTOM | Flags::FOCUSABLE,
                size: [fill(), fill()],
                role: Some(Role::GenericContainer),
                ..Spec::default()
            },
        );
        clicked
    };
    frame(&mut ui, |ui| {
        build(ui);
    });
    ui.set_focus(Some(page));
    // The page takes Tab for itself; F6 moves to the toolbar, entered at its first button.
    key(&mut ui, NamedKey::Tab);
    frame(&mut ui, |ui| {
        build(ui);
    });
    assert_eq!(ui.focused(), Some(page));
    key(&mut ui, NamedKey::F6);
    frame(&mut ui, |ui| {
        build(ui);
    });
    assert_eq!(ui.focused(), Some(toolbar_id().child("undo")));
    // The disabled button is passed over, and the arrows wrap.
    key(&mut ui, NamedKey::ArrowRight);
    frame(&mut ui, |ui| {
        build(ui);
    });
    assert_eq!(ui.focused(), Some(toolbar_id().child("bold")));
    key(&mut ui, NamedKey::ArrowLeft);
    key(&mut ui, NamedKey::ArrowLeft);
    frame(&mut ui, |ui| {
        build(ui);
    });
    assert_eq!(ui.focused(), Some(toolbar_id().child("level")));
    key(&mut ui, NamedKey::Home);
    key(&mut ui, NamedKey::ArrowRight);
    key(&mut ui, NamedKey::Enter);
    let mut clicked = Vec::new();
    frame(&mut ui, |ui| clicked = build(ui));
    assert_eq!(clicked, ["bold"]);
    // Tab leaves the toolbar as one step, for the page.
    key(&mut ui, NamedKey::Tab);
    frame(&mut ui, |ui| {
        build(ui);
    });
    assert_eq!(ui.focused(), Some(page));
    key(&mut ui, NamedKey::F6);
    key(&mut ui, NamedKey::Escape);
    frame(&mut ui, |ui| {
        build(ui);
    });
    assert_eq!(ui.focused(), Some(page), "Escape returns to the page");
}

fn dialog_id() -> Id {
    Id::ROOT.child("dialog")
}

/// A dialog of a labelled field, a check box and two buttons; returns the buttons clicked.
fn dialog(ui: &mut Ui, name: &mut String, checked: bool) -> Vec<&'static str> {
    let mut clicked = Vec::new();
    if !ui.popup_open(dialog_id()) {
        return clicked;
    }
    ui.open_as(
        dialog_id(),
        Spec {
            axis: Axis::Y,
            size: [px(300.0), children()],
            anchor: Some(Anchor::Dialog),
            role: Some(Role::Dialog),
            ..Spec::default()
        },
    );
    if let Some(node) = ui.access(dialog_id()) {
        node.set_label("Rename");
    }
    ui.leaf(
        "heading",
        Spec {
            size: [fit(), px(20.0)],
            text: Some("Choose a name for the section."),
            ..Spec::default()
        },
    );
    let field = dialog_id().child("name");
    text_field(
        ui,
        field,
        name,
        "Untitled",
        Spec {
            size: [fill(), px(22.0)],
            ..Spec::default()
        },
    );
    if let Some(node) = ui.access(field) {
        node.set_label("Name");
    }
    if check_box(ui, "keep", "Keep a copy", checked).clicked {
        clicked.push("keep");
    }
    for label in ["OK", "Cancel"] {
        if button(ui, label, label).clicked {
            clicked.push(label);
        }
    }
    ui.close();
    clicked
}

#[test]
fn a_dialog_holds_the_focus_and_tab_steps_through_its_controls() {
    let mut ui = new_ui();
    let mut name = "Notes".to_owned();
    let build = |ui: &mut Ui, name: &mut String| {
        ui.leaf(
            "behind",
            Spec {
                flags: Flags::CLICKABLE,
                size: [px(40.0), px(20.0)],
                text: Some("Behind"),
                role: Some(Role::Button),
                ..Spec::default()
            },
        );
        dialog(ui, name, true)
    };
    frame(&mut ui, |ui| {
        build(ui, &mut name);
    });
    ui.open_popup(dialog_id());
    frame(&mut ui, |ui| {
        build(ui, &mut name);
    });
    assert_eq!(
        snapshot(&tree(&mut ui)),
        r#"Window "Notes"
  Button "Behind" {click focus}
  Dialog "Rename" [focused]
    Label = "Choose a name for the section."
    TextInput "Name" = "Notes" (Untitled) {focus set}
    CheckBox "Keep a copy" [toggled] {click focus}
    Button "OK" {click focus}
    Button "Cancel" {click focus}
"#
    );
    let field = dialog_id().child("name");
    let mut focused = Vec::new();
    for _ in 0..5 {
        key(&mut ui, NamedKey::Tab);
        frame(&mut ui, |ui| {
            build(ui, &mut name);
        });
        focused.push(ui.focused());
    }
    let [keep, ok, cancel] = ["keep", "OK", "Cancel"].map(|part| Some(dialog_id().child(part)));
    assert_eq!(
        focused,
        [Some(field), keep, ok, cancel, Some(field)],
        "Tab stays within the dialog"
    );
    ui.event(Event::Modifiers(ModifiersState::SHIFT));
    key(&mut ui, NamedKey::Tab);
    ui.event(Event::Modifiers(ModifiersState::empty()));
    key(&mut ui, NamedKey::Space);
    let mut clicked = Vec::new();
    frame(&mut ui, |ui| clicked = build(ui, &mut name));
    assert_eq!(clicked, ["Cancel"]);
}

#[test]
fn assistive_technology_presses_focuses_and_sets_values() {
    let mut ui = new_ui();
    let mut name = "Notes".to_owned();
    frame(&mut ui, |ui| {
        dialog(ui, &mut name, false);
    });
    ui.open_popup(dialog_id());
    frame(&mut ui, |ui| {
        dialog(ui, &mut name, false);
    });
    let request = |action, id: Id, data| {
        Event::Access(ActionRequest {
            action,
            target_tree: TreeId::ROOT,
            target_node: id.node(),
            data,
        })
    };
    let field = dialog_id().child("name");
    ui.event(request(
        Action::SetValue,
        field,
        Some(accesskit::ActionData::Value("Journal".into())),
    ));
    ui.event(request(Action::Click, dialog_id().child("OK"), None));
    let mut clicked = Vec::new();
    frame(&mut ui, |ui| clicked = dialog(ui, &mut name, false));
    assert_eq!(name, "Journal");
    assert_eq!(clicked, ["OK"]);
    assert_eq!(ui.focused(), Some(field));
    ui.event(request(Action::Focus, dialog_id().child("keep"), None));
    frame(&mut ui, |ui| {
        dialog(ui, &mut name, false);
    });
    assert_eq!(ui.focused(), Some(dialog_id().child("keep")));
}

#[test]
fn the_palette_is_a_dialog_whose_field_leads_through_its_options() {
    let mut ui = new_ui();
    let id = Id::ROOT.child("palette");
    let items = [
        popup::Item {
            text: "Bold",
            shortcut: "⌘B",
            ..popup::Item::default()
        },
        popup::Item {
            text: "New Page",
            ..popup::Item::default()
        },
        popup::Item {
            text: "Italic",
            ..popup::Item::default()
        },
    ];
    frame(&mut ui, |ui| {
        popup::palette(ui, id, &[("", &items)], "Search commands");
    });
    ui.open_popup(id);
    frame(&mut ui, |ui| {
        popup::palette(ui, id, &[("", &items)], "Search commands");
    });
    for text in ["i", "t"] {
        ui.event(Event::Key {
            key: winit::keyboard::Key::Character(text.into()),
            text: Some(text.into()),
        });
        frame(&mut ui, |ui| {
            popup::palette(ui, id, &[("", &items)], "Search commands");
        });
    }
    frame(&mut ui, |ui| {
        popup::palette(ui, id, &[("", &items)], "Search commands");
    });
    assert_eq!(
        snapshot(&tree(&mut ui)),
        r#"Window "Notes"
  Dialog "Search commands"
    SearchInput = "it" (Search commands) {focus set}
    ListBox
      ListBoxOption "Italic" [selected] [focused] {click focus}
"#
    );
}

#[test]
fn updates_send_only_what_changed_and_grafts_hold_the_host_s_tree() {
    let mut ui = new_ui();
    let page = Id::ROOT.child("page");
    let tree_id = TreeId(accesskit::Uuid::from_u128(7));
    let build = |ui: &mut Ui, bold: bool| {
        toolbar(ui, bold);
        ui.open_as(
            page,
            Spec {
                flags: Flags::CUSTOM | Flags::FOCUSABLE,
                size: [fill(), fill()],
                role: Some(Role::GenericContainer),
                ..Spec::default()
            },
        );
        if let Some(node) = ui.access(page) {
            node.set_tree_id(tree_id);
        }
        // Built inside the page, the scrollbar shows beside it.
        button(ui, "scroll", "Scroll");
        ui.close();
    };
    frame(&mut ui, |ui| build(ui, false));
    let mut tree = tree(&mut ui);
    let mut page_node = accesskit::Node::new(Role::Group);
    page_node.set_label("Page");
    tree.update_and_process_changes(
        accesskit::TreeUpdate {
            nodes: vec![(NodeId(0), page_node)],
            tree: Some(accesskit::TreeInfo::new(NodeId(0))),
            tree_id,
            focus: NodeId(0),
        },
        &mut NoChanges,
    );
    ui.set_focus(Some(page));
    frame(&mut ui, |ui| build(ui, true));
    let update = ui.accessibility("Notes", 2.0);
    let changed: Vec<_> = update.nodes.iter().map(|(id, _)| *id).collect();
    assert_eq!(changed, [toolbar_id().child("bold").node()]);
    assert_eq!(update.focus, page.node());
    tree.update_and_process_changes(update, &mut NoChanges);
    let snapshot = snapshot(&tree);
    assert!(
        snapshot.ends_with(
            r#"  Group "Page" [focused]
  Button "Scroll" {click focus}
"#
        ),
        "{snapshot}"
    );
}

struct NoChanges;

impl accesskit_consumer::TreeChangeHandler for NoChanges {
    fn node_added(&mut self, _: &NodeRef) {}
    fn node_updated(&mut self, _: &NodeRef, _: &NodeRef) {}
    fn focus_moved(&mut self, _: Option<&NodeRef>, _: Option<&NodeRef>) {}
    fn node_removed(&mut self, _: &NodeRef) {}
}

/// A segmented control of sides with `chosen` picked, which a pick moves.
fn sides(ui: &mut Ui, chosen: &mut usize) {
    if let Some(picked) = segmented(ui, "sides", "Page tabs", &["Left", "Right"], *chosen) {
        *chosen = picked;
    }
}

#[test]
fn a_segmented_control_is_a_radio_group_tab_enters_at_its_choice_and_arrows_pick() {
    let mut ui = new_ui();
    let mut chosen = 1;
    for _ in 0..2 {
        frame(&mut ui, |ui| sides(ui, &mut chosen));
    }
    assert_eq!(
        snapshot(&tree(&mut ui)),
        r#"Window "Notes" [focused]
  RadioGroup "Page tabs"
    RadioButton "Left" [untoggled] {click focus}
    RadioButton "Right" [toggled] {click focus}
"#
    );
    let segment = |index: usize| Some(Id::ROOT.child("sides").child(index));
    key(&mut ui, NamedKey::Tab);
    frame(&mut ui, |ui| sides(ui, &mut chosen));
    assert_eq!(ui.focused(), segment(1), "Tab enters at the choice");
    assert_eq!(chosen, 1);
    key(&mut ui, NamedKey::ArrowLeft);
    for _ in 0..2 {
        frame(&mut ui, |ui| sides(ui, &mut chosen));
    }
    assert_eq!((ui.focused(), chosen), (segment(0), 0), "an arrow picks");
    // A click picks without moving the focus, which stays put on later frames.
    let [left, top, right, bottom] = ui.rect(segment(1).unwrap()).unwrap();
    ui.event(Event::PointerMoved([
        (left + right) / 2.0,
        (top + bottom) / 2.0,
    ]));
    for pressed in [true, false] {
        ui.event(Event::Button {
            button: winit::event::MouseButton::Left,
            pressed,
            at: Instant::now(),
        });
        frame(&mut ui, |ui| sides(ui, &mut chosen));
    }
    for _ in 0..2 {
        frame(&mut ui, |ui| sides(ui, &mut chosen));
    }
    assert_eq!(chosen, 1);
}
