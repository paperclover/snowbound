//! Copy and Paste of formatted content: a selection's paragraphs with the definitions they
//! name, which hosts put on the clipboard in Snowbound's own format beside HTML and text.

use super::*;
use onestore::page::text::new_id;

/// Paragraphs copied from a page, in document order with each parent before its children;
/// levels count from 1 and a paragraph whose parent was not copied has none.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Clip {
    pub paragraphs: Vec<PageParagraph>,
    /// The styles, lists and tags the paragraphs name.
    pub definitions: BTreeMap<ExGuid, Definition>,
}

impl Clip {
    pub(crate) fn new(
        mut paragraphs: Vec<PageParagraph>,
        definitions: &BTreeMap<ExGuid, Definition>,
    ) -> Self {
        let held: BTreeSet<ExGuid> = paragraphs.iter().map(|node| node.id).collect();
        let shift = paragraphs.iter().map(|node| node.level).min().unwrap_or(1) - 1;
        for node in &mut paragraphs {
            node.parent = node.parent.filter(|parent| held.contains(parent));
            node.level -= shift;
        }
        Self {
            definitions: ops::named(definitions, &[&paragraphs]),
            paragraphs,
        }
    }

    /// The text as shown, a line to each paragraph, table cells included.
    pub fn text(&self) -> String {
        leaves(&self.paragraphs, None)
            .map(|(.., node)| {
                let text = &node.text().unwrap().text;
                text.project().map_or_else(
                    |_| text.text().to_owned(),
                    |shown| shown.text().text().to_owned(),
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Snowbound's own clipboard format.
    pub fn encode(&self) -> String {
        serde_json::to_string(self).expect("a clip serializes")
    }

    /// None for a clip another version of Snowbound wrote in a shape this one can't read.
    pub fn decode(json: &str) -> Option<Self> {
        serde_json::from_str(json).ok()
    }
}

impl CanvasEditor {
    /// What Copy takes: the selection's paragraphs with the first and last cut to it, or the
    /// page selection's outlines top to bottom with an empty paragraph between, as OneNote
    /// 2010 copies them. None where nothing shows.
    pub fn clip(&self) -> Result<Option<Clip>, EditError> {
        let paragraphs = if self.page_selected() {
            let mut outlines = self
                .outlines
                .iter()
                .filter(|outline| !outline.title)
                .collect::<Vec<_>>();
            outlines.sort_by(|a, b| {
                let ([ax, ay], [bx, by]) = (a.origin(), b.origin());
                ay.total_cmp(&by).then(ax.total_cmp(&bx))
            });
            let mut paragraphs = Vec::new();
            for outline in outlines {
                if !paragraphs.is_empty() {
                    let empty = Paragraph::new(String::new(), Format::default());
                    paragraphs.push(crate::document::node(empty, Format::default())?);
                }
                paragraphs.extend_from_slice(outline.document.nodes());
            }
            paragraphs
        } else {
            let [anchor, focus] = self.selection().positions;
            self.active_outline()
                .document
                .selected(anchor.min(focus)..anchor.max(focus))?
        };
        let clip = Clip::new(paragraphs, &self.definitions);
        Ok((!clip.text().is_empty()).then_some(clip))
    }

    /// Pastes `clip` as one undo step: one paragraph's runs go in at the selection; more
    /// paragraphs go between the halves of the caret's paragraph, each keeping its
    /// formatting, style, list, tags and indentation below that paragraph's. A title takes
    /// one paragraph's text alone; more go into the body.
    pub fn paste_clip(&mut self, engine: &mut TextEngine, clip: Clip) -> Result<(), EditorError> {
        if self.page_selected() {
            return self.grouped(|editor| {
                editor.remove_page(engine, true)?;
                editor.paste_clip(engine, clip)
            });
        }
        if self.active_outline().title && clip.paragraphs.len() > 1 {
            self.leave_title_for_body(engine)?;
        } else if self.active_outline().title {
            // A title takes text alone, as OneNote 2010's does (lab, 2026-10-02).
            let language = leaves(&clip.paragraphs, None)
                .find_map(|(.., node)| node.text()?.text.spans()[0].format.language)
                .unwrap_or(0x409);
            return self.paste(engine, &clip.text(), language);
        }
        self.take_objects()?;
        if let Some(pasted) = self.across(engine, |editor, engine| {
            editor.paste_clip(engine, clip.clone())
        })? {
            return Ok(pasted);
        }
        let mut nodes = self.admit(clip)?;
        if nodes.is_empty() {
            return Ok(());
        }
        if let [node] = &nodes[..]
            && let Some(text) = node.text()
        {
            return self.replace(engine, vec![text.text.clone()]);
        }
        let [anchor, focus] = self.active_outline().selection.positions;
        let (start, end) = (anchor.min(focus), anchor.max(focus));
        let edge = Paragraph::new(String::new(), self.typing_format(start)?);
        let texts = nodes.iter().map(|node| {
            node.text()
                .map_or_else(|| edge.clone(), |text| text.text.clone())
        });
        let replacement = std::iter::once(edge.clone())
            .chain(texts)
            .chain([edge.clone()])
            .collect();
        let mut edit = self
            .active_outline()
            .document
            .replace(start..end, replacement)?;
        let head = &edit.replacement[0];
        let (parent, level) = (head.parent, head.level);
        for node in &mut nodes {
            if node.parent.is_none() {
                node.parent = parent;
            }
            node.level += level - 1;
        }
        let ends_in_text = nodes.last().is_some_and(|node| node.text().is_some());
        let last = nodes
            .last()
            .and_then(|node| node.text())
            .map_or(Ok(0), |text| text.text.utf16_offset(text.text.text().len()))?;
        let count = nodes.len();
        let added = leaves(&nodes, None).count();
        edit.replacement.splice(1..=count, nodes);
        let head = drop_empty_halves(&mut edit, count, ends_in_text);
        let paragraph = start.paragraph + added - usize::from(head);
        let caret = if ends_in_text {
            TextPosition {
                paragraph,
                offset: last,
            }
        } else {
            TextPosition {
                paragraph: paragraph + 1,
                offset: 0,
            }
        };
        self.commit(engine, edit, [caret; 2].into())
    }

    /// `clip`'s paragraphs under identities of their own, naming this page's definitions:
    /// a style or tag the page already has, else one added beside its own.
    fn admit(&mut self, clip: Clip) -> Result<Vec<PageParagraph>, EditError> {
        let mut renamed = BTreeMap::new();
        for (id, definition) in clip.definitions {
            let own = match definition.kind {
                Kind::Style { .. } => self.define_style(&definition)?,
                Kind::TagDefinition { .. } => self.define_tag(&definition)?,
                _ => {
                    let own = new_id()?;
                    self.definitions.insert(own, definition);
                    own
                }
            };
            renamed.insert(id, own);
        }
        let mut nodes = clip.paragraphs;
        renew(&mut nodes, &renamed, &self.active_outline().indents)?;
        Ok(nodes)
    }
}

/// Drops a half of the caret's paragraph around `count` pasted nodes where empty, as OneNote
/// 2010 pastes (lab, 2026-10-02), unless it holds children or, past what `ends_in_text`
/// pasted, the caret; true if the upper half went.
pub(super) fn drop_empty_halves(edit: &mut DocumentEdit, count: usize, ends_in_text: bool) -> bool {
    let gone = |edit: &DocumentEdit, at: usize| {
        let node = &edit.replacement[at];
        node.text().is_some_and(|text| text.text.text().is_empty())
            && !edit
                .replacement
                .iter()
                .any(|child| child.parent == Some(node.id))
    };
    if ends_in_text && gone(edit, count + 1) {
        edit.replacement.remove(count + 1);
    }
    let head = gone(edit, 0);
    if head {
        edit.replacement.remove(0);
    }
    head
}

/// Gives `nodes` and everything they hold new identities, keeping their tree, and points
/// their definitions at `renamed`'s, dropping what names none; a cell without indents takes
/// `indents`.
fn renew(
    nodes: &mut Vec<PageParagraph>,
    renamed: &BTreeMap<ExGuid, ExGuid>,
    indents: &[f32],
) -> Result<(), EditError> {
    let tags = |tags: &mut Vec<onestore::document::Tag>| {
        tags.retain_mut(|tag| match tag.definition {
            Some(id) => renamed
                .get(&id)
                .map(|own| tag.definition = Some(*own))
                .is_some(),
            None => true,
        });
    };
    // What a clip from elsewhere holds without its bytes can't be pasted; their
    // children lose their parent.
    nodes.retain(|node| match &node.content {
        ParagraphContent::Image(image) => image.bytes.is_some(),
        ParagraphContent::Attachment(file) => file.bytes.is_some(),
        ParagraphContent::Unsupported(_) => false,
        _ => true,
    });
    let mut parents = BTreeMap::new();
    for node in nodes.iter_mut() {
        let id = new_id()?;
        parents.insert(node.id, id);
        node.id = id;
        node.parent = node.parent.and_then(|parent| parents.get(&parent).copied());
        node.style = node.style.and_then(|style| renamed.get(&style).copied());
        node.lists = node
            .lists
            .iter()
            .filter_map(|list| renamed.get(list).copied())
            .collect();
        tags(&mut node.tags);
        node.media = Default::default();
        match &mut node.content {
            ParagraphContent::Text(text) => {
                text.id = new_id()?;
                text.date_field = None;
                tags(&mut text.tags);
            }
            ParagraphContent::Image(image) => {
                image.id = new_id()?;
                tags(&mut image.tags);
            }
            ParagraphContent::Attachment(file) => file.id = new_id()?,
            ParagraphContent::Ink(ink) => ink.id = new_id()?,
            ParagraphContent::Unsupported(unsupported) => unsupported.id = new_id()?,
            ParagraphContent::Table(table) => {
                table.id = new_id()?;
                tags(&mut table.tags);
                for row in &mut table.rows {
                    row.id = new_id()?;
                    for cell in &mut row.cells {
                        cell.id = new_id()?;
                        if cell.indents.is_empty() {
                            cell.indents = indents.to_vec();
                        }
                        renew(&mut cell.paragraphs, renamed, indents)?;
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
impl Clip {
    /// A line to each paragraph: its indentation, list, runs with their formatting, links,
    /// tags and style, then each table's cells below it.
    pub(super) fn outline(&self) -> Vec<String> {
        let mut lines = Vec::new();
        outline(&self.paragraphs, &self.definitions, "", &mut lines);
        lines
    }
}

#[cfg(test)]
fn outline(
    nodes: &[PageParagraph],
    definitions: &BTreeMap<ExGuid, Definition>,
    prefix: &str,
    lines: &mut Vec<String>,
) {
    use super::format::AUTOMATIC;
    for node in nodes {
        let mut line = format!("{prefix}{}", "  ".repeat(node.level as usize - 1));
        if let Some(kind @ Kind::List { .. }) =
            node.lists.last().map(|list| &definitions[list].kind)
        {
            match super::html::list_tag(kind) {
                ("ul", _) => line.push_str("• "),
                (_, kind) => line.push_str(&format!("{kind}. ")),
            }
        }
        let mut tags = node.tags.clone();
        match &node.content {
            ParagraphContent::Text(text) => {
                tags.extend(text.tags.iter().cloned());
                let shown = text.text.project().unwrap();
                let shown = shown.text();
                let mut byte = 0;
                for span in shown.spans() {
                    let fragment = &shown.text()[byte..span.end];
                    byte = span.end;
                    let format = &span.format;
                    let mut marks = Vec::new();
                    for (set, mark) in [
                        (format.bold, "b"),
                        (format.italic, "i"),
                        (format.underline, "u"),
                        (format.strike, "s"),
                        (format.superscript, "sup"),
                        (format.subscript, "sub"),
                        (format.hyperlink, "link"),
                    ] {
                        if set == Some(true) {
                            marks.push(mark.to_owned());
                        }
                    }
                    if let Some(color) = format.color.filter(|color| *color != AUTOMATIC) {
                        marks.push(format!("color={color:06x}"));
                    }
                    if let Some(color) = format.highlight.filter(|color| *color != AUTOMATIC) {
                        marks.push(format!("highlight={color:06x}"));
                    }
                    if let Some(font) = format.font.as_deref().filter(|font| *font != "Calibri") {
                        marks.push(format!("font={font}"));
                    }
                    if let Some(size) = format.font_size.filter(|size| *size != 11.0) {
                        marks.push(format!("size={size}"));
                    }
                    if marks.is_empty() {
                        line.push_str(fragment);
                    } else {
                        line.push_str(&format!("<{}>{fragment}</>", marks.join(" ")));
                    }
                }
                for link in super::link::links(&text.text, 0) {
                    line.push_str(&format!(" -> {}", link.target));
                }
            }
            ParagraphContent::Table(table) => {
                line.push_str(&format!(
                    "table {}x{}",
                    table.rows.len(),
                    table.columns.len()
                ));
            }
            ParagraphContent::Image(image) => line.push_str(&format!("picture {:?}", image.size)),
            _ => line.push_str("other"),
        }
        for tag in &tags {
            if let Some(Kind::TagDefinition { label, .. }) =
                tag.definition.map(|id| &definitions[&id].kind)
            {
                line.push_str(&format!(" [{}]", label.as_deref().unwrap_or("")));
            }
        }
        if let Some(Kind::Style {
            name: Some(name), ..
        }) = node.style.map(|id| &definitions[&id].kind)
        {
            line.push_str(&format!(" ({name})"));
        }
        lines.push(line);
        if let ParagraphContent::Table(table) = &node.content {
            for (row, cells) in table.rows.iter().enumerate() {
                for (column, cell) in cells.cells.iter().enumerate() {
                    let prefix = format!("{prefix}  | {row},{column}: ");
                    outline(&cell.paragraphs, definitions, &prefix, lines);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::format::{NoteTag, Toggle};
    use crate::editor::{Formatting, html_pieces};

    fn at(paragraph: usize, offset: u32) -> TextPosition {
        TextPosition { paragraph, offset }
    }

    fn editor(engine: &mut TextEngine, lines: &[&str]) -> CanvasEditor {
        let paragraphs = lines
            .iter()
            .map(|line| {
                Paragraph::new(
                    (*line).to_owned(),
                    Format {
                        font: Some("Calibri".into()),
                        font_size: Some(11.0),
                        ..Format::default()
                    },
                )
            })
            .collect();
        CanvasEditor::new(engine, TextDocument::new(paragraphs).unwrap(), 400.0).unwrap()
    }

    fn page(editor: &mut CanvasEditor) -> Clip {
        editor.select_page().unwrap();
        let clip = editor.clip().unwrap().unwrap();
        editor.select([at(0, 0); 2].into()).unwrap();
        clip
    }

    /// The report's case: a bold line copied, through Snowbound's own format, into a new
    /// paragraph stays bold.
    #[test]
    fn a_copied_bold_line_pastes_bold() {
        let mut engine = TextEngine::default();
        let mut editor = editor(&mut engine, &["Bold line", ""]);
        editor.select([at(0, 0), at(0, 9)].into()).unwrap();
        editor
            .format(&mut engine, Formatting::Toggle(Toggle::Bold))
            .unwrap();
        let clip = editor.clip().unwrap().unwrap();
        assert_eq!(clip.text(), "Bold line");
        let clip = Clip::decode(&clip.encode()).unwrap();
        editor.select([at(1, 0); 2].into()).unwrap();
        editor.paste_clip(&mut engine, clip).unwrap();
        assert_eq!(editor.selection().positions, [at(1, 9); 2]);
        assert_eq!(
            page(&mut editor).outline(),
            ["<b>Bold line</>", "<b>Bold line</>"]
        );
    }

    /// Part of a paragraph copies only what is selected, and pastes into the middle of
    /// another's text.
    #[test]
    fn a_partial_selection_pastes_inline() {
        let mut engine = TextEngine::default();
        let mut editor = editor(&mut engine, &["one two three", "ab"]);
        editor.select([at(0, 4), at(0, 7)].into()).unwrap();
        editor
            .format(&mut engine, Formatting::Toggle(Toggle::Italic))
            .unwrap();
        let clip = editor.clip().unwrap().unwrap();
        editor.select([at(1, 1); 2].into()).unwrap();
        editor.paste_clip(&mut engine, clip).unwrap();
        assert_eq!(
            page(&mut editor).outline(),
            ["one <i>two</> three", "a<i>two</>b"]
        );
    }

    /// Styles, lists, indentation, tags and tables go through Snowbound's own format to
    /// another page between the halves of the caret's paragraph, as one undo step.
    #[test]
    fn a_clip_carries_styles_lists_tags_and_tables_to_another_page() {
        let mut engine = TextEngine::default();
        let mut source = editor(&mut engine, &["Heading", "item one", "child", "tagged"]);
        let heading = Definition {
            kind: Kind::Style {
                name: Some("h1".into()),
                next: None,
            },
            format: Format {
                bold: Some(true),
                font: Some("Calibri".into()),
                font_size: Some(16.0),
                color: Some(0x0060_3a1f),
                ..Format::default()
            },
        };
        let mut apply = |editor: &mut CanvasEditor, selection: [TextPosition; 2], command| {
            editor.select(selection.into()).unwrap();
            editor.format(&mut engine, command).unwrap();
        };
        apply(&mut source, [at(0, 0); 2], Formatting::Style(heading));
        apply(&mut source, [at(1, 0), at(2, 5)], Formatting::Bullets);
        apply(&mut source, [at(2, 2); 2], Formatting::Indent);
        apply(
            &mut source,
            [at(3, 0); 2],
            Formatting::Tag(NoteTag::defaults()[0].clone(), 0),
        );
        source.select([at(3, 6); 2].into()).unwrap();
        source.insert_table(&mut engine, 2, 2).unwrap();
        source.insert(&mut engine, "a1").unwrap();
        let copied = page(&mut source);
        let expected = [
            "<b color=603a1f size=16>Heading</> (h1)",
            "  • item one",
            "    • child",
            "tagged [To Do]",
            "table 2x2",
            "  | 0,0: a1",
            "  | 0,1: ",
            "  | 1,0: ",
            "  | 1,1: ",
        ];
        assert_eq!(copied.outline(), expected);

        let mut target = editor(&mut engine, &["before after"]);
        target.select([at(0, 7); 2].into()).unwrap();
        let clip = Clip::decode(&copied.encode()).unwrap();
        target.paste_clip(&mut engine, clip).unwrap();
        let pasted = page(&mut target).outline();
        assert_eq!(pasted[0], "before ");
        assert_eq!(pasted[1..pasted.len() - 1], expected);
        assert_eq!(pasted.last().unwrap(), "after");
        target.page().unwrap();
        assert!(target.undo(&mut engine).unwrap());
        assert_eq!(page(&mut target).outline(), ["before after"]);
    }

    /// A page's HTML pasted into an empty paragraph takes its place, keeping its formatting
    /// and lists, as OneNote 2010 pastes (lab, 2026-10-02).
    #[test]
    fn pasted_html_keeps_formatting_and_lists() {
        let mut engine = TextEngine::default();
        let mut editor = editor(&mut engine, &[""]);
        let pieces = html_pieces(
            "<p><b>Bold</b> and <span style='color:#C00000'>red</span></p>\
             <ul><li>one<ul><li>two</li></ul></li></ul>",
            0x409,
            |_, _| None,
        );
        editor.paste_pieces(&mut engine, pieces).unwrap();
        assert_eq!(
            page(&mut editor).outline(),
            ["<b>Bold</> and <color=0000c0>red</>", "• one", "  • two"]
        );
    }
}
