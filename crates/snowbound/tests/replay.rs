//! The app driven by a replay in a hidden window, its accessibility tree written as text.
#![cfg(target_os = "macos")]

use std::path::{Path, PathBuf};
use std::process::Command;

/// A scratch folder, deleted when dropped. Its replays run with a home folder of its own, its
/// `settings.json` unless removed, and its `icloud` folder, where made, standing in for iCloud's.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("snowbound-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(path.join("notebook")).unwrap();
        std::fs::create_dir_all(path.join("home")).unwrap();
        std::fs::write(
            path.join("settings.json"),
            r#"{"user_name": "Snowbound Test"}"#,
        )
        .unwrap();
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Runs `steps` against a copy of `notebook`'s files with the scratch's settings, then
/// returns the trees each
/// `accessibility` step named by the trailing word wrote.
fn replay(scratch: &Scratch, notebook: Option<&Path>, steps: &[&str]) -> Vec<String> {
    replay_with(scratch, notebook, steps, &[])
}

/// `replay` with these variables set for the app.
fn replay_with(
    scratch: &Scratch,
    notebook: Option<&Path>,
    steps: &[&str],
    variables: &[(&str, &str)],
) -> Vec<String> {
    let dir = &scratch.0;
    if let Some(source) = notebook {
        for entry in std::fs::read_dir(source).unwrap() {
            let entry = entry.unwrap();
            std::fs::copy(entry.path(), dir.join("notebook").join(entry.file_name())).unwrap();
        }
    }
    let mut script = String::from("settle\n");
    let mut trees = Vec::new();
    for step in steps {
        match step.strip_prefix("accessibility ") {
            Some(name) => {
                let path = dir.join(format!("{name}.txt"));
                script += &format!("accessibility {}\n", path.display());
                trees.push(path);
            }
            None => script += &format!("{step}\n"),
        }
    }
    script += "quit\n";
    std::fs::write(dir.join("script"), script).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_snowbound"));
    if dir.join("icloud").is_dir() {
        command.env("SNOWBOUND_ICLOUD_FOLDER", dir.join("icloud"));
    }
    command
        .envs(variables.iter().copied())
        .env("HOME", dir.join("home"))
        .env("SNOWBOUND_REPLAY", dir.join("script"))
        .arg("--notebook")
        .arg(dir.join("notebook"))
        .args(["--cache".as_ref(), dir.join("cache").as_os_str()])
        .args(["--screenshot".as_ref(), dir.join("shot").as_os_str()]);
    if dir.join("settings.json").exists() {
        command.args(["--settings".as_ref(), dir.join("settings.json").as_os_str()]);
    }
    let status = command.status().unwrap();
    assert!(status.success(), "the replay ended with {status}");
    trees
        .iter()
        .map(|path| std::fs::read_to_string(path).unwrap())
        .collect()
}

/// The names of the toolbar's controls in `tree`, one a line.
fn toolbar_names(tree: &str) -> Vec<&str> {
    tree.lines()
        .skip_while(|line| line.trim() != "Toolbar")
        .skip(1)
        .take_while(|line| line.starts_with("    "))
        .filter(|line| !line.starts_with("     "))
        .map(|line| line.split('"').nth(1).unwrap_or_default())
        .collect()
}

fn assert_named_once(tree: &str) {
    let names = toolbar_names(tree);
    assert!(names.len() > 10, "{tree}");
    for (at, name) in names.iter().enumerate() {
        assert!(
            !name.is_empty() && !names[at + 1..].contains(name),
            "{name:?} is not the only toolbar control so named:\n{tree}"
        );
    }
}

#[test]
fn a_notebook_without_sections_has_a_tree_without_a_page() {
    let scratch = Scratch::new("no-sections");
    let [tree] = replay(&scratch, None, &["accessibility tree"])
        .try_into()
        .unwrap();
    assert!(
        tree.contains(r#"Label = "No sections in this notebook""#),
        "{tree}"
    );
    assert!(
        tree.contains(r#"ComboBox "Font" = "#) && tree.contains("[disabled]"),
        "{tree}"
    );
    assert_named_once(&tree);
}

#[test]
fn each_toolbar_control_has_a_name_of_its_own_at_every_width() {
    let scratch = Scratch::new("toolbar-names");
    let notebook =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/cross-container/candidate");
    let mut steps = Vec::new();
    for width in ["1600", "1180", "900", "600"] {
        steps.push(format!("resize {width} 760"));
        // The window system resizes the window, which settling does not wait for.
        steps.push("wait 300".into());
        steps.push(format!("accessibility {width}"));
    }
    let steps: Vec<_> = steps.iter().map(String::as_str).collect();
    for tree in replay(&scratch, Some(&notebook), &steps) {
        assert_named_once(&tree);
    }
}

/// The menu items listed under the first `Menu` of `tree`, one a line.
fn menu_items(tree: &str) -> Vec<&str> {
    tree.lines()
        .skip_while(|line| !line.trim_start().starts_with("Menu"))
        .skip(1)
        .take_while(|line| !line.trim_start().starts_with("Menu") || line.contains("MenuItem"))
        .filter(|line| line.contains("MenuItem"))
        .map(|line| line.split('"').nth(1).unwrap_or_default())
        .collect()
}

#[test]
fn the_palette_lists_recent_pages_and_the_actions_a_context_menu_offers() {
    let scratch = Scratch::new("palette-actions");
    let notebook =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/cross-container/candidate");
    let palette = ["modifiers command", "key p", "modifiers", "settle"];
    let chord = ["modifiers command", "key k", "modifiers", "settle"];
    let mut steps = Vec::from(palette);
    steps.extend(["type type over", "settle", "key Enter", "settle"]);
    steps.extend(palette);
    steps.push("accessibility recent");
    steps.extend(chord);
    steps.push("accessibility actions");
    steps.extend(["key Escape", "accessibility back"]);
    steps.extend(["key Escape", "settle", "move 1000 87", "press right"]);
    steps.extend(["release right", "accessibility context"]);
    let [recent, actions, back, context] = replay(&scratch, Some(&notebook), &steps)
        .try_into()
        .unwrap();
    let first = recent
        .lines()
        .find(|line| line.contains("ListBoxOption"))
        .unwrap_or_default();
    assert!(
        first.contains(r#""Delete into a table" -- "Cross" [selected]"#),
        "the page left starts the list, highlighted:\n{recent}"
    );
    let actions = menu_items(&actions);
    assert_eq!(actions.first(), Some(&"Open"), "{actions:?}");
    assert!(
        back.contains("Dialog") && menu_items(&back).is_empty(),
        "{back}"
    );
    // Open leads; rows past what the panel shows unscrolled are not built.
    let context = menu_items(&context);
    assert!(
        actions.len() > 5 && context.starts_with(&actions[1..]),
        "one list feeds both: {actions:?} {context:?}"
    );
}

#[test]
fn the_palette_makes_a_page_of_a_query_matching_nothing() {
    let scratch = Scratch::new("palette-create");
    let notebook =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/cross-container/candidate");
    let steps = [
        "modifiers command",
        "key p",
        "modifiers",
        "settle",
        "type Zebra crossings",
        "settle",
        "accessibility offered",
        "key Enter",
        "settle",
        "accessibility made",
    ];
    let [offered, made] = replay(&scratch, Some(&notebook), &steps)
        .try_into()
        .unwrap();
    assert!(
        offered.contains(r#""Create Page “Zebra crossings”""#),
        "{offered}"
    );
    assert!(
        page_tabs(&made).contains(&"*Zebra crossings".to_owned()),
        "{made}"
    );
}

/// The options listed in `tree`'s palette, one a line.
fn options(tree: &str) -> Vec<&str> {
    tree.lines()
        .filter(|line| line.contains("ListBoxOption"))
        .map(|line| line.split('"').nth(1).unwrap_or_default())
        .collect()
}

/// Every item of the context menus of the page's text, the page, the section and the notebook
/// is a command in the palette: one of the same name, or one naming what it acts on as well.
#[test]
fn every_context_menu_action_is_a_palette_command() {
    let scratch = Scratch::new("palette-commands");
    let notebook =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/cross-container/candidate");
    let menu = |at: &'static str, name: &'static str| {
        [
            at,
            "press right",
            "release right",
            "settle",
            name,
            "key Escape",
        ]
    };
    let mut steps = Vec::from(menu("move 200 106", "accessibility text"));
    steps.extend(menu("move 1000 87", "accessibility page"));
    steps.extend(menu("move 109 50", "accessibility section"));
    steps.extend(["modifiers command", "key \\", "modifiers", "wait 500"]);
    steps.extend(menu("move 60 76", "accessibility notebook"));
    let menus = replay(&scratch, Some(&notebook), &steps);
    let mut labels = Vec::new();
    for (noun, tree) in ["", "page", "section", "notebook"].into_iter().zip(&menus) {
        let items = menu_items(tree);
        assert!(items.len() > 4, "{tree}");
        // The sync status heads the notebook's.
        let skip = usize::from(noun == "notebook");
        labels.extend(items[skip..].iter().map(|label| (noun, label.to_string())));
    }
    let mut steps = Vec::new();
    for (at, (_, label)) in labels.iter().enumerate() {
        steps.extend(["modifiers command", "key p", "modifiers", "settle"].map(String::from));
        steps.extend([format!("type >{label}"), "settle".into()]);
        steps.extend([format!("accessibility {at}"), "key Escape".into()]);
    }
    let steps: Vec<_> = steps.iter().map(String::as_str).collect();
    let scratch = Scratch::new("palette-commands-found");
    let found = replay(&scratch, Some(&notebook), &steps);
    for ((noun, label), tree) in labels.iter().zip(&found) {
        let words = |text: &str| {
            text.to_lowercase()
                .split(' ')
                .map(String::from)
                .collect::<Vec<_>>()
        };
        let wanted = words(label);
        assert!(
            options(tree).iter().any(|option| {
                let have = words(option);
                *option == label
                    || wanted.iter().all(|word| have.contains(word))
                        && option.to_lowercase().contains(noun)
            }),
            "the {noun}'s {label:?} has no palette command:\n{tree}"
        );
    }
}

/// The page tabs listed in `tree`, the selected one marked with `*`.
fn page_tabs(tree: &str) -> Vec<String> {
    tree.lines()
        .skip_while(|line| line.trim() != r#"TabList "Pages""#)
        .skip(1)
        .take_while(|line| line.trim_start().starts_with("Tab "))
        .map(|line| {
            let name = line.split('"').nth(1).unwrap_or_default();
            match line.ends_with("[selected]") {
                true => format!("*{name}"),
                false => name.to_owned(),
            }
        })
        .collect()
}

/// Undo walks back through two new pages and what was typed on each, crossing pages as
/// it goes: each page keeps its own history once left, and a new page goes once what was
/// typed on it has gone.
#[test]
fn undo_walks_back_through_new_pages_and_their_titles() {
    let scratch = Scratch::new("undo-pages");
    let notebook =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/cross-container/candidate");
    let chord =
        |modifiers: &str, key: &str| [modifiers, key, "modifiers", "settle"].map(String::from);
    let mut steps = Vec::new();
    for title in ["type One", "type Two"] {
        steps.extend(chord("modifiers command", "key n"));
        steps.push(title.into());
    }
    steps.extend(chord("modifiers command control", "key Left"));
    steps.push("accessibility back".into());
    for name in ["shown", "two", "removed", "one", "first"] {
        steps.extend(chord("modifiers command", "key z"));
        steps.push(format!("accessibility {name}"));
    }
    steps.extend(chord("modifiers command shift", "key z"));
    steps.push("accessibility redone".into());
    let steps: Vec<&str> = steps.iter().map(String::as_str).collect();
    let trees = replay(&scratch, Some(&notebook), &steps);
    let tabs: Vec<Vec<String>> = trees.iter().map(|tree| page_tabs(tree)).collect();
    let corpus = [
        "Delete into a table",
        "Delete out of a table",
        "Type over a row",
        "Delete a whole table",
    ];
    let list = |extra: &[&str], selected: &str| -> Vec<String> {
        corpus
            .iter()
            .chain(extra)
            .map(|name| match *name == selected {
                true => format!("*{name}"),
                false => name.to_string(),
            })
            .collect()
    };
    let expected = [
        // Back to the first new page.
        list(&["One", "Two"], "One"),
        // Undo there shows the newer page, whose typing goes first.
        list(&["One", "Two"], "Two"),
        list(&["One", "Untitled page"], "Untitled page"),
        // Then the page, back to the page shown before it.
        list(&["One"], "One"),
        // The first page's own typing, kept while it was left.
        list(&["Untitled page"], "Untitled page"),
        list(&[], "Delete into a table"),
        // Redo brings the page back.
        list(&["Untitled page"], "Untitled page"),
    ];
    assert_eq!(tabs.len(), expected.len());
    for ((tabs, expected), tree) in tabs.iter().zip(&expected).zip(&trees) {
        assert_eq!(tabs, expected, "{tree}");
    }
}

/// Whether `tree`'s toolbar offers Back and Forward.
fn travels(tree: &str) -> [bool; 2] {
    ["Back", "Forward"].map(|name| {
        tree.lines()
            .find(|line| line.trim_start().starts_with(&format!("Button \"{name}\"")))
            .is_some_and(|line| !line.ends_with("[disabled]"))
    })
}

const BACK: [&str; 4] = [
    "modifiers command control",
    "key Left",
    "modifiers",
    "settle",
];
const FORWARD: [&str; 4] = [
    "modifiers command control",
    "key Right",
    "modifiers",
    "settle",
];
const NEW_PAGE: [&str; 4] = ["modifiers command", "key n", "modifiers", "settle"];

/// Forward reaches a page whose section was renamed after going Back from it, where the
/// path it was visited at no longer opens.
#[test]
fn forward_follows_a_section_renamed_since() {
    let scratch = Scratch::new("forward-renamed");
    let notebook =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/cross-container/candidate");
    let mut steps = Vec::from(NEW_PAGE);
    steps.push("type Two");
    steps.extend(BACK);
    // The section's tab menu, Rename.
    steps.extend(["move 110 53", "press right", "release right", "settle"]);
    steps.extend([
        "key Down",
        "key Enter",
        "settle",
        "type Renamed",
        "key Enter",
        "settle",
    ]);
    steps.push("accessibility renamed");
    steps.extend(FORWARD);
    steps.push("accessibility forward");
    let [renamed, forward] = replay(&scratch, Some(&notebook), &steps)
        .try_into()
        .unwrap();
    assert!(scratch.0.join("notebook/Renamed.one").exists());
    assert_eq!(travels(&renamed), [false, true], "{renamed}");
    let shown = |tree: &str| page_tabs(tree).into_iter().find(|tab| tab.starts_with('*'));
    assert_eq!(shown(&forward).as_deref(), Some("*Two"), "{forward}");
    assert!(forward.contains(r#"Tab "Renamed" [selected]"#), "{forward}");
    assert_eq!(travels(&forward), [true, false], "{forward}");
}

/// Back passes over a page deleted since it was shown, and goes dark once nothing is left
/// behind.
#[test]
fn back_passes_over_a_page_deleted_since() {
    let scratch = Scratch::new("back-deleted");
    let notebook =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/cross-container/candidate");
    let mut steps = Vec::new();
    for title in ["type Two", "type Three"] {
        steps.extend(NEW_PAGE);
        steps.push(title);
    }
    // Two's tab menu, Delete.
    steps.extend([
        "settle",
        "move 1003 198",
        "press right",
        "release right",
        "settle",
    ]);
    steps.extend(["key Down", "key Down", "key Enter", "settle"]);
    steps.push("accessibility deleted");
    steps.extend(BACK);
    steps.push("accessibility back");
    let [deleted, back] = replay(&scratch, Some(&notebook), &steps)
        .try_into()
        .unwrap();
    let shown = |tree: &str| page_tabs(tree).into_iter().find(|tab| tab.starts_with('*'));
    assert!(
        !page_tabs(&deleted).contains(&"Two".to_owned()),
        "{deleted}"
    );
    assert_eq!(shown(&deleted).as_deref(), Some("*Three"), "{deleted}");
    assert_eq!(
        shown(&back).as_deref(),
        Some("*Delete into a table"),
        "{back}"
    );
    assert_eq!(travels(&back), [false, true], "{back}");
}

/// The search box's results in `tree`, one a line.
fn search_results(tree: &str) -> Vec<&str> {
    tree.lines()
        .skip_while(|line| line.trim() != r#"Dialog "Search""#)
        .filter(|line| line.trim_start().starts_with("ListBoxOption"))
        .collect()
}

/// Steps typing `query` into the search box, then writing the tree as `name`.
fn search(query: &str, name: &str) -> Vec<String> {
    ["modifiers command", "key e", "modifiers"]
        .into_iter()
        .map(String::from)
        .chain([format!("type {query}"), format!("accessibility {name}")])
        .collect()
}

/// Steps opening the page versions notebook's first version and its bar's menu.
fn version_menu() -> Vec<String> {
    let mut steps: Vec<String> = ["modifiers command shift", "key p", "modifiers"]
        .into_iter()
        .chain(["type Page Versions", "settle", "key Enter", "settle"])
        .map(String::from)
        .collect();
    // The version row under the page, then the yellow bar above the version.
    for point in ["1003 114", "177 94"] {
        steps.extend([format!("move {point}"), "press".into(), "release".into()]);
        steps.push("settle".into());
    }
    steps
}

fn run(scratch: &Scratch, notebook: &Path, steps: &[String]) -> Vec<String> {
    let steps: Vec<&str> = steps.iter().map(String::as_str).collect();
    replay(scratch, Some(notebook), &steps)
}

/// A template's content is found by search once it is on the page.
#[test]
fn search_finds_what_a_template_put_on_the_page() {
    let scratch = Scratch::new("template-search");
    let notebook =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/cross-container/candidate");
    let mut steps: Vec<String> = ["modifiers command", "key n", "modifiers", "settle"]
        .into_iter()
        // The Meeting tile of the template strip over the new page.
        .chain(["move 615 287", "press", "release", "settle"])
        .map(String::from)
        .collect();
    steps.extend(search("Attendees", "found"));
    let [found] = run(&scratch, &notebook, &steps).try_into().unwrap();
    assert!(
        search_results(&found)
            .iter()
            .any(|line| line.contains("Attendees")),
        "{found}"
    );
}

/// A version restored is found by search.
#[test]
fn search_finds_a_restored_version() {
    let scratch = Scratch::new("restore-search");
    let notebook =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/page-versions/candidate-restore");
    let mut steps = version_menu();
    // Restore Version.
    steps.extend(["key Down", "key Enter", "settle"].map(String::from));
    steps.extend(search("Second author", "found"));
    let [found] = run(&scratch, &notebook, &steps).try_into().unwrap();
    assert!(!search_results(&found).is_empty(), "{found}");
}

/// A version copied into its own section is listed there at once, and found by search.
#[test]
fn a_version_copied_into_its_section_is_listed_and_found() {
    let scratch = Scratch::new("copy-version");
    let notebook =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/page-versions/candidate-restore");
    let mut steps = version_menu();
    // Copy Page To, then the section itself.
    steps.extend(["key Down", "key Down", "key Down", "key Enter", "settle"].map(String::from));
    steps.extend(["key Down", "key Enter", "accessibility copied"].map(String::from));
    steps.extend(search("Second author", "found"));
    let [copied, found] = run(&scratch, &notebook, &steps).try_into().unwrap();
    let pages = page_tabs(&copied);
    let versioned = pages
        .iter()
        .filter(|tab| tab.ends_with("Versioned"))
        .count();
    assert_eq!(versioned, 3, "{pages:?}");
    assert!(!search_results(&found).is_empty(), "{found}");
}

/// Rename on a section tab's menu opens the field on the tab, as a double click there does,
/// which takes what is typed over the old name at once; the shut sidebar stays shut.
#[test]
fn renaming_a_section_from_its_tab_types_into_the_tab_at_once() {
    let scratch = Scratch::new("rename-tab");
    let notebook =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/cross-container/candidate");
    let mut steps = vec!["move 110 53", "press right", "release right", "settle"];
    steps.extend(["key Down", "key Enter", "settle"]);
    steps.extend(["type Renamed", "accessibility typed"]);
    steps.extend(["key Enter", "settle", "accessibility renamed"]);
    let [typed, renamed] = replay(&scratch, Some(&notebook), &steps)
        .try_into()
        .unwrap();
    assert!(
        typed.contains(r#"TextInput "Name" = "Renamed" [focused]"#),
        "{typed}"
    );
    assert!(notebooks(&typed).is_empty(), "the sidebar opened:\n{typed}");
    assert!(renamed.contains(r#"Tab "Renamed""#), "{renamed}");
}

/// Finding on the page leaves the query room beside the match count: a press just past
/// where the count ends lands in the field.
#[test]
fn the_find_bar_keeps_room_for_the_query() {
    let scratch = Scratch::new("find-room");
    let notebook =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/cross-container/candidate");
    let mut steps = vec!["modifiers command", "key f", "modifiers", "settle"];
    steps.extend(["type Alpha", "settle", "move 600 600", "press", "release"]);
    steps.extend(["settle", "move 1000 52", "press", "release", "settle"]);
    steps.extend(["type zz", "accessibility found"]);
    let [found] = replay(&scratch, Some(&notebook), &steps)
        .try_into()
        .unwrap();
    assert!(
        found.contains(r#"SearchInput "Find on Page" = "Alphazz" [focused]"#),
        "{found}"
    );
}

#[test]
fn paste_replaces_a_focused_input_without_editing_the_page() {
    let scratch = Scratch::new("paste-field");
    let notebook =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/cross-container/candidate");
    let steps = [
        "move 900 600",
        "press",
        "release",
        "type 雪 paste ☃",
        "settle",
        "modifiers command shift",
        "key Left",
        "modifiers command",
        "key c",
        "modifiers",
        "settle",
        "modifiers command",
        "key f",
        "modifiers",
        "settle",
        "type replace me",
        "settle",
        "modifiers command",
        "key a",
        "modifiers",
        "settle",
        "modifiers command",
        "key v",
        "modifiers",
        "accessibility pasted",
    ];
    let [pasted] = replay(&scratch, Some(&notebook), &steps)
        .try_into()
        .unwrap();
    assert!(
        pasted.contains(r#"SearchInput "Find on Page" = "雪 paste ☃" [focused]"#),
        "{pasted}"
    );
    assert_eq!(pasted.matches("雪 paste ☃").count(), 2, "{pasted}");
}

/// A launch shows the page each notebook was left on, as the settings recall them: the
/// notebook shown at once, and another as its section is opened.
#[test]
fn a_launch_returns_to_the_page_each_notebook_was_left_on() {
    let scratch = Scratch::new("left-on");
    let source =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/cross-container/candidate");
    let other = scratch.0.join("other");
    std::fs::create_dir_all(&other).unwrap();
    for entry in std::fs::read_dir(&source).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), other.join(entry.file_name())).unwrap();
    }
    let place = |notebook: &Path, page: &str| serde_json::json!({"notebook": notebook, "section": "Cross.one", "page": page});
    let settings = serde_json::json!({
        "user_name": "Snowbound Test",
        "sidebar": true,
        "notebooks": [other],
        "recent": [
            // Type over a row, then Delete a whole table.
            place(&scratch.0.join("notebook"), "{F8285B5C-5410-4A62-B7DC-43E408FE82AE},1"),
            place(&other, "{9F114CE9-F7D5-4234-B7C3-FF0D79615BB2},1"),
        ],
    });
    std::fs::write(scratch.0.join("settings.json"), settings.to_string()).unwrap();
    // The other notebook's section in the sidebar.
    let steps = [
        "accessibility launched",
        "move 63 104",
        "press",
        "release",
        "accessibility other",
    ];
    let shown = |tree: &str| page_tabs(tree).into_iter().find(|tab| tab.starts_with('*'));
    let [launched, other] = replay(&scratch, Some(&source), &steps).try_into().unwrap();
    assert_eq!(
        shown(&launched).as_deref(),
        Some("*Type over a row"),
        "{launched}"
    );
    assert_eq!(
        shown(&other).as_deref(),
        Some("*Delete a whole table"),
        "{other}"
    );
}

/// The names of the rows that fold in `tree`'s sidebar: its notebooks, where none holds a
/// section group.
fn notebooks(tree: &str) -> Vec<&str> {
    let lines: Vec<&str> = tree.lines().map(str::trim_start).collect();
    lines
        .windows(2)
        .filter(|pair| {
            pair[0].starts_with("TreeItem ")
                && ["Button \"Collapse\"", "Button \"Expand\""]
                    .iter()
                    .any(|fold| pair[1].starts_with(fold))
        })
        .filter_map(|pair| pair[0].split('"').nth(1))
        .collect()
}

/// Copies the notebook at `source` to `folder`.
fn copy_notebook(source: &Path, folder: &Path) {
    std::fs::create_dir_all(folder).unwrap();
    for entry in std::fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), folder.join(entry.file_name())).unwrap();
    }
}

/// Tooling never reaches the account's own notebooks: not those its settings list, nor
/// iCloud's beyond the folder it names.
#[test]
fn a_replay_opens_only_the_notebooks_and_icloud_folder_it_is_given() {
    let scratch = Scratch::new("isolated");
    let notebook =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/cross-container/candidate");
    let account = scratch.0.join("Account");
    copy_notebook(&notebook, &account);
    let settings = scratch
        .0
        .join("home/Library/Application Support/Snowbound/settings.json");
    std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
    let listed = serde_json::json!({"sidebar": true, "notebooks": [account]});
    std::fs::write(&settings, listed.to_string()).unwrap();
    std::fs::remove_file(scratch.0.join("settings.json")).unwrap();
    let sidebar = [
        "modifiers command",
        "key \\",
        "modifiers",
        "settle",
        "accessibility tree",
    ];
    let [tree] = replay(&scratch, Some(&notebook), &sidebar)
        .try_into()
        .unwrap();
    assert_eq!(notebooks(&tree), ["notebook"], "{tree}");
    copy_notebook(&notebook, &scratch.0.join("icloud/Cloudy"));
    let [tree] = replay(&scratch, None, &sidebar).try_into().unwrap();
    let mut listed = notebooks(&tree);
    listed.sort_unstable();
    assert_eq!(listed, ["Cloudy", "notebook"], "{tree}");
}

/// Stop Sharing ends the share: Live Share offers Start Sharing again, and nothing is kept to
/// share the notebook again on the next launch.
#[cfg(feature = "live")]
#[test]
fn stop_sharing_ends_the_share() {
    let scratch = Scratch::new("stop-sharing");
    let notebook =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/cross-container/candidate");
    let mut steps = vec!["modifiers command shift", "key p", "modifiers", "settle"];
    steps.extend(["type Live Share", "settle", "key Enter", "settle"]);
    steps.extend(["key Enter", "wait 1500", "accessibility shared"]);
    // Copy, Copy Link, Ask before joining, then Stop Sharing.
    steps.extend([
        "key Tab",
        "key Tab",
        "key Tab",
        "key Tab",
        "key Enter",
        "wait 1500",
        "accessibility stopped",
    ]);
    // Off every network, so that the test reaches no one.
    let variables = [("SNOWBOUND_LIVE", "off"), ("SNOWBOUND_LIVE_RELAY", "off")];
    let [shared, stopped] = replay_with(&scratch, Some(&notebook), &steps, &variables)
        .try_into()
        .unwrap();
    assert!(
        shared.contains("Stop Sharing") && shared.contains("No one has joined yet."),
        "{shared}"
    );
    assert!(
        stopped.contains("Start Sharing") && !stopped.contains("Stop Sharing"),
        "{stopped}"
    );
    let kept = scratch.0.join("cache/live/hosting.json");
    let kept = std::fs::read_to_string(kept).unwrap_or_default();
    assert!(!kept.contains("code"), "still kept: {kept}");
}

/// Open Shared Notebook is where people look with notebooks open: in the sidebar's footer,
/// and in the palette, which opens it.
#[cfg(feature = "live")]
#[test]
fn open_shared_notebook_is_offered_with_notebooks_open() {
    let scratch = Scratch::new("open-shared");
    std::fs::write(
        scratch.0.join("settings.json"),
        r#"{"user_name": "Snowbound Test", "sidebar": true}"#,
    )
    .unwrap();
    let notebook =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/cross-container/candidate");
    let mut steps = vec!["accessibility sidebar", "modifiers command shift", "key p"];
    steps.extend([
        "modifiers",
        "settle",
        "type Open Shared",
        "settle",
        "key Enter",
    ]);
    steps.extend(["settle", "accessibility dialog"]);
    let variables = [("SNOWBOUND_LIVE", "off"), ("SNOWBOUND_LIVE_RELAY", "off")];
    let [sidebar, dialog] = replay_with(&scratch, Some(&notebook), &steps, &variables)
        .try_into()
        .unwrap();
    assert!(sidebar.contains(r#""Open Shared""#), "{sidebar}");
    assert!(
        dialog.contains(r#"Dialog "Open Shared Notebook""#),
        "{dialog}"
    );
}

/// Switching between Metal and OpenGL, again and again, leaves the page, its edits and the
/// interface as they were.
#[test]
fn switching_renderers_keeps_the_window_as_it_was() {
    let scratch = Scratch::new("renderer-switch");
    let notebook = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/search/notebook");
    let mut steps = vec![
        "move 600 400".to_owned(),
        "press".to_owned(),
        "release".to_owned(),
        "type switching renderers ".to_owned(),
        "settle".to_owned(),
        "accessibility before".to_owned(),
    ];
    for round in 0..5 {
        for renderer in ["gl", "metal"] {
            steps.push(format!("renderer {renderer}"));
            steps.push("settle".to_owned());
            steps.push(format!("accessibility {renderer}-{round}"));
        }
    }
    steps.extend([
        "type and after".to_owned(),
        "accessibility after".to_owned(),
    ]);
    let steps: Vec<&str> = steps.iter().map(String::as_str).collect();
    let trees = replay(&scratch, Some(&notebook), &steps);
    let (before, after) = (&trees[0], &trees[trees.len() - 1]);
    assert!(before.contains("switching renderers"), "{before}");
    for tree in &trees[1..trees.len() - 1] {
        assert_eq!(tree, before, "a switch changed the window");
    }
    assert!(
        after.contains("switching renderers and after"),
        "the caret stayed put: {after}"
    );
}

/// A double click on a sidebar row renames it in place, as the Finder renames a file: the
/// notebook's folder and the section each take the name typed, and the notebook stays
/// unfolded through the double click's two clicks.
#[test]
fn a_double_click_renames_a_sidebar_row_in_place() {
    let scratch = Scratch::new("double-click-rename");
    let notebook =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/cross-container/candidate");
    let settings = r#"{"user_name": "Snowbound Test", "sidebar": true}"#;
    std::fs::write(scratch.0.join("settings.json"), settings).unwrap();
    let double = |y: &'static str| [y, "press", "release", "press", "release", "settle"];
    let mut steps = Vec::from(double("move 60 76"));
    steps.extend(["type Projects", "key Enter", "settle", "wait 500", "settle"]);
    steps.extend(double("move 80 100"));
    steps.extend(["type Soups", "key Enter", "settle", "accessibility renamed"]);
    let [renamed] = replay(&scratch, Some(&notebook), &steps)
        .try_into()
        .unwrap();
    assert!(
        renamed.contains(r#"TreeItem "Projects" [expanded]"#),
        "{renamed}"
    );
    assert!(
        renamed.contains(r#"TreeItem "Soups" [selected]"#),
        "{renamed}"
    );
    assert!(scratch.0.join("Projects/Soups.one").exists());
}

/// A notebook's row dragged below another's moves the notebook there.
#[test]
fn dragging_a_notebook_row_reorders_the_notebooks() {
    let scratch = Scratch::new("drag-notebook");
    let notebook =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/cross-container/candidate");
    copy_notebook(&notebook, &scratch.0.join("icloud/Cloudy"));
    let settings = r#"{"user_name": "Snowbound Test", "sidebar": true}"#;
    std::fs::write(scratch.0.join("settings.json"), settings).unwrap();
    let mut steps = vec![
        "wait 1000",
        "settle",
        "accessibility before",
        "move 60 76",
        "press",
    ];
    for y in ["move 60 84", "move 60 110", "move 60 140", "move 60 170"] {
        steps.extend([y, "wait 50"]);
    }
    steps.extend(["release", "wait 400", "settle", "accessibility after"]);
    let [before, after] = replay(&scratch, Some(&notebook), &steps)
        .try_into()
        .unwrap();
    assert_eq!(notebooks(&before), ["notebook", "Cloudy"], "{before}");
    assert_eq!(notebooks(&after), ["Cloudy", "notebook"], "{after}");
}

/// A notebook at the top of the iCloud folder, which Close can't take from the sidebar, offers
/// Delete Notebook in its menu instead.
#[test]
fn an_icloud_notebook_offers_delete_in_place_of_close() {
    let scratch = Scratch::new("icloud-delete");
    let notebook =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/cross-container/candidate");
    let cloudy = scratch.0.join("icloud/Cloudy");
    copy_notebook(&notebook, &cloudy);
    let settings = serde_json::json!({
        "user_name": "Snowbound Test",
        "sidebar": true,
        "notebooks": [cloudy],
    });
    std::fs::write(scratch.0.join("settings.json"), settings.to_string()).unwrap();
    let steps = [
        "wait 500",
        "settle",
        "move 60 76",
        "press right",
        "release right",
    ];
    let [menu] = replay(
        &scratch,
        Some(&notebook),
        &[&steps[..], &["accessibility menu"]].concat(),
    )
    .try_into()
    .unwrap();
    let items = menu_items(&menu);
    let close = items.iter().position(|item| *item == "Close This Notebook");
    assert_eq!(
        close.map(|at| items[at + 1]),
        Some("Delete Notebook"),
        "{menu}"
    );
    assert!(
        menu.lines()
            .any(|line| line.contains(r#""Close This Notebook""#) && line.contains("[disabled]")),
        "{menu}"
    );
}
