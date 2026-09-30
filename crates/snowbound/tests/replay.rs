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
