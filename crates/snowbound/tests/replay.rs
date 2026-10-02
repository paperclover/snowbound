//! The app driven by a replay in a hidden window, its accessibility tree written as text.
#![cfg(target_os = "macos")]

use std::path::{Path, PathBuf};
use std::process::Command;

/// A scratch folder, deleted when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("snowbound-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(path.join("notebook")).unwrap();
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Runs `steps` against a copy of `notebook`'s files, then returns the trees each
/// `accessibility` step named by the trailing word wrote.
fn replay(scratch: &Scratch, notebook: Option<&Path>, steps: &[&str]) -> Vec<String> {
    let dir = &scratch.0;
    if let Some(source) = notebook {
        for entry in std::fs::read_dir(source).unwrap() {
            let entry = entry.unwrap();
            std::fs::copy(entry.path(), dir.join("notebook").join(entry.file_name())).unwrap();
        }
    }
    let mut script = String::from("wait 500\n");
    let mut trees = Vec::new();
    for step in steps {
        match step.strip_prefix("accessibility ") {
            Some(name) => {
                let path = dir.join(format!("{name}.txt"));
                script += &format!("accessibility {}\nwait 100\n", path.display());
                trees.push(path);
            }
            None => script += &format!("{step}\n"),
        }
    }
    script += "quit\n";
    std::fs::write(dir.join("script"), script).unwrap();
    std::fs::write(
        dir.join("settings.json"),
        r#"{"user_name": "Snowbound Test"}"#,
    )
    .unwrap();
    let status = Command::new(env!("CARGO_BIN_EXE_snowbound"))
        .env("SNOWBOUND_REPLAY", dir.join("script"))
        .arg("--notebook")
        .arg(dir.join("notebook"))
        .args(["--cache".as_ref(), dir.join("cache").as_os_str()])
        .args(["--settings".as_ref(), dir.join("settings.json").as_os_str()])
        .args(["--screenshot".as_ref(), dir.join("shot").as_os_str()])
        .status()
        .unwrap();
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
    let palette = ["modifiers command", "key p", "modifiers", "wait 400"];
    let chord = ["modifiers command", "key k", "modifiers", "wait 400"];
    let mut steps = Vec::from(palette);
    steps.extend(["type type over", "wait 200", "key Enter", "wait 800"]);
    steps.extend(palette);
    steps.push("accessibility recent");
    steps.extend(chord);
    steps.push("accessibility actions");
    steps.extend(["key Escape", "wait 400", "accessibility back"]);
    steps.extend(["key Escape", "wait 400", "move 1000 87", "press right"]);
    steps.extend(["release right", "wait 400", "accessibility context"]);
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
        |modifiers: &str, key: &str| [modifiers, key, "modifiers", "wait 800"].map(String::from);
    let mut steps = Vec::new();
    for title in ["type One", "type Two"] {
        steps.extend(chord("modifiers command", "key n"));
        steps.extend([title, "wait 300"].map(String::from));
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
        .chain([
            format!("type {query}"),
            "wait 2500".into(),
            format!("accessibility {name}"),
        ])
        .collect()
}

/// Steps opening the page versions notebook's first version and its bar's menu.
fn version_menu() -> Vec<String> {
    let mut steps: Vec<String> = ["modifiers command shift", "key p", "modifiers"]
        .into_iter()
        .chain(["type Page Versions", "wait 300", "key Enter", "wait 1000"])
        .map(String::from)
        .collect();
    // The version row under the page, then the yellow bar above the version.
    for point in ["1003 114", "177 94"] {
        steps.extend([format!("move {point}"), "press".into(), "release".into()]);
        steps.push("wait 1200".into());
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
    let mut steps: Vec<String> = ["modifiers command", "key n", "modifiers", "wait 1200"]
        .into_iter()
        // The Meeting tile of the template strip over the new page.
        .chain(["move 615 287", "press", "release", "wait 3000"])
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
    steps.extend(["key Down", "key Enter", "wait 2000"].map(String::from));
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
    // Copy Page To…, then the section itself.
    steps.extend(["key Down", "key Down", "key Down", "key Enter", "wait 600"].map(String::from));
    steps.extend(["key Down", "key Enter", "wait 2000", "accessibility copied"].map(String::from));
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

/// Rename on a section tab's menu opens the shut sidebar on a field that takes what is
/// typed over the old name at once.
#[test]
fn renaming_a_section_from_its_tab_types_into_the_sidebar_at_once() {
    let scratch = Scratch::new("rename-shut-sidebar");
    let notebook =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/cross-container/candidate");
    let mut steps = vec!["move 53 53", "press right", "release right", "wait 400"];
    steps.extend(["key Down", "key Enter", "wait 400"]);
    steps.extend(["type Renamed", "wait 100", "accessibility typed"]);
    let [typed] = replay(&scratch, Some(&notebook), &steps)
        .try_into()
        .unwrap();
    assert!(
        typed.contains(r#"TextInput "Name" = "Renamed" [focused]"#),
        "{typed}"
    );
}

/// Finding on the page leaves the query room beside the match count: a press just past
/// where the count ends lands in the field.
#[test]
fn the_find_bar_keeps_room_for_the_query() {
    let scratch = Scratch::new("find-room");
    let notebook =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/cross-container/candidate");
    let mut steps = vec!["modifiers command", "key f", "modifiers", "wait 200"];
    steps.extend(["type Alpha", "wait 400", "move 600 600", "press", "release"]);
    steps.extend(["wait 200", "move 1000 52", "press", "release", "wait 200"]);
    steps.extend(["type zz", "wait 200", "accessibility found"]);
    let [found] = replay(&scratch, Some(&notebook), &steps)
        .try_into()
        .unwrap();
    assert!(
        found.contains(r#"SearchInput "Find on Page" = "Alphazz" [focused]"#),
        "{found}"
    );
}
