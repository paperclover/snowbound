//! The ops the editor records for each edit, applied to the section, store what the model
//! oracle (`op::predict`) predicts from them, read back so once sealed, and hold the editor's
//! page or what it lowered whole stores: every editor operation alone and in random sequences with undo and redo, on every page of the corpus sections
//! below and of any named in `OPS_SWEEP_SECTIONS` (`:`-separated paths; the structural
//! probe section, which `structural_roundtrip` sweeps, takes minutes). The smoke slice takes
//! the first pages of each section.

use canvas::{
    document::TextPosition,
    editor::{Alignment, CanvasEditor, Formatting, NoteTag, Toggle, Whole},
    layout::TextEngine,
};
use draw::edit::Movement;
use onestore::{
    Arena, ExGuid, RevisionIndex, Section, Store,
    document::{Document, Kind as Node},
    op::{Edit, Op, PageOp},
    page::Page,
};
use std::{collections::BTreeMap, path::Path};

#[path = "../../onestore/tests/support/sweep.rs"]
mod sweep;

const SECTIONS: [&str; 18] = [
    "corpus/outline-edit/before/notebook/synthetic.one",
    "corpus/paragraph-edit/before/notebook/synthetic.one",
    "corpus/outline-edit/tree/before/notebook/synthetic.one",
    "corpus/m6/native-features-01/notebook/Features.one",
    "corpus/table-edit/nested/cold/notebook/synthetic.one",
    "corpus/m6/native-table-controls-01/notebook/synthetic.one",
    "corpus/list-edit/cold/notebook/lists.one",
    "corpus/tag-edit/cold/notebook/tags.one",
    "corpus/picture-edit/native-page-level/notebook/pictures.one",
    "corpus/attachment-edit/plain/cold/notebook/files.one",
    "corpus/ink-edit/drawing/cold/notebook/ink.one",
    "corpus/math-edit/native-editor/notebook/links.one",
    "corpus/link-edit/native-links/notebook/links.one",
    "corpus/link-edit/native-typed/notebook/links.one",
    "corpus/math-edit/native-editor-3/notebook/links.one",
    "corpus/math-edit/native-enter/notebook/links.one",
    "corpus/paragraph-format/cold/notebook/synthetic.one",
    "corpus/canvas/baseline-anchors.one",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    Type,
    TypeEnd,
    Compose,
    ComposeCancel,
    DeleteBackward,
    DeleteForward,
    DeleteRange,
    DeleteWord,
    EnterStart,
    EnterMiddle,
    EnterEnd,
    SoftEnter,
    Backspace,
    Delete,
    Tab,
    ShiftTab,
    TabMiddle,
    Paste,
    PasteLine,
    Bold,
    Italic,
    Font,
    FontSize,
    Color,
    Highlight,
    Clear,
    Center,
    Bullets,
    Numbering,
    Tag,
    Check,
    ClickCheck,
    MoveOutline,
    ResizeOutline,
    CreateOutline,
    EmptyOutline,
    ArrowPastEnd,
    PlaceImage,
    RemoveImage,
    Date,
    Link,
    Unlink,
    TypedUrl,
    Equation,
    Linear,
    Professional,
    PageDelete,
    PageType,
    PageMove,
    PageBold,
    InsertSpace,
    RemoveSpace,
    InsertSpaceRight,
    Undo,
    Redo,
}

const KINDS: [Kind; 55] = [
    Kind::Type,
    Kind::TypeEnd,
    Kind::Compose,
    Kind::ComposeCancel,
    Kind::DeleteBackward,
    Kind::DeleteForward,
    Kind::DeleteRange,
    Kind::DeleteWord,
    Kind::EnterStart,
    Kind::EnterMiddle,
    Kind::EnterEnd,
    Kind::SoftEnter,
    Kind::Backspace,
    Kind::Delete,
    Kind::Tab,
    Kind::ShiftTab,
    Kind::TabMiddle,
    Kind::Paste,
    Kind::PasteLine,
    Kind::Bold,
    Kind::Italic,
    Kind::Font,
    Kind::FontSize,
    Kind::Color,
    Kind::Highlight,
    Kind::Clear,
    Kind::Center,
    Kind::Bullets,
    Kind::Numbering,
    Kind::Tag,
    Kind::Check,
    Kind::ClickCheck,
    Kind::MoveOutline,
    Kind::ResizeOutline,
    Kind::CreateOutline,
    Kind::EmptyOutline,
    Kind::ArrowPastEnd,
    Kind::PlaceImage,
    Kind::RemoveImage,
    Kind::Date,
    Kind::Link,
    Kind::Unlink,
    Kind::TypedUrl,
    Kind::Equation,
    Kind::Linear,
    Kind::Professional,
    Kind::PageDelete,
    Kind::PageType,
    Kind::PageMove,
    Kind::PageBold,
    Kind::InsertSpace,
    Kind::RemoveSpace,
    Kind::InsertSpaceRight,
    Kind::Undo,
    Kind::Redo,
];

struct Random(u64);

impl Random {
    fn below(&mut self, bound: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % bound.max(1) as u64) as usize
    }
}

/// A random caret in the active outline: the paragraph and its UTF-16 length.
fn caret(editor: &CanvasEditor, random: &mut Random) -> (usize, u32) {
    let count = editor.active_outline().document().paragraphs().count();
    let paragraph = random.below(count);
    let text = editor
        .active_outline()
        .document()
        .paragraphs()
        .nth(paragraph)
        .unwrap();
    (paragraph, text.text().encode_utf16().count() as u32)
}

/// A scalar boundary of the paragraph near its middle, or a random one.
fn inside(editor: &CanvasEditor, paragraph: usize, random: &mut Random) -> u32 {
    let text = editor
        .active_outline()
        .document()
        .paragraphs()
        .nth(paragraph)
        .unwrap();
    let chars: Vec<usize> = text.text().char_indices().map(|(at, _)| at).collect();
    let at = chars
        .get(random.below(chars.len() + 1))
        .copied()
        .unwrap_or(text.text().len());
    text.utf16_offset(at).unwrap()
}

/// Performs `kind` somewhere on the page; false when the editor declines it.
fn perform(
    editor: &mut CanvasEditor,
    engine: &mut TextEngine,
    kind: Kind,
    random: &mut Random,
) -> bool {
    let outlines: Vec<ExGuid> = editor.outlines().iter().map(|outline| outline.id).collect();
    if !matches!(
        kind,
        Kind::Undo
            | Kind::Redo
            | Kind::PlaceImage
            | Kind::RemoveImage
            | Kind::CreateOutline
            | Kind::Date
    ) {
        if outlines.is_empty() {
            return false;
        }
        let outline = outlines[random.below(outlines.len())];
        if editor.focus_outline(outline).is_err() {
            return false;
        }
    }
    let at = |paragraph, offset| [TextPosition { paragraph, offset }; 2].into();
    let (paragraph, length) = if outlines.is_empty() {
        (0, 0)
    } else {
        caret(editor, random)
    };
    let middle = if outlines.is_empty() {
        0
    } else {
        inside(editor, paragraph, random)
    };
    let place = |editor: &mut CanvasEditor, offset| editor.select(at(paragraph, offset)).is_ok();
    let range = |editor: &mut CanvasEditor, random: &mut Random| {
        let (last, _) = caret(editor, random);
        let (first, last) = (paragraph.min(last), paragraph.max(last));
        let start = inside(editor, first, random);
        let end = inside(editor, last, random);
        let (start, end) = if first == last {
            (start.min(end), start.max(end))
        } else {
            (start, end)
        };
        editor
            .select(
                [
                    TextPosition {
                        paragraph: first,
                        offset: start,
                    },
                    TextPosition {
                        paragraph: last,
                        offset: end,
                    },
                ]
                .into(),
            )
            .is_ok()
    };
    let format =
        |editor: &mut CanvasEditor, engine: &mut TextEngine, random: &mut Random, command| {
            range(editor, random) && editor.format(engine, command).is_ok()
        };
    // Select All until it holds every body outline.
    let page = |editor: &mut CanvasEditor| {
        for _ in 0..16 {
            if editor.whole() == Some(Whole::Page) {
                return true;
            }
            if editor.widen_selection().is_err() {
                return false;
            }
        }
        false
    };
    match kind {
        Kind::Type => place(editor, middle) && editor.insert(engine, "ab").is_ok(),
        Kind::TypeEnd => place(editor, length) && editor.commit_text(engine, "z".into()).is_ok(),
        Kind::Compose => {
            place(editor, middle)
                && editor.compose(engine, "k".into(), 1..1).is_ok()
                && editor.compose(engine, "ka".into(), 2..2).is_ok()
                && editor.commit_text(engine, "か".into()).is_ok()
        }
        Kind::ComposeCancel => {
            place(editor, middle)
                && editor.compose(engine, "k".into(), 1..1).is_ok()
                && editor.cancel_composition(engine).is_ok()
        }
        Kind::DeleteBackward => place(editor, middle) && editor.delete(engine, true).is_ok(),
        Kind::DeleteForward => place(editor, middle) && editor.delete(engine, false).is_ok(),
        Kind::DeleteRange => range(editor, random) && editor.delete(engine, true).is_ok(),
        Kind::DeleteWord => {
            place(editor, middle) && editor.delete_to(engine, Movement::WordLeft).is_ok()
        }
        Kind::EnterStart => place(editor, 0) && editor.enter(engine, false).is_ok(),
        Kind::EnterMiddle => place(editor, middle) && editor.enter(engine, false).is_ok(),
        Kind::EnterEnd => place(editor, length) && editor.enter(engine, false).is_ok(),
        Kind::SoftEnter => place(editor, middle) && editor.enter(engine, true).is_ok(),
        Kind::Backspace => place(editor, 0) && editor.delete(engine, true).is_ok(),
        Kind::Delete => place(editor, length) && editor.delete(engine, false).is_ok(),
        Kind::Tab => place(editor, 0) && editor.tab(engine, false).is_ok(),
        Kind::ShiftTab => place(editor, 0) && editor.tab(engine, true).is_ok(),
        Kind::TabMiddle => place(editor, middle) && editor.tab(engine, false).is_ok(),
        Kind::Paste => {
            range(editor, random) && editor.paste(engine, "Line one\r\nLine two", 1033).is_ok()
        }
        Kind::PasteLine => place(editor, middle) && editor.paste(engine, "pasted", 1033).is_ok(),
        Kind::Bold => format(editor, engine, random, Formatting::Toggle(Toggle::Bold)),
        Kind::Italic => format(editor, engine, random, Formatting::Toggle(Toggle::Italic)),
        Kind::Font => format(editor, engine, random, Formatting::Font("Georgia".into())),
        Kind::FontSize => format(editor, engine, random, Formatting::FontSize(14.0)),
        Kind::Color => format(editor, engine, random, Formatting::Color(Some(0x0000_00ff))),
        Kind::Highlight => format(
            editor,
            engine,
            random,
            Formatting::Highlight(Some(0x0000_ffff)),
        ),
        Kind::Clear => format(editor, engine, random, Formatting::Clear),
        Kind::Center => format(editor, engine, random, Formatting::Align(Alignment::Center)),
        Kind::Bullets => format(editor, engine, random, Formatting::Bullets),
        Kind::Numbering => format(editor, engine, random, Formatting::Numbering),
        Kind::Tag => {
            let tag = [
                NoteTag::ToDo,
                NoteTag::Important,
                NoteTag::Question,
                NoteTag::RememberForLater,
                NoteTag::Definition,
                NoteTag::Highlight,
                NoteTag::Contact,
                NoteTag::Address,
                NoteTag::PhoneNumber,
            ][random.below(9)];
            format(editor, engine, random, Formatting::Tag(tag))
        }
        Kind::Check => format(editor, engine, random, Formatting::Check),
        Kind::ClickCheck => {
            let outline = editor.active_outline();
            let id = outline.document().text_nodes().nth(paragraph).unwrap().id;
            let outline = outline.id;
            editor.click_check(engine, outline, id).is_ok()
        }
        Kind::MoveOutline => {
            let outline = editor.active_outline();
            let [x, y] = outline.origin();
            let id = outline.id;
            editor.move_outline(id, [x + 36.0, y + 18.0]).is_ok()
        }
        Kind::ResizeOutline => {
            let width = editor.active_outline().wrap_width();
            editor.resize(engine, width + 72.0).is_ok()
        }
        Kind::CreateOutline => {
            editor
                .create_outline(
                    engine,
                    [400.0, 600.0 + 20.0 * random.below(10) as f32],
                    300.0,
                )
                .is_ok()
                && editor.insert(engine, "Fresh").is_ok()
        }
        Kind::EmptyOutline => editor.select_all().is_ok() && editor.delete(engine, true).is_ok(),
        Kind::ArrowPastEnd => {
            editor
                .move_selection(engine, Movement::DocumentEnd, false)
                .is_ok()
                && editor.move_selection(engine, Movement::Down, false).is_ok()
                && editor.move_selection(engine, Movement::Down, false).is_ok()
                && editor.insert(engine, "Below").is_ok()
        }
        Kind::PlaceImage | Kind::RemoveImage => {
            let images: Vec<ExGuid> = editor
                .object_layouts()
                .map(|(id, _)| id)
                .filter(|id| editor.image_placement(*id).is_some())
                .collect();
            if images.is_empty() {
                return false;
            }
            let id = images[random.below(images.len())];
            if kind == Kind::RemoveImage {
                return editor.remove_image(engine, id).is_ok();
            }
            let (origin, size) = editor.image_placement(id).unwrap();
            editor
                .place_image(
                    engine,
                    id,
                    [origin[0] + 18.0, origin[1] + 9.0],
                    [size[0] * 1.5, size[1] * 1.5],
                )
                .is_ok()
        }
        Kind::Date => {
            let Some(date) = editor.date() else {
                return false;
            };
            let timestamp = date.timestamp() + 864_000_000_000 * (1 + random.below(30) as u64);
            editor
                .change_date(
                    engine,
                    timestamp,
                    ["Friday, July 04, 2025".into(), "9:45 AM".into()],
                )
                .unwrap_or(false)
        }
        Kind::Link => {
            range(editor, random)
                && editor
                    .set_link(engine, "linked", "https://example.invalid/ops")
                    .is_ok()
        }
        Kind::Unlink => place(editor, middle) && editor.remove_link(engine).unwrap_or(false),
        Kind::TypedUrl => {
            place(editor, middle)
                && editor.insert(engine, " www.example.invalid").is_ok()
                && editor.insert(engine, " ").is_ok()
        }
        Kind::Equation => {
            place(editor, middle)
                && editor.insert_equation(engine).is_ok()
                && editor.insert(engine, "x^2").is_ok()
                && editor.insert(engine, " ").is_ok()
        }
        Kind::Linear => place(editor, middle) && editor.linear_equation(engine).unwrap_or(false),
        Kind::Professional => {
            place(editor, middle) && editor.build_equation(engine).unwrap_or(false)
        }
        Kind::PageDelete => page(editor) && editor.delete(engine, false).is_ok(),
        Kind::PageType => page(editor) && editor.insert(engine, "Z").is_ok(),
        Kind::PageMove => page(editor) && editor.move_page([36.0, 18.0]).is_ok(),
        Kind::PageBold => {
            page(editor)
                && editor
                    .format(engine, Formatting::Toggle(Toggle::Bold))
                    .is_ok()
        }
        Kind::InsertSpace | Kind::RemoveSpace => {
            // A line at a paragraph's top parts the outline there, or moves all of it.
            let outline = editor.active_outline();
            let paragraphs = &outline.shaped().paragraphs;
            let line = outline.origin()[1]
                + paragraphs[random.below(paragraphs.len())].origin[1]
                + [0.0, 0.5][random.below(2)];
            let delta = if kind == Kind::InsertSpace {
                36.0
            } else {
                -200.0
            };
            editor.insert_space(engine, 1, line, delta).unwrap_or(false)
        }
        Kind::InsertSpaceRight => {
            let line = editor.active_outline().origin()[0] - random.below(40) as f32;
            editor.insert_space(engine, 0, line, 45.0).unwrap_or(false)
        }
        Kind::Undo => editor.undo(engine).unwrap_or(false),
        Kind::Redo => editor.redo(engine).unwrap_or(false),
    }
}

/// How a step's ops compared with the editor's page.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Outcome {
    /// The ops store the editor's page, or what that page lowered whole stores.
    Same,
    /// Both refuse the edit.
    Refused,
    /// The ops store; the page lowered whole cannot reach the editor's page from what is
    /// stored.
    OpsOnly,
    /// The paths part where the section already stored the page differently from the
    /// editor's model, as the writers normalize some values, or where text keeps an insertion
    /// style the page model cannot show.
    Normalized,
    Differs(String),
}

/// The section the editor's ops are applied to, sealed after each step.
struct Stored<'a> {
    section: Section<'a>,
    space: ExGuid,
    at: u64,
}

/// The text objects of the page in `space` whose empty final run keeps an insertion style,
/// which inserting at their end takes and the page model cannot show.
fn hidden_styles(image: &[u8], space: ExGuid) -> Vec<ExGuid> {
    let store = Store::parse(image).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let space = &document.spaces[&space];
    space.revisions[&space.contexts[&ExGuid::default()]]
        .nodes
        .iter()
        .filter(|(_, node)| {
            matches!(&node.kind, Node::RichText { text, runs, .. }
                if !text.is_empty() && runs.len() > 1 && runs.last().is_some_and(|r| r.start == r.end))
        })
        .map(|(id, _)| *id)
        .collect()
}

/// A page as JSON without tags' property-set indices, which the writer assigns.
fn comparable(page: &Page) -> serde_json::Value {
    fn strip(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::Object(map) => {
                map.remove("extra_set");
                map.values_mut().for_each(strip);
            }
            serde_json::Value::Array(items) => items.iter_mut().for_each(strip),
            _ => {}
        }
    }
    let mut value = serde_json::to_value(page).unwrap();
    strip(&mut value);
    value
}

impl Stored<'_> {
    /// Stores the editor's step as its ops, checks the stored page against what the model
    /// oracle predicts from them and against the sealed image reread, and, unless it is the
    /// editor's page, compares with that page lowered whole onto the section as it was
    /// before the step.
    fn step(&mut self, editor: &mut CanvasEditor, model: &Page) -> Outcome {
        self.at += 10_000_000;
        let image = self.section.image();
        let before = self.section.page(self.space).unwrap();
        let normalized = !reads_as(&before, model);
        let ops = editor.take_ops().map_err(|error| error.to_string());
        let applied = ops.clone().and_then(|ops| {
            if ops.is_empty() {
                return Ok(());
            }
            let edit = Edit {
                at: self.at,
                ops: ops
                    .into_iter()
                    .map(|op| Op::Page {
                        space: self.space,
                        op,
                    })
                    .collect(),
            };
            self.section
                .apply("Sweep", &edit)
                .map_err(|error| error.to_string())
        });
        let stored = self.section.page(self.space).unwrap();
        let edited = editor.page().unwrap();
        let outcome = self.classify(image, &before, &stored, &edited, &ops, applied, normalized);
        if !matches!(outcome, Outcome::Same | Outcome::Refused)
            && std::env::var_os("OPS_SWEEP_DEBUG").is_some()
        {
            eprintln!("DIFFERS {outcome:?}\n  ops:");
            for op in ops.iter().flatten() {
                let op = format!("{op:?}");
                eprintln!("    {}", &op[..op.len().min(600)]);
            }
        }
        self.section.seal().unwrap();
        let arena = Arena::default();
        let reread = Section::open(&arena, self.section.image())
            .unwrap()
            .page(self.space)
            .unwrap();
        if reread != stored {
            return Outcome::Differs(format!(
                "reads back otherwise: {}",
                difference(&stored, &reread)
            ));
        }
        outcome
    }

    #[allow(clippy::too_many_arguments)]
    fn classify(
        &self,
        image: Vec<u8>,
        before: &Page,
        stored: &Page,
        edited: &Page,
        ops: &Result<Vec<PageOp>, String>,
        applied: Result<(), String>,
        normalized: bool,
    ) -> Outcome {
        if let Err(error) = &applied
            && stored != before
        {
            return Outcome::Differs(format!("a refused edit changed the page: {error}"));
        }
        if let (Ok(()), Ok(ops)) = (&applied, ops) {
            let mut predicted = before.clone();
            if let Err(error) = ops
                .iter()
                .try_for_each(|op| onestore::op::predict(&mut predicted, op))
            {
                return Outcome::Differs(format!(
                    "the model refuses what the section stores: {error}"
                ));
            }
            if comparable(&predicted) != comparable(stored) {
                let hidden = hidden_styles(&image, self.space);
                if ops.iter().any(|op| {
                    matches!(op, PageOp::Text { text, .. } | PageOp::Link { text, .. } | PageOp::Split { text, .. }
                        if hidden.contains(text))
                }) {
                    return Outcome::Normalized;
                }
                return Outcome::Differs(format!(
                    "the model predicts otherwise: {}",
                    difference(stored, &predicted)
                ));
            }
            if reads_as(stored, edited) {
                return Outcome::Same;
            }
        }
        let arena = Arena::default();
        let mut whole = Section::open(&arena, image).unwrap();
        let lowered = onestore::op::lower_page(before, edited)
            .map_err(|error| error.to_string())
            .and_then(|ops| {
                let edit = Edit {
                    at: self.at,
                    ops: ops
                        .into_iter()
                        .map(|op| Op::Page {
                            space: self.space,
                            op,
                        })
                        .collect(),
                };
                whole
                    .apply("Sweep", &edit)
                    .map_err(|error| error.to_string())
            });
        match (applied, lowered) {
            (Err(error), Err(whole)) => {
                if std::env::var_os("OPS_SWEEP_DEBUG").is_some() {
                    eprintln!("BOTH {error} | {whole}");
                }
                Outcome::Refused
            }
            (Ok(()), Ok(())) => {
                let whole = whole.page(self.space).unwrap();
                if *stored == whole {
                    Outcome::Same
                } else if normalized {
                    Outcome::Normalized
                } else {
                    Outcome::Differs(difference(stored, &whole))
                }
            }
            (Err(_), Ok(())) if normalized => Outcome::Normalized,
            (Err(error), Ok(())) => Outcome::Differs(format!("ops refused: {error}")),
            (Ok(()), Err(_)) => Outcome::OpsOnly,
        }
    }
}

fn reads_as(stored: &Page, model: &Page) -> bool {
    Page {
        title: model.title.clone(),
        ..stored.clone()
    } == *model
}

/// The first place two pages part: an object missing from one, then objects by identity,
/// then their order.
fn difference(a: &Page, b: &Page) -> String {
    let objects = |page: &Page| -> BTreeMap<ExGuid, String> {
        page.objects
            .iter()
            .map(|object| (object.id(), format!("{object:?}")))
            .collect()
    };
    let (x, y) = (objects(a), objects(b));
    if let Some(id) = x
        .keys()
        .chain(y.keys())
        .find(|id| x.contains_key(id) != y.contains_key(id))
    {
        return format!("object {id} on one side only");
    }
    for (id, x) in &x {
        let y = &y[id];
        if x != y {
            let at = x.bytes().zip(y.bytes()).take_while(|(p, q)| p == q).count();
            let from = at.saturating_sub(160);
            return format!(
                "object {id} differs\n    one:   …{}\n    other: …{}",
                &x[from..(at + 160).min(x.len())],
                &y[from..(at + 160).min(y.len())]
            );
        }
    }
    if a.objects
        .iter()
        .map(|o| o.id())
        .ne(b.objects.iter().map(|o| o.id()))
    {
        return "object order".into();
    }
    if a.definitions != b.definitions {
        return "definitions".into();
    }
    "page fields".into()
}

#[derive(Default)]
struct Tally {
    outcomes: BTreeMap<(Kind, Outcome), usize>,
    examples: BTreeMap<String, String>,
}

fn sweep(
    path: &Path,
    random: &mut Random,
    engine: &mut TextEngine,
    tally: &mut Tally,
    pages: usize,
) {
    let image = std::fs::read(path).unwrap();
    let arena = Arena::default();
    let mut section = Section::open(&arena, image.clone()).unwrap();
    for (space, title, _) in section.pages().unwrap().into_iter().take(pages) {
        let Ok(page) = section.page(space) else {
            continue;
        };
        if CanvasEditor::from_page(page.clone(), engine).is_err() {
            continue;
        }
        let plans = KINDS
            .iter()
            .map(|kind| vec![*kind])
            .chain(KINDS.iter().map(|kind| vec![*kind, Kind::Undo, Kind::Redo]))
            .chain((0..4).map(|_| {
                (0..3 + random.below(8))
                    .map(|_| KINDS[random.below(KINDS.len())])
                    .collect()
            }))
            .collect::<Vec<Vec<Kind>>>();
        for plan in plans {
            let arena = Arena::default();
            let mut stored = Stored {
                section: Section::open(&arena, image.clone()).unwrap(),
                space,
                at: 133_000_000_000_000_000,
            };
            let mut editor = CanvasEditor::from_page(page.clone(), engine).unwrap();
            for kind in &plan {
                let model = editor.page().unwrap();
                if !perform(&mut editor, engine, *kind, random) {
                    continue;
                }
                if std::env::var_os("OPS_SWEEP_DEBUG").is_some() {
                    eprintln!("STEP {kind:?} of {plan:?} on {title:?}");
                }
                let outcome = stored.step(&mut editor, &model);
                *tally.outcomes.entry((*kind, outcome.clone())).or_default() += 1;
                if let Outcome::Differs(reason) = &outcome {
                    tally
                        .examples
                        .entry(reason.clone())
                        .or_insert_with(|| format!("{plan:?} on {title:?} in {}", path.display()));
                }
                if outcome != Outcome::Same {
                    break;
                }
            }
        }
    }
}

#[test]
fn editor_ops_store_the_editor_s_page() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut paths: Vec<_> = SECTIONS.iter().map(|path| root.join(path)).collect();
    if let Some(extra) = std::env::var_os("OPS_SWEEP_SECTIONS") {
        paths.extend(std::env::split_paths(&extra));
    }
    let full = sweep::full();
    let pages = if full.is_some() { usize::MAX } else { 2 };
    let mut random = Random(1 + full.unwrap_or(0));
    let mut engine = TextEngine::default();
    let mut tally = Tally::default();
    for path in &paths {
        let start = std::time::Instant::now();
        sweep(path, &mut random, &mut engine, &mut tally, pages);
        if std::env::var_os("OPS_SWEEP_DEBUG").is_some() {
            eprintln!("TIME {:?} {}", start.elapsed(), path.display());
        }
    }
    let mut report = String::new();
    let mut per_kind: BTreeMap<Kind, [usize; 5]> = BTreeMap::new();
    for ((kind, outcome), count) in &tally.outcomes {
        let slot = match outcome {
            Outcome::Same => 0,
            Outcome::Refused => 1,
            Outcome::OpsOnly => 2,
            Outcome::Normalized => 3,
            Outcome::Differs(_) => 4,
        };
        per_kind.entry(*kind).or_default()[slot] += count;
    }
    let mut totals = [0; 5];
    for (kind, counts) in &per_kind {
        let [same, refused, ops_only, normalized, differs] = counts;
        report += &format!(
            "{kind:?}: {same} same, {refused} refused, {ops_only} by ops only, {normalized} after normalization, {differs} differ\n"
        );
        for (total, count) in totals.iter_mut().zip(counts) {
            *total += count;
        }
    }
    report +=
        &format!("all: {totals:?} (same, refused, by ops only, after normalization, differ)\n");
    for ((kind, outcome), count) in &tally.outcomes {
        if let Outcome::Differs(reason) = outcome {
            report += &format!(
                "{count} x {kind:?} {reason}\n  e.g. {}\n",
                tally.examples.get(reason).map_or("", String::as_str)
            );
        }
    }
    println!("{report}");
    let differs = totals[4];
    assert_eq!(differs, 0, "{report}");
    // Undoing stores what the edit it undoes stored, so storage never refuses it.
    for kind in [Kind::Undo, Kind::Redo] {
        assert_eq!(
            per_kind.get(&kind).map_or(0, |counts| counts[1]),
            0,
            "{report}"
        );
    }
}
