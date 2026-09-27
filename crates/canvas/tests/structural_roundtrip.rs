//! Every structural edit the editor makes saves as the ops the editor recorded and rereads
//! as the model it saved: random Enter, Backspace, Delete, Tab,
//! Shift+Tab, list and paste edits, alone and several to a save, on every outline of the probe
//! section and of any sections named in `CANVAS_SWEEP_SECTIONS` (`:`-separated paths).

use canvas::{
    document::TextPosition,
    editor::{CanvasEditor, Formatting},
    layout::TextEngine,
};
use onestore::{
    Arena, ExGuid, RevisionIndex, Section, Store,
    document::Document,
    page::{Outline, Page, PageObject, PageParagraph, ParagraphContent},
};
use std::{collections::BTreeMap, path::Path};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    EnterStart,
    EnterMiddle,
    EnterEnd,
    Backspace,
    Delete,
    Tab,
    ShiftTab,
    TabMiddle,
    Paste,
    Bullets,
    Numbering,
}

const KINDS: [Kind; 11] = [
    Kind::EnterStart,
    Kind::EnterMiddle,
    Kind::EnterEnd,
    Kind::Backspace,
    Kind::Delete,
    Kind::Tab,
    Kind::ShiftTab,
    Kind::TabMiddle,
    Kind::Paste,
    Kind::Bullets,
    Kind::Numbering,
];

struct Random(u64);

impl Random {
    fn below(&mut self, bound: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % bound as u64) as usize
    }
}

fn pages(section: &[u8]) -> Vec<(ExGuid, Page)> {
    let store = Store::parse(section).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let mut spaces: Vec<_> = document
        .pages()
        .unwrap()
        .into_iter()
        .map(|(space, _)| space)
        .collect();
    spaces.dedup();
    spaces
        .into_iter()
        .filter_map(|space| Some((space, Page::from_space(&document, space).ok()?)))
        .collect()
}

/// Applies `kind` at a random paragraph of the focused outline; false when the editor
/// declines it.
fn apply(
    editor: &mut CanvasEditor,
    engine: &mut TextEngine,
    kind: Kind,
    random: &mut Random,
) -> bool {
    let count = editor.active_outline().document().paragraphs().count();
    let paragraph = random.below(count);
    let text = editor
        .active_outline()
        .document()
        .paragraphs()
        .nth(paragraph)
        .unwrap();
    let length = text.text().encode_utf16().count() as u32;
    let middle = || {
        let chars: Vec<_> = text.text().char_indices().map(|(at, _)| at).collect();
        let at = chars.get(chars.len() / 2).copied().unwrap_or(0);
        text.utf16_offset(at).unwrap()
    };
    let offset = match kind {
        Kind::EnterStart | Kind::Backspace | Kind::Tab | Kind::ShiftTab => 0,
        Kind::EnterEnd | Kind::Delete => length,
        Kind::EnterMiddle | Kind::TabMiddle | Kind::Paste | Kind::Bullets | Kind::Numbering => {
            middle()
        }
    };
    let position = TextPosition { paragraph, offset };
    if editor.select([position; 2].into()).is_err() {
        return false;
    }
    let result = match kind {
        Kind::EnterStart | Kind::EnterMiddle | Kind::EnterEnd => editor.enter(engine, false),
        Kind::Backspace => editor.delete(engine, true).map(|_| ()),
        Kind::Delete => editor.delete(engine, false).map(|_| ()),
        Kind::Tab | Kind::TabMiddle => editor.tab(engine, false),
        Kind::ShiftTab => editor.tab(engine, true),
        Kind::Paste => editor.paste(engine, "Line one\r\nLine two", 1033),
        Kind::Bullets => editor.format(engine, Formatting::Bullets),
        Kind::Numbering => editor.format(engine, Formatting::Numbering),
    };
    result.is_ok()
}

/// Every paragraph on the page, cells' included, by identity.
fn paragraphs(page: &Page) -> BTreeMap<ExGuid, PageParagraph> {
    fn walk(list: &[PageParagraph], out: &mut BTreeMap<ExGuid, PageParagraph>) {
        for paragraph in list {
            out.insert(paragraph.id, paragraph.clone());
            if let ParagraphContent::Table(table) = &paragraph.content {
                for cell in table.rows.iter().flat_map(|row| &row.cells) {
                    walk(&cell.paragraphs, out);
                }
            }
        }
    }
    let mut out = BTreeMap::new();
    for object in &page.objects {
        match object {
            PageObject::Outline(outline) => walk(&outline.paragraphs, &mut out),
            PageObject::Title(title) => {
                for outline in &title.outlines {
                    walk(&outline.paragraphs, &mut out);
                }
            }
            _ => {}
        }
    }
    out
}

/// The fields two values disagree on, as `name: saved -> reread`.
fn fields<T: serde::Serialize>(saved: &T, reread: &T) -> String {
    let (serde_json::Value::Object(a), serde_json::Value::Object(b)) = (
        serde_json::to_value(saved).unwrap(),
        serde_json::to_value(reread).unwrap(),
    ) else {
        return "differs".into();
    };
    a.iter()
        .filter(|(key, value)| b.get(*key) != Some(value))
        .map(|(key, value)| format!("{key}: {value} -> {}", b[key]))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The first field a reread paragraph lost, as a signature without identities.
fn difference(saved: &Page, reread: &Page) -> String {
    let (a, b) = (paragraphs(saved), paragraphs(reread));
    for (id, x) in &a {
        let Some(y) = b.get(id) else {
            return "a paragraph identity is missing".into();
        };
        macro_rules! field {
            ($($name:ident),*) => {$(
                if x.$name != y.$name {
                    return format!(
                        "{} {:?} reread as {:?}",
                        stringify!($name),
                        x.$name,
                        y.$name
                    );
                }
            )*};
        }
        if x.lists.len() == y.lists.len() && x.lists != y.lists {
            let shared = a
                .values()
                .any(|other| other.id != x.id && other.lists.iter().any(|l| x.lists.contains(l)));
            return format!("list identities (shared {shared})");
        }
        if x.format != y.format {
            return format!("paragraph format {}", fields(&x.format, &y.format));
        }
        field!(parent, level, style, lists, tags, collapsed);
        if let (Some(p), Some(q)) = (x.text(), y.text()) {
            if p.id != q.id {
                return "text identity".into();
            }
            if p.text.text() != q.text.text() {
                return "text".into();
            }
            if p.text.spans().len() != q.text.spans().len() {
                return format!(
                    "{} spans reread as {}",
                    p.text.spans().len(),
                    q.text.spans().len()
                );
            }
            for (s, t) in p.text.spans().iter().zip(q.text.spans()) {
                if s.format != t.format {
                    return format!("span {}", fields(&s.format, &t.format));
                }
            }
            if p != q {
                return "text object".into();
            }
        }
        if x != y {
            return "content".into();
        }
    }
    if a.len() != b.len() {
        return "extra paragraphs".into();
    }
    let kind = |definition: &onestore::page::Definition| match &definition.kind {
        onestore::document::Kind::List { .. } => "list",
        onestore::document::Kind::Style { .. } => "style",
        _ => "tag",
    };
    for (id, definition) in &saved.definitions {
        match reread.definitions.get(id) {
            None => return format!("{} definition missing", kind(definition)),
            Some(other) if other != definition => {
                return format!(
                    "{} definition {}",
                    kind(definition),
                    fields(&definition.format, &other.format)
                );
            }
            _ => {}
        }
    }
    if let Some(extra) = reread
        .definitions
        .iter()
        .find(|(id, _)| !saved.definitions.contains_key(id))
    {
        return format!("extra {} definition", kind(extra.1));
    }
    for (x, y) in saved.objects.iter().zip(&reread.objects) {
        if x != y {
            return match (x, y) {
                (PageObject::Outline(x), PageObject::Outline(y)) => format!(
                    "outline {}",
                    fields(
                        &Outline {
                            paragraphs: Vec::new(),
                            ..x.clone()
                        },
                        &Outline {
                            paragraphs: Vec::new(),
                            ..y.clone()
                        }
                    )
                ),
                _ => format!("page object {x:?}\n reread as {y:?}"),
            };
        }
    }
    format!(
        "page {} ({} objects reread as {})",
        fields(
            &Page {
                objects: Vec::new(),
                definitions: Default::default(),
                ..saved.clone()
            },
            &Page {
                objects: Vec::new(),
                definitions: Default::default(),
                ..reread.clone()
            }
        ),
        saved.objects.len(),
        reread.objects.len()
    )
}

#[derive(Default)]
struct Tally {
    samples: usize,
    failures: BTreeMap<String, Vec<String>>,
}

fn sweep(path: &Path, random: &mut Random, engine: &mut TextEngine, tally: &mut Tally) {
    let section = std::fs::read(path).unwrap();
    for (space, page) in pages(&section) {
        let Ok(editor) = CanvasEditor::from_page(page.clone(), engine) else {
            continue;
        };
        let Ok(before) = editor.page() else {
            continue;
        };
        let outlines: Vec<ExGuid> = editor
            .outlines()
            .iter()
            .filter(|outline| !outline.title)
            .map(|outline| outline.id)
            .collect();
        for outline in outlines {
            let plans: Vec<Vec<Kind>> = KINDS
                .iter()
                .map(|kind| vec![*kind])
                .chain((0..3).map(|_| {
                    (0..2 + random.below(3))
                        .map(|_| KINDS[random.below(KINDS.len())])
                        .collect()
                }))
                .collect();
            for plan in plans {
                let mut editor = CanvasEditor::from_page(page.clone(), engine).unwrap();
                editor.focus_outline(outline).unwrap();
                let mut applied = 0;
                for kind in &plan {
                    applied += usize::from(apply(&mut editor, engine, *kind, random));
                }
                let Ok(after) = editor.page() else { continue };
                if applied == 0 || after == before {
                    continue;
                }
                tally.samples += 1;
                if let Some(dir) = std::env::var_os("CANVAS_SWEEP_DUMP") {
                    let value = serde_json::json!({"section": path.canonicalize().unwrap(), "space": space, "page": after});
                    std::fs::write(
                        Path::new(&dir).join(format!("{}.json", tally.samples)),
                        serde_json::to_vec(&value).unwrap(),
                    )
                    .unwrap();
                }
                // An untitled page's title follows its first line as the writer stores it.
                let rereads = |reread: &Page| {
                    Page {
                        title: after.title.clone(),
                        ..reread.clone()
                    } == after
                };
                let arena = Arena::default();
                let mut stored = Section::open(&arena, section.clone()).unwrap();
                let edit = onestore::op::Edit {
                    at: 133_000_000_000_000_000,
                    ops: editor
                        .take_ops()
                        .unwrap()
                        .into_iter()
                        .map(|op| onestore::op::Op::Page { space, op })
                        .collect(),
                };
                let failure = match stored.apply("Sweep", &edit) {
                    Err(error) => Some(format!("ops: {error}")),
                    Ok(()) => {
                        let reread = stored.page(space).unwrap();
                        (!rereads(&reread)).then(|| format!("ops: {}", difference(&after, &reread)))
                    }
                };
                if let Some(failure) = failure {
                    tally.failures.entry(failure).or_default().push(format!(
                        "{plan:?} on {:?} in {}",
                        page.title,
                        path.display()
                    ));
                }
            }
        }
    }
}

#[test]
fn structural_edits_reread_as_saved() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut paths = vec![root.join("evidence/structural-edits/probe-section/probe.one")];
    if let Some(extra) = std::env::var_os("CANVAS_SWEEP_SECTIONS") {
        paths.extend(std::env::split_paths(&extra));
    }
    let seed = std::env::var("CANVAS_SWEEP_SEED").map_or(1, |seed| seed.parse().unwrap());
    let mut random = Random(seed);
    let mut engine = TextEngine::default();
    let mut tally = Tally::default();
    for path in &paths {
        sweep(path, &mut random, &mut engine, &mut tally);
    }
    let failed: usize = tally.failures.values().map(Vec::len).sum();
    let mut report = String::new();
    for (failure, samples) in &tally.failures {
        report += &format!("{} x {failure}\n  e.g. {}\n", samples.len(), samples[0]);
        if std::env::var_os("CANVAS_SWEEP_ALL").is_some() {
            for sample in &samples[1..] {
                report += &format!("  e.g. {sample}\n");
            }
        }
    }
    println!(
        "{} of {} edits reread as saved\n{report}",
        tally.samples - failed,
        tally.samples
    );
    assert_eq!(failed, 0, "{report}");
}

/// Where a scripted edit puts the caret: in the first paragraph containing the text, before
/// or after it.
enum Caret {
    Before(&'static str),
    After(&'static str),
}

/// A key or command the scripted edits press.
enum Press {
    Enter,
    Backspace,
    Delete,
    Tab,
    ShiftTab,
    Paste(&'static str),
    Numbering,
}

/// Opens the first page titled `title`, presses each `(caret, key)` in one editing session and
/// saves the page once, checking that it rereads as saved.
fn edit(section: &mut Vec<u8>, title: &str, keys: &[(Caret, Press)], engine: &mut TextEngine) {
    let (space, page) = pages(section)
        .into_iter()
        .find(|(_, page)| page.title == title)
        .unwrap();
    let mut editor = CanvasEditor::from_page(page, engine).unwrap();
    for (caret, key) in keys {
        let needle = match caret {
            Caret::Before(needle) | Caret::After(needle) => *needle,
        };
        let outline = editor
            .outlines()
            .iter()
            .find(|outline| {
                outline
                    .document()
                    .paragraphs()
                    .any(|p| p.text().contains(needle))
            })
            .unwrap()
            .id;
        editor.focus_outline(outline).unwrap();
        let (paragraph, text) = editor
            .active_outline()
            .document()
            .paragraphs()
            .enumerate()
            .find(|(_, p)| p.text().contains(needle))
            .unwrap();
        let at = text.text().find(needle).unwrap()
            + match caret {
                Caret::Before(_) => 0,
                Caret::After(_) => needle.len(),
            };
        let offset = text.utf16_offset(at).unwrap();
        editor
            .select([TextPosition { paragraph, offset }; 2].into())
            .unwrap();
        match key {
            Press::Enter => editor.enter(engine, false).unwrap(),
            Press::Backspace => assert!(editor.delete(engine, true).unwrap()),
            Press::Delete => assert!(editor.delete(engine, false).unwrap()),
            Press::Tab => editor.tab(engine, false).unwrap(),
            Press::ShiftTab => editor.tab(engine, true).unwrap(),
            Press::Paste(text) => editor.paste(engine, text, 1033).unwrap(),
            Press::Numbering => editor.format(engine, Formatting::Numbering).unwrap(),
        }
    }
    let after = editor.page().unwrap();
    let arena = Arena::default();
    let mut stored = Section::open(&arena, section.to_vec()).unwrap();
    let edit = onestore::op::Edit {
        at: 133_000_000_000_000_000,
        ops: editor
            .take_ops()
            .unwrap()
            .into_iter()
            .map(|op| onestore::op::Op::Page { space, op })
            .collect(),
    };
    stored.apply("Snowbound", &edit).unwrap();
    stored.seal().unwrap();
    let saved = stored.image();
    let (_, reread) = pages(&saved)
        .into_iter()
        .find(|(id, _)| *id == space)
        .unwrap();
    assert_eq!(
        Page {
            title: after.title.clone(),
            ..reread.clone()
        },
        after,
        "{title}"
    );
    *section = saved;
}

/// Group indents, pasted lines, splits and joins around note tags, headings, hyperlinks and
/// a recording link, and several of them in one save, written into the probe section and two
/// native feature sections. `CANVAS_STRUCTURAL_EXPORT` names a new directory receiving them
/// for a cold OneNote reopen (`corpus/structural-edit`).
#[test]
fn structural_edits_write_what_onenote_reads() {
    use Caret::{After, Before};
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let read = |path: &str| std::fs::read(root.join(path)).unwrap();
    let mut engine = TextEngine::default();
    let mut probe = read("evidence/structural-edits/probe-section/probe.one");
    for (title, keys) in [
        ("c6-plain", vec![(Before("Above"), Press::Tab)]),
        ("c6-first", vec![(Before("Target text"), Press::ShiftTab)]),
        (
            "c10-tab-parent",
            vec![(Before("Omega"), Press::Tab), (Before("Child"), Press::Tab)],
        ),
        ("c1-todo", vec![(Before("Target text"), Press::Enter)]),
        (
            "c1-bullet",
            vec![(After("Target "), Press::Paste("Line one\r\nLine two"))],
        ),
        ("c9-h1", vec![(After("ing"), Press::Enter)]),
        (
            "c2-q",
            vec![
                (Before("Target "), Press::Enter),
                (Before("Target "), Press::Backspace),
                (Before("Xtext"), Press::Backspace),
            ],
        ),
        (
            "c3-q",
            vec![
                (After("Above"), Press::Enter),
                (After("Above"), Press::Delete),
            ],
        ),
        (
            "c4-imp",
            vec![
                (After("First li"), Press::Paste("Line one\r\nLine two")),
                (Before("Line one"), Press::Backspace),
            ],
        ),
        ("c1-q", vec![(After("Target "), Press::Tab)]),
        (
            "c1-rem",
            vec![
                (Before("Target text"), Press::Numbering),
                (Before("Target text"), Press::Tab),
                (Before("Target text"), Press::ShiftTab),
            ],
        ),
    ] {
        edit(&mut probe, title, &keys, &mut engine);
    }
    let mut features = read("corpus/m6/native-features-01/notebook/Features.one");
    edit(
        &mut features,
        "Files and recording",
        &[(After("Recording "), Press::Enter)],
        &mut engine,
    );
    let mut links = read("corpus/m6/native-link-controls-01/notebook/Links.one");
    edit(
        &mut links,
        "Hyperlink boundary controls",
        &[
            (
                Before("\u{fddf}HYPERLINK \"https://example.invalid/label/0"),
                Press::Enter,
            ),
            (After("label/4\"label"), Press::Delete),
        ],
        &mut engine,
    );
    if let Some(directory) = std::env::var_os("CANVAS_STRUCTURAL_EXPORT") {
        let directory = Path::new(&directory);
        std::fs::create_dir(directory).unwrap();
        for (name, bytes) in [
            ("probe.one", &probe),
            ("Features.one", &features),
            ("Links.one", &links),
        ] {
            std::fs::write(directory.join(name), bytes).unwrap();
        }
    }
}
