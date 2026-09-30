//! Every place the canvas shows a page's content as a placeholder, draws it read-only, or
//! refuses an ordinary edit as content it cannot edit, across the active pages of every
//! section under the given directories, grouped by kind as Markdown. Identical sections
//! count once.
//!
//! `cargo run --release -p canvas --features gpu --example unsupported_inventory -- corpus`

use canvas::{
    document::TextPosition,
    editor::{CanvasEditor, EditorError, TextOutline},
    layout::{LayoutError, TextEngine},
    outline::Arrange,
};
use onestore::{
    Arena, ExGuid, Section,
    page::{
        Image, Outline, Page, PageObject, PageParagraph, Paragraph, ParagraphContent,
        text::{Affinity, EditError},
    },
};
use std::{
    collections::{BTreeMap, BTreeSet, hash_map::DefaultHasher},
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
};

#[derive(Default)]
struct Inventory {
    /// Kind to the places it occurs, as `section — page`.
    kinds: BTreeMap<(Stage, String), Vec<String>>,
    sections: usize,
    pages: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Stage {
    /// Drawn as an "Unsupported content" (or "Image unavailable") box.
    Placeholder,
    /// Drawn as stored, but the editor cannot hold it.
    ReadOnly,
    /// An ordinary edit the editor refuses as content it cannot edit.
    Refused,
    /// The page or section does not open at all.
    Failure,
}

impl Inventory {
    fn add(&mut self, stage: Stage, kind: impl Into<String>, place: &str) {
        self.kinds
            .entry((stage, kind.into()))
            .or_default()
            .push(place.to_owned());
    }
}

/// MS-ONE's names for the object kinds it documents, and the observed undocumented ones.
fn jcid_name(jcid: u32) -> String {
    let name = match jcid {
        0x60007 => "section",
        0x60008 => "page series",
        0x6000b => "page",
        0x6000c => "outline",
        0x6000d => "outline element",
        0x6000e => "rich text",
        0x60011 => "picture",
        0x60012 => "number list",
        0x60014 => "ink container",
        0x60019 => "outline group",
        0x60022 => "table",
        0x60023 => "table row",
        0x60024 => "table cell",
        0x6002c => "title",
        0x60035 => "embedded file",
        0x60037 => "page manifest",
        0x6003c => "version history",
        0x6003d => "version proxy",
        _ => "undocumented",
    };
    format!("jcid 0x{jcid:05x} ({name})")
}

fn unsupported_error(error: &EditorError) -> bool {
    matches!(
        error,
        EditorError::Layout(LayoutError::UnsupportedContent)
            | EditorError::Edit(EditError::UnsupportedContent)
    )
}

/// What a text paragraph holds that bears on editing it.
fn traits(text: &Paragraph) -> Vec<&'static str> {
    let spans = text.spans();
    let mut traits = Vec::new();
    if text.text().contains('\u{fffc}')
        || spans.iter().any(|s| s.format.embedded_object == Some(true))
    {
        traits.push("embedded object in text");
    }
    if spans.iter().any(|s| s.format.math == Some(true)) {
        traits.push("equation");
    }
    if spans.iter().any(|s| s.format.hyperlink == Some(true)) {
        traits.push("link");
    }
    if spans.iter().any(|s| s.format.hidden == Some(true)) {
        traits.push("hidden text");
    }
    if text.text().is_empty() {
        traits.push("empty");
    }
    traits
}

/// Why `node`, alone in an outline, does not lay out as content the canvas understands.
fn culprit(
    engine: &mut TextEngine,
    outline: &Outline,
    node: &PageParagraph,
    page: &Page,
) -> Option<String> {
    let alone = Outline {
        paragraphs: vec![PageParagraph {
            parent: None,
            level: 1,
            ..node.clone()
        }],
        unsupported: Vec::new(),
        ..outline.clone()
    };
    match alone.layout(engine, &page.definitions) {
        Err(LayoutError::UnsupportedContent) => {}
        _ => return None,
    }
    Some(match &node.content {
        ParagraphContent::Unsupported(u) => format!("holds {}", jcid_name(u.jcid)),
        ParagraphContent::Table(table) if !table.tags.is_empty() => "table with note tags".into(),
        ParagraphContent::Table(table) => {
            let cells = || table.rows.iter().flat_map(|row| &row.cells);
            if let Some(cell) = cells().find(|cell| !cell.unsupported.is_empty()) {
                format!(
                    "table cell holding {}",
                    cell.unsupported
                        .iter()
                        .map(|u| jcid_name(u.jcid))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            } else {
                let inner = cells()
                    .flat_map(|cell| &cell.paragraphs)
                    .find_map(|inner| culprit(engine, outline, inner, page));
                format!(
                    "table cell: {}",
                    inner.unwrap_or_else(|| "nested content".into())
                )
            }
        }
        ParagraphContent::Image(_) => "picture without a size".into(),
        ParagraphContent::Ink(_) => "handwriting without strokes".into(),
        ParagraphContent::Attachment(_) => "file".into(),
        ParagraphContent::Text(text) => {
            let lists: Vec<String> = node
                .lists
                .iter()
                .filter_map(|id| page.definitions.get(id))
                .map(|definition| format!("{:?}", definition.kind))
                .collect();
            if !lists.is_empty() {
                format!("list marker {}", lists.join(", "))
            } else if text
                .text
                .spans()
                .iter()
                .any(|s| s.format.math == Some(true))
            {
                "equation the math layout does not parse".into()
            } else {
                format!("text ({})", traits(&text.text).join(", "))
            }
        }
    })
}

/// Why an outline the page shows is not editable.
fn diagnose(engine: &mut TextEngine, outline: &Outline, page: &Page) -> Vec<String> {
    let mut reasons: Vec<String> = outline
        .unsupported
        .iter()
        .map(|u| format!("outline child {}", jcid_name(u.jcid)))
        .collect();
    reasons.extend(
        outline
            .paragraphs
            .iter()
            .filter_map(|node| culprit(engine, outline, node, page)),
    );
    if reasons.is_empty() {
        let text = outline.paragraphs.iter().any(|node| {
            node.text().is_some()
                || matches!(&node.content, ParagraphContent::Table(t)
                    if t.rows.iter().flat_map(|r| &r.cells).any(|c| !c.paragraphs.is_empty()))
        });
        reasons.push(if text {
            match TextOutline::from_outline(engine, outline, &page.definitions) {
                Err(error) => format!("editor: {error}"),
                Ok(_) => "outline on its own is editable".into(),
            }
        } else {
            let kinds: BTreeSet<&str> = outline
                .paragraphs
                .iter()
                .map(|node| match node.content {
                    ParagraphContent::Image(_) => "pictures",
                    ParagraphContent::Attachment(_) => "files",
                    ParagraphContent::Ink(_) => "handwriting",
                    _ => "other",
                })
                .collect();
            format!(
                "no paragraph of text (only {})",
                kinds.into_iter().collect::<Vec<_>>().join(", ")
            )
        });
    }
    reasons
}

/// A picture that draws as "Image unavailable": no data, or (with `--features gpu`) data
/// the renderer does not decode, named by its leading bytes.
fn picture(image: &Image, at: &str, inventory: &mut Inventory, place: &str) {
    let Some(bytes) = &image.bytes else {
        return inventory.add(
            Stage::Placeholder,
            format!("{at} picture without data"),
            place,
        );
    };
    #[cfg(feature = "gpu")]
    if [Some(bytes), image.display.as_ref()]
        .into_iter()
        .flatten()
        .all(|bytes| draw::RasterImage::measure(bytes).is_err())
    {
        let format = match &bytes[..bytes.len().min(4)] {
            [0x50, 0x4b, 3, 4] => "a zip package (XPS printout)",
            [1, 0, 0, 0] => "EMF",
            [0xd7, 0xcd, 0xc6, 0x9a] | [1, 0, 9, 0] => "WMF",
            [b'B', b'M', ..] => "BMP",
            _ => "an unknown format",
        };
        inventory.add(
            Stage::Placeholder,
            format!("{at} picture stored as {format}"),
            place,
        );
    }
    #[cfg(not(feature = "gpu"))]
    let _ = bytes;
}

/// Pictures that draw as "Image unavailable" and objects the model keeps aside.
fn scan_paragraphs(nodes: &[PageParagraph], at: &str, inventory: &mut Inventory, place: &str) {
    for node in nodes {
        match &node.content {
            ParagraphContent::Unsupported(u) => inventory.add(
                Stage::Placeholder,
                format!("{at} paragraph holding {}", jcid_name(u.jcid)),
                place,
            ),
            ParagraphContent::Image(image) => picture(image, at, inventory, place),
            ParagraphContent::Table(table) => {
                for cell in table.rows.iter().flat_map(|row| &row.cells) {
                    for u in &cell.unsupported {
                        inventory.add(
                            Stage::Placeholder,
                            format!("table cell child {}", jcid_name(u.jcid)),
                            place,
                        );
                    }
                    scan_paragraphs(&cell.paragraphs, "table cell", inventory, place);
                }
            }
            _ => {}
        }
    }
}

/// Ordinary edits at every text paragraph of every editable outline: typing at its start,
/// middle and end, Enter, Backspace at its start, Delete at its end, and deleting from its
/// middle into the next.
fn probe(
    editor: &mut CanvasEditor,
    engine: &mut TextEngine,
    inventory: &mut Inventory,
    place: &str,
) {
    let ids: Vec<ExGuid> = editor.outlines().iter().map(|o| o.id).collect();
    for id in ids {
        if editor.focus_outline(id).is_err() {
            continue;
        }
        let document = editor.active_outline().document();
        let containers = containers(document.nodes());
        let leaves: Vec<(Option<ExGuid>, Paragraph)> = document
            .text_nodes()
            .map(|node| {
                (
                    containers.get(&node.id).copied().flatten(),
                    node.text().unwrap().text.clone(),
                )
            })
            .collect();
        for (index, (container, text)) in leaves.iter().enumerate() {
            let length = text.text().encode_utf16().count() as u32;
            // The middle of what shows, where a click could put the caret.
            let Ok(projection) = text.project() else {
                continue;
            };
            let shown = projection.text();
            let middle = shown
                .text()
                .char_indices()
                .map(|(at, _)| at)
                .nth(shown.text().chars().count() / 2)
                .and_then(|at| shown.utf16_offset(at).ok())
                .and_then(|at| projection.source_offset(at, Affinity::Downstream).ok())
                .unwrap_or(length);
            let at = |offset| TextPosition {
                paragraph: index,
                offset,
            };
            let next = leaves.get(index + 1);
            let mut probes: Vec<(&str, [TextPosition; 2], Probe)> = vec![
                ("typing at a paragraph's start", [at(0); 2], Probe::Type),
                ("typing inside a paragraph", [at(middle); 2], Probe::Type),
                ("typing at a paragraph's end", [at(length); 2], Probe::Type),
                ("Enter inside a paragraph", [at(middle); 2], Probe::Enter),
                (
                    "Delete at a paragraph's end",
                    [at(length); 2],
                    Probe::Delete,
                ),
            ];
            if index > 0 {
                probes.push((
                    "Backspace at a paragraph's start",
                    [at(0); 2],
                    Probe::Backspace,
                ));
            }
            if let Some((_, next_text)) = next {
                let end = next_text.text().encode_utf16().count() as u32 / 2;
                probes.push((
                    "deleting a selection into the next paragraph",
                    [
                        at(middle),
                        TextPosition {
                            paragraph: index + 1,
                            offset: end,
                        },
                    ],
                    Probe::Backspace,
                ));
            }
            for (name, positions, kind) in probes {
                if editor.select(positions.into()).is_err() {
                    continue;
                }
                let result = match kind {
                    Probe::Type => editor.insert(engine, "x").map(|()| true),
                    Probe::Enter => editor.enter(engine, false).map(|()| true),
                    Probe::Delete => editor.delete(engine, false),
                    Probe::Backspace => editor.delete(engine, true),
                };
                match result {
                    Ok(true) => {
                        editor.undo(engine).ok();
                    }
                    Ok(false) => {}
                    Err(error) if unsupported_error(&error) => {
                        let show = |t: &Paragraph, o: u32| {
                            let b = t.byte_offset(o).unwrap_or(0);
                            let v = |s: &str| s.replace('\u{fdd0}', "⟨").replace('\u{fdef}', "⟩").replace('\u{fdee}', "|").replace('\u{fffc}', "¤");
                            format!("{}‸{}", v(&t.text()[..b]), v(&t.text()[b..]))
                        };
                        let a = show(text, positions[0].offset);
                        let b = if positions[1].paragraph != index { show(&leaves[positions[1].paragraph].1, positions[1].offset) } else { String::new() };
                        eprintln!("REFUSED {name} @ {place}\n   A: {a}\n   B: {b}");
                        let crossing = positions[0].paragraph != positions[1].paragraph
                            || name.starts_with("Backspace")
                            || name.starts_with("Delete");
                        let neighbour = match name {
                            n if n.starts_with("Backspace") => leaves.get(index - 1),
                            _ if crossing => next,
                            _ => None,
                        };
                        let mut why: Vec<&str> = traits(text);
                        if let Some((other, other_text)) = neighbour {
                            if other != container {
                                why.push("across a table cell boundary");
                            }
                            why.extend(traits(other_text).into_iter().map(|t| match t {
                                "embedded object in text" => "next to an embedded object",
                                "equation" => "next to an equation",
                                "link" => "next to a link",
                                t => t,
                            }));
                        }
                        if container.is_some() {
                            why.push("in a table cell");
                        }
                        why.sort();
                        why.dedup();
                        inventory.add(
                            Stage::Refused,
                            format!("{name} [{}]", why.join(", ")),
                            place,
                        );
                    }
                    Err(_) => {}
                }
            }
        }
    }
}

#[derive(Clone, Copy)]
enum Probe {
    Type,
    Enter,
    Delete,
    Backspace,
}

/// Each paragraph's table cell, `None` at the outline's top.
fn containers(nodes: &[PageParagraph]) -> BTreeMap<ExGuid, Option<ExGuid>> {
    let mut map = BTreeMap::new();
    let mut pending = vec![(None, nodes)];
    while let Some((container, nodes)) = pending.pop() {
        for node in nodes {
            map.insert(node.id, container);
            if let ParagraphContent::Table(table) = &node.content {
                for cell in table.rows.iter().flat_map(|row| &row.cells) {
                    pending.push((Some(cell.id), cell.paragraphs.as_slice()));
                }
            }
        }
    }
    map
}

fn page(engine: &mut TextEngine, page: Page, place: &str, inventory: &mut Inventory) {
    for object in &page.objects {
        match object {
            PageObject::Unsupported(u) => inventory.add(
                Stage::Placeholder,
                format!("page object {}", jcid_name(u.jcid)),
                place,
            ),
            PageObject::Image(image) => picture(image, "page", inventory, place),
            PageObject::Outline(outline) => {
                for u in &outline.unsupported {
                    inventory.add(
                        Stage::Placeholder,
                        format!("outline child {}", jcid_name(u.jcid)),
                        place,
                    );
                }
                scan_paragraphs(&outline.paragraphs, "outline", inventory, place);
            }
            PageObject::Title(title) => {
                for outline in &title.outlines {
                    scan_paragraphs(&outline.paragraphs, "title", inventory, place);
                }
            }
            _ => {}
        }
    }
    let mut editor = match CanvasEditor::from_page(page.clone(), engine) {
        Ok(editor) => editor,
        Err(error) => {
            inventory.add(
                Stage::Failure,
                format!("page does not open: {error}"),
                place,
            );
            return;
        }
    };
    for object in &page.objects {
        let (outlines, title) = match object {
            PageObject::Outline(outline) => (std::slice::from_ref(outline), None),
            PageObject::Title(title) => (title.outlines.as_slice(), Some(title)),
            _ => continue,
        };
        for outline in outlines {
            if editor.has_page_outline(outline.id)
                || editor.outlines().iter().any(|o| o.id == outline.id)
            {
                continue;
            }
            if let Some(title) = title {
                if title.date == Some(outline.id) {
                    if editor.date().is_none() {
                        inventory.add(Stage::ReadOnly, "page date the date editor declines", place);
                    }
                    continue;
                }
                if !outline.title {
                    continue;
                }
            }
            let placeholder = matches!(
                outline.layout(engine, &page.definitions),
                Err(LayoutError::UnsupportedContent)
            );
            let at = if title.is_some() { "title" } else { "outline" };
            for reason in diagnose(engine, outline, &page) {
                let stage = if placeholder {
                    Stage::Placeholder
                } else {
                    Stage::ReadOnly
                };
                let whole = if placeholder {
                    format!("whole {at} as a box: {reason}")
                } else {
                    format!("{at} read-only: {reason}")
                };
                inventory.add(stage, whole, place);
            }
        }
    }
    probe(&mut editor, engine, inventory, place);
}

fn section(engine: &mut TextEngine, path: &Path, root: &Path, inventory: &mut Inventory) {
    let Ok(image) = std::fs::read(path) else {
        return;
    };
    let name = path
        .strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string();
    let arena = Arena::default();
    let mut section = match Section::open(&arena, image) {
        Ok(section) => section,
        Err(error) => {
            inventory.add(
                Stage::Failure,
                format!("section does not open: {}", error.message),
                &name,
            );
            return;
        }
    };
    inventory.sections += 1;
    let pages = match section.pages() {
        Ok(pages) => pages,
        Err(error) => {
            inventory.add(
                Stage::Failure,
                format!("page list does not read: {}", error.message),
                &name,
            );
            return;
        }
    };
    for (space, title, _) in pages {
        inventory.pages += 1;
        let place = format!("{name} — {title}");
        match section.page(space) {
            Ok(model) => page(engine, model, &place, inventory),
            Err(error) => inventory.add(
                Stage::Failure,
                format!("page does not read: {}", error.message),
                &place,
            ),
        }
    }
}

fn sections(directory: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    let mut entries: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            sections(&path, out);
        } else if path.extension().is_some_and(|e| e == "one") {
            out.push(path);
        }
    }
}

fn main() {
    let roots: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();
    let mut engine = TextEngine::default();
    let mut inventory = Inventory::default();
    let mut seen = BTreeSet::new();
    let mut duplicates = 0;
    for root in &roots {
        let mut paths = Vec::new();
        sections(root, &mut paths);
        for path in paths {
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            let mut hasher = DefaultHasher::new();
            bytes.hash(&mut hasher);
            if !seen.insert(hasher.finish()) {
                duplicates += 1;
                continue;
            }
            section(&mut engine, &path, root, &mut inventory);
        }
    }
    println!("# Unsupported content inventory\n");
    println!(
        "{} distinct sections ({duplicates} identical copies skipped), {} pages, under {}.\n",
        inventory.sections,
        inventory.pages,
        roots
            .iter()
            .map(|r| format!("`{}`", r.display()))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let headings = [
        (Stage::Placeholder, "Drawn as a placeholder box"),
        (Stage::ReadOnly, "Drawn as stored, read-only"),
        (
            Stage::Refused,
            "Edits refused as content the editor cannot edit",
        ),
        (Stage::Failure, "Does not open"),
    ];
    for (stage, heading) in headings {
        let mut rows: Vec<(&String, &Vec<String>)> = inventory
            .kinds
            .iter()
            .filter(|((s, _), _)| *s == stage)
            .map(|((_, kind), places)| (kind, places))
            .collect();
        rows.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(b.0)));
        println!("## {heading}\n");
        if rows.is_empty() {
            println!("None.\n");
            continue;
        }
        println!("| Kind | Count | Pages | Examples |\n| --- | ---: | ---: | --- |");
        for (kind, places) in rows {
            let pages: BTreeSet<&String> = places.iter().collect();
            let examples: Vec<String> = pages.iter().take(3).map(|p| format!("`{p}`")).collect();
            println!(
                "| {} | {} | {} | {} |",
                kind.replace('|', "\\|"),
                places.len(),
                pages.len(),
                examples.join("<br>").replace('|', "\\|")
            );
        }
        println!();
    }
}
