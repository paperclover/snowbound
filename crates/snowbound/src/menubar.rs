//! The macOS menu bar, laid out as OneNote for Mac's: the command table's commands, and
//! AppKit's own items where the system provides them.

use crate::commands::{self, Choice, Chord, Id, cmd, named};
use crate::recording::Transport;
use canvas::editor::{Alignment, NoteTag, Toggle};
use draw::edit::NamedKey;
use objc2::{
    rc::Retained,
    runtime::{AnyObject, Sel},
};
use objc2_app_kit::{NSEventModifierFlags, NSMenu, NSMenuItem};
use objc2_foundation::{MainThreadMarker, NSString};

enum Item {
    Command(Id),
    /// AppKit's own: its title, its action's selector and its chord.
    System(&'static str, &'static str, Option<Chord>),
    Separator,
    Menu(&'static str, &'static [Item]),
    /// The system's Services menu.
    Services,
    /// Each of a list's choices.
    Fonts,
    Sizes,
    Highlights,
}

use Item::{Command as C, Separator as S};

const MENUS: &[Item] = &[
    Item::Menu(
        "Snowbound",
        &[
            Item::System("About Snowbound", "orderFrontStandardAboutPanel:", None),
            S,
            C(Id::Settings),
            S,
            Item::Services,
            S,
            Item::System("Hide Snowbound", "hide:", Some(cmd('h'))),
            Item::System(
                "Hide Others",
                "hideOtherApplications:",
                Some(cmd('h').option()),
            ),
            Item::System("Show All", "unhideAllApplications:", None),
            S,
            Item::System("Quit Snowbound", "terminate:", Some(cmd('q'))),
        ],
    ),
    Item::Menu(
        "File",
        &[
            C(Id::NewNotebook),
            C(Id::OpenNotebook),
            C(Id::CloseNotebook),
            S,
            C(Id::NewSection),
            C(Id::NewSectionGroup),
            C(Id::NewPage),
            C(Id::NewSubpage),
            S,
            C(Id::PageVersions),
            C(Id::CopyPageLink),
            C(Id::ShowNotebook),
        ],
    ),
    Item::Menu(
        "Edit",
        &[
            C(Id::Undo),
            C(Id::Redo),
            S,
            C(Id::Cut),
            C(Id::Copy),
            C(Id::Paste),
            C(Id::FormatPainter),
            C(Id::SelectAll),
            S,
            C(Id::Find),
            C(Id::Search),
            C(Id::SearchResults),
            S,
            Item::System("Start Dictation…", "startDictation:", None),
            Item::System(
                "Emoji & Symbols",
                "orderFrontCharacterPalette:",
                Some(named(NamedKey::Space).command().control()),
            ),
        ],
    ),
    // AppKit appends Enter Full Screen itself, and would repeat one listed here.
    Item::Menu(
        "View",
        &[
            C(Id::CommandPalette),
            S,
            C(Id::Back),
            C(Id::Forward),
            S,
            C(Id::ZoomIn),
            C(Id::ZoomOut),
            C(Id::ActualSize),
            S,
            C(Id::FullPageView),
            C(Id::Sidebar),
            C(Id::PageList),
            S,
            C(Id::PageColor),
            C(Id::DarkPages),
        ],
    ),
    Item::Menu(
        "Insert",
        &[
            C(Id::Table),
            C(Id::Picture),
            C(Id::ScreenClipping),
            C(Id::Link),
            S,
            C(Id::Equation),
            C(Id::Symbol),
            S,
            C(Id::Date),
            C(Id::Time),
            C(Id::DateTime),
            S,
            C(Id::RecordAudio),
            C(Id::RecordVideo),
            Item::Menu(
                "Audio & Video",
                &[
                    C(Id::Transport(Transport::Pause)),
                    C(Id::Transport(Transport::Stop)),
                    S,
                    C(Id::Transport(Transport::Skip(-600))),
                    C(Id::Transport(Transport::Skip(-10))),
                    C(Id::Transport(Transport::Skip(10))),
                    C(Id::Transport(Transport::Skip(600))),
                    C(Id::Transport(Transport::SeekTo)),
                    S,
                    C(Id::Transport(Transport::SeePlayback)),
                ],
            ),
        ],
    ),
    Item::Menu(
        "Format",
        &[
            Item::Menu("Font", &[Item::Fonts]),
            Item::Menu("Size", &[Item::Sizes]),
            S,
            C(Id::Toggle(Toggle::Bold)),
            C(Id::Toggle(Toggle::Italic)),
            C(Id::Toggle(Toggle::Underline)),
            C(Id::Toggle(Toggle::Strikethrough)),
            C(Id::Toggle(Toggle::Subscript)),
            C(Id::Toggle(Toggle::Superscript)),
            S,
            C(Id::Highlight),
            Item::Menu("Highlight Color", &[Item::Highlights]),
            C(Id::FontColor),
            S,
            C(Id::Bullets),
            C(Id::Numbering),
            S,
            C(Id::Align(Alignment::Left)),
            C(Id::Align(Alignment::Center)),
            C(Id::Align(Alignment::Right)),
            S,
            C(Id::Indent),
            C(Id::Outdent),
            S,
            C(Id::ClearFormatting),
        ],
    ),
    // Filled from the tag list by `tags`.
    Item::Menu("Tags", &[]),
    Item::Menu(
        "Window",
        &[
            Item::System("Minimize", "performMiniaturize:", Some(cmd('m'))),
            Item::System("Zoom", "performZoom:", None),
            S,
            Item::System("Bring All to Front", "arrangeInFront:", None),
        ],
    ),
    Item::Menu("Help", &[C(Id::Help)]),
];

/// The bar, and the menus AppKit fills itself.
pub struct Menus {
    pub bar: Retained<NSMenu>,
    pub services: Retained<NSMenu>,
    pub window: Retained<NSMenu>,
    pub help: Retained<NSMenu>,
    pub tags: Retained<NSMenu>,
}

/// Builds the bar; the table's items send `choose:` to `target`, which validates them.
pub fn build(mtm: MainThreadMarker, target: &AnyObject) -> Menus {
    let services = NSMenu::new(mtm);
    let bar = NSMenu::new(mtm);
    let mut submenus = Vec::new();
    for item in MENUS {
        let Item::Menu(title, items) = item else {
            unreachable!("The bar holds menus")
        };
        let menu = menu(mtm, target, title, items, &services);
        let holder = NSMenuItem::new(mtm);
        holder.setSubmenu(Some(&menu));
        bar.addItem(&holder);
        submenus.push(menu);
    }
    let named = |name: &str| {
        submenus
            .iter()
            .find(|menu| unsafe { menu.title() }.to_string() == name)
            .cloned()
            .expect("The bar has the menus AppKit fills")
    };
    Menus {
        tags: named("Tags"),
        window: named("Window"),
        help: named("Help"),
        services,
        bar,
    }
}

fn menu(
    mtm: MainThreadMarker,
    target: &AnyObject,
    title: &str,
    items: &[Item],
    services: &NSMenu,
) -> Retained<NSMenu> {
    let menu = unsafe { NSMenu::initWithTitle(mtm.alloc(), &NSString::from_str(title)) };
    let choose = |choice: &Choice, title: &str, chord: Option<Chord>| {
        let item = entry(mtm, title, Some(objc2::sel!(choose:)), chord);
        // The application validates and runs the table's items whatever holds focus.
        unsafe {
            item.setTarget(Some(target));
            item.setTag(commands::index(choice) as isize);
        }
        menu.addItem(&item);
    };
    let list = |pick: fn(&Choice) -> bool| {
        for choice in commands::choices().filter(pick) {
            choose(&choice, &commands::title(choice.clone()), None);
        }
    };
    for item in items {
        match item {
            Item::Command(id) => {
                let command = commands::command(*id);
                choose(
                    &Choice::Command(*id),
                    command.title,
                    command.mac.first().copied(),
                );
            }
            Item::System(title, action, chord) => {
                menu.addItem(&entry(mtm, title, Some(Sel::register(action)), *chord));
            }
            Item::Separator => menu.addItem(&NSMenuItem::separatorItem(mtm)),
            Item::Menu(title, items) => {
                let holder = entry(mtm, title, None, None);
                holder.setSubmenu(Some(&self::menu(mtm, target, title, items, services)));
                menu.addItem(&holder);
            }
            Item::Services => {
                let holder = entry(mtm, "Services", None, None);
                holder.setSubmenu(Some(services));
                menu.addItem(&holder);
            }
            Item::Fonts => list(|choice| matches!(choice, Choice::Font(_))),
            Item::Sizes => list(|choice| matches!(choice, Choice::Size(_))),
            Item::Highlights => list(|choice| matches!(choice, Choice::Highlight(_))),
        }
    }
    menu
}

/// Fills the Tags menu from the user's tag list, as OneNote's gallery lists it: the first
/// nine with their chords, the rest under More Tags, then managing them.
pub fn tags(mtm: MainThreadMarker, target: &AnyObject, menu: &NSMenu, tags: &[NoteTag]) {
    unsafe { menu.removeAllItems() };
    let choose = |menu: &NSMenu, id: Id, title: &str, chord: Option<Chord>| {
        let item = entry(mtm, title, Some(objc2::sel!(choose:)), chord);
        unsafe {
            item.setTarget(Some(target));
            item.setTag(commands::index(&Choice::Command(id)) as isize);
        }
        menu.addItem(&item);
    };
    let more = unsafe { NSMenu::initWithTitle(mtm.alloc(), &NSString::from_str("More Tags")) };
    for (place, tag) in tags.iter().enumerate().take(commands::TAGS) {
        let chord = commands::tag_chord(place);
        let shown = if chord.is_some() { menu } else { &*more };
        choose(shown, Id::Tag(place), &tag.label, chord);
    }
    if tags.len() > 9 {
        let holder = entry(mtm, "More Tags", None, None);
        holder.setSubmenu(Some(&more));
        menu.addItem(&holder);
    }
    if !tags.is_empty() {
        menu.addItem(&NSMenuItem::separatorItem(mtm));
    }
    for id in [Id::CustomizeTags, Id::RemoveTags, Id::FindTags] {
        let command = commands::command(id);
        choose(menu, id, command.title, command.mac.first().copied());
    }
}

fn entry(
    mtm: MainThreadMarker,
    title: &str,
    action: Option<Sel>,
    chord: Option<Chord>,
) -> Retained<NSMenuItem> {
    let (key, shift) = chord.map_or((String::new(), false), Chord::equivalent);
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            mtm.alloc(),
            &NSString::from_str(title),
            action,
            &NSString::from_str(&key),
        )
    };
    if let Some(chord) = chord {
        let mut mask = NSEventModifierFlags(0);
        for (held, flag) in [
            (shift, NSEventModifierFlags::NSEventModifierFlagShift),
            (
                chord.option,
                NSEventModifierFlags::NSEventModifierFlagOption,
            ),
            (
                chord.control,
                NSEventModifierFlags::NSEventModifierFlagControl,
            ),
            (
                chord.command,
                NSEventModifierFlags::NSEventModifierFlagCommand,
            ),
        ] {
            if held {
                mask.0 |= flag.0;
            }
        }
        item.setKeyEquivalentModifierMask(mask);
    }
    item
}

#[cfg(test)]
mod tests {
    use super::*;
    use draw::edit::Platform;

    /// The menu tree as the bar shows it: titles, chords, and each table item's choice.
    fn dump(menu: &NSMenu, depth: usize, out: &mut String, chords: &mut Vec<String>) {
        for index in 0..unsafe { menu.numberOfItems() } {
            let item = unsafe { menu.itemAtIndex(index) }.expect("Items within the count");
            let indent = "  ".repeat(depth);
            if unsafe { item.isSeparatorItem() } {
                out.push_str(&format!("{indent}---\n"));
                continue;
            }
            let mask = unsafe { item.keyEquivalentModifierMask() }.0;
            let key = unsafe { item.keyEquivalent() }.to_string();
            let chord = if key.is_empty() {
                String::new()
            } else {
                let symbols = [
                    (NSEventModifierFlags::NSEventModifierFlagControl, "⌃"),
                    (NSEventModifierFlags::NSEventModifierFlagOption, "⌥"),
                    (NSEventModifierFlags::NSEventModifierFlagShift, "⇧"),
                    (NSEventModifierFlags::NSEventModifierFlagCommand, "⌘"),
                ];
                let held: String = symbols
                    .iter()
                    .filter(|(flag, _)| mask & flag.0 != 0)
                    .map(|(_, symbol)| *symbol)
                    .collect();
                let key = match key.as_str() {
                    " " => "Space".to_owned(),
                    "\u{f702}" => "←".to_owned(),
                    "\u{f703}" => "→".to_owned(),
                    key => key.to_uppercase(),
                };
                format!("{held}{key}")
            };
            if !chord.is_empty() {
                chords.push(chord.clone());
            }
            let action = unsafe { item.action() }
                .filter(|action| *action != objc2::sel!(submenuAction:))
                .map_or(String::new(), |action| {
                    if action == objc2::sel!(choose:) {
                        let tag = unsafe { item.tag() } as usize;
                        format!(
                            "{:?}",
                            commands::choices().nth(tag).expect("Tags index choices")
                        )
                    } else {
                        action.name().to_owned()
                    }
                });
            let submenu = unsafe { item.submenu() };
            // The bar's items show their menu's title.
            let title = match &submenu {
                Some(submenu) if depth == 0 => unsafe { submenu.title() }.to_string(),
                _ => unsafe { item.title() }.to_string(),
            };
            out.push_str(format!("{indent}{title:<24} {chord:<8} {action}").trim_end());
            out.push('\n');
            if let Some(submenu) = submenu {
                dump(&submenu, depth + 1, out, chords);
            }
        }
    }

    #[test]
    fn menu_bar_offers_each_chord_once_as_the_keyboard_does() {
        // Built off the main thread for the test only; nothing shows or runs.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        let target: Retained<AnyObject> =
            unsafe { objc2::msg_send_id![objc2::class!(NSObject), new] };
        let menus = build(mtm, &target);
        tags(mtm, &target, &menus.tags, &NoteTag::defaults());
        let (mut tree, mut chords) = (String::new(), Vec::new());
        dump(&menus.bar, 0, &mut tree, &mut chords);
        println!("{tree}");
        for (at, chord) in chords.iter().enumerate() {
            assert!(!chords[at + 1..].contains(chord), "{chord} is on two items");
        }
        // The keyboard's first chord for each command is on its menu item.
        for command in commands::COMMANDS {
            let Some(chord) = command.mac.first() else {
                continue;
            };
            let line = tree
                .lines()
                .find(|line| line.ends_with(&format!("Command({:?})", command.id)))
                .unwrap_or_else(|| panic!("{} has a chord and no menu item", command.title));
            assert!(
                line.contains(&chord.label(Platform::MacOs)),
                "{line} shows {}",
                chord.label(Platform::MacOs)
            );
        }
    }
}
