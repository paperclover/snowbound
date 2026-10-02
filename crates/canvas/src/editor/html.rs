//! Clips as HTML, both ways. Copy writes what OneNote 2010 puts on the clipboard (lab,
//! 2026-10-02): every run's formatting inline on a `span`, lists as `ul` and `ol` nested by
//! level, tables bordered, pictures and note tags left out. Paste reads that and what other
//! applications write into paragraphs, lists, tables and pictures.

use super::Piece;
use super::clip::Clip;
use super::format::{AUTOMATIC, ListStyle, NUMBER_LIBRARY, list_definition};
use super::link::{field_code, links};
use super::table::COLUMN_WIDTH;
use crate::document::node;
use crate::layout::{DEFAULT_FONT, DEFAULT_FONT_SIZE};
use onestore::ExGuid;
use onestore::document::{Format, Kind};
use onestore::page::text::{EditError, Paragraph, new_id};
use onestore::page::{
    Definition, PageParagraph, ParagraphContent, Table, TableCell, TableColumn, TableRow,
};
use std::collections::BTreeMap;
use std::fmt::Write;

impl Clip {
    /// The clip as a web page whose fragment the comments OneNote's mark out hold.
    pub fn html(&self) -> String {
        let mut body = String::new();
        blocks(&self.paragraphs, &self.definitions, &mut body);
        format!(
            "<html>\n<head>\n<meta http-equiv=Content-Type content=\"text/html; charset=utf-8\">\n\
             <meta name=Generator content=\"Snowbound\">\n</head>\n<body>\n\
             <!--StartFragment-->\n{body}<!--EndFragment-->\n</body>\n</html>\n"
        )
    }
}

/// `nodes` as blocks: a list opens at a paragraph whose list differs from the one open at
/// its level, and a block's left margin steps it in from the list it sits in, which OneNote
/// and Word take its level from.
fn blocks(nodes: &[PageParagraph], definitions: &BTreeMap<ExGuid, Definition>, out: &mut String) {
    let mut open: Vec<(u32, &str)> = Vec::new();
    for node in nodes {
        let list = node
            .lists
            .last()
            .and_then(|id| definitions.get(id))
            .filter(|definition| matches!(definition.kind, Kind::List { .. }))
            .map(|definition| list_tag(&definition.kind));
        while let Some(&(level, tag)) = open.last()
            && (level > node.level || level == node.level && list.map(|(tag, _)| tag) != Some(tag))
        {
            let _ = writeln!(out, "</{tag}>");
            open.pop();
        }
        // A level's step in, past the list it sits in, as OneNote indents its nested lists.
        let indent = (node.level - open.last().map_or(1, |(level, _)| *level)) as f32 * 0.375;
        match (list, &node.content) {
            (Some((tag, kind)), ParagraphContent::Text(text)) => {
                if open.last().map(|(level, _)| *level) != Some(node.level) {
                    let _ = writeln!(
                        out,
                        "<{tag} type={kind} style='margin-left:{indent}in;margin-top:0in;margin-bottom:0in'>"
                    );
                    open.push((node.level, tag));
                }
                out.push_str("<li style='margin-top:0;margin-bottom:0'>");
                runs(&text.text, out);
                out.push_str("</li>\n");
            }
            (_, ParagraphContent::Text(text)) => {
                let _ = write!(out, "<p style='margin:0in;margin-left:{indent}in'>");
                runs(&text.text, out);
                out.push_str("</p>\n");
            }
            (_, ParagraphContent::Table(table)) => {
                out.push_str(
                    "<table border=1 cellpadding=0 cellspacing=0 style='border-collapse:collapse;\
                     border-style:solid;border-color:#A3A3A3;border-width:1pt'>\n",
                );
                for row in &table.rows {
                    out.push_str("<tr>\n");
                    for (cell, column) in row.cells.iter().zip(&table.columns) {
                        let _ = writeln!(
                            out,
                            "<td style='border-style:solid;border-color:#A3A3A3;border-width:1pt;\
                             vertical-align:top;width:{}pt;padding:4pt 4pt 4pt 4pt'>",
                            column.width
                        );
                        blocks(&cell.paragraphs, definitions, out);
                        out.push_str("</td>\n");
                    }
                    out.push_str("</tr>\n");
                }
                out.push_str("</table>\n");
            }
            _ => {}
        }
    }
    for (_, tag) in open.into_iter().rev() {
        let _ = writeln!(out, "</{tag}>");
    }
}

/// The element and `type` a stored list writes as.
pub(super) fn list_tag(kind: &Kind) -> (&'static str, &'static str) {
    let Kind::List { bullet, format, .. } = kind else {
        return ("ul", "disc");
    };
    if bullet.is_some() {
        let kind = match ListStyle::of(kind) {
            Some(ListStyle::Bullet(3)) => "circle",
            Some(ListStyle::Bullet(11 | 14)) => "square",
            _ => "disc",
        };
        return ("ul", kind);
    }
    let sequence = format
        .as_deref()
        .and_then(|format| format.split('\u{fffd}').nth(1)?.chars().next());
    let kind = match sequence.map(u32::from) {
        Some(1) => "I",
        Some(2) => "i",
        Some(3) => "A",
        Some(4) => "a",
        _ => "1",
    };
    ("ol", kind)
}

/// A paragraph's shown runs as spans, its links' around theirs.
fn runs(text: &Paragraph, out: &mut String) {
    let links = links(text, 0);
    let (mut byte, mut unit) = (0, 0);
    let mut shown = false;
    let mut space = true;
    for span in text.spans() {
        let fragment = &text.text()[byte..span.end];
        let start = unit;
        (byte, unit) = (span.end, unit + fragment.encode_utf16().count() as u32);
        if span.format.hidden == Some(true) || fragment.is_empty() {
            continue;
        }
        shown = true;
        let link = links.iter().find(|link| link.label.contains(&start));
        if let Some(link) = link {
            let _ = write!(out, "<a href=\"{}\">", escape(&link.target, &mut false));
        }
        let _ = write!(
            out,
            "<span style='{}'>{}</span>",
            css(&span.format),
            escape(fragment, &mut space)
        );
        if link.is_some() {
            out.push_str("</a>");
        }
    }
    if !shown {
        out.push_str("&nbsp;");
    }
}

fn css(format: &Format) -> String {
    let font = format.font.as_deref().unwrap_or(DEFAULT_FONT);
    let font = if font.contains(' ') {
        format!("\"{font}\"")
    } else {
        font.to_owned()
    };
    let mut css = format!(
        "font-family:{font};font-size:{}pt",
        format.font_size.unwrap_or(DEFAULT_FONT_SIZE)
    );
    let set = |value: Option<bool>| value == Some(true);
    match format.bold {
        Some(true) => css.push_str(";font-weight:bold"),
        Some(false) => css.push_str(";font-weight:normal"),
        None => {}
    }
    match format.italic {
        Some(true) => css.push_str(";font-style:italic"),
        Some(false) => css.push_str(";font-style:normal"),
        None => {}
    }
    match (set(format.underline), set(format.strike)) {
        (true, true) => css.push_str(";text-decoration:underline line-through"),
        (true, false) => css.push_str(";text-decoration:underline"),
        (false, true) => css.push_str(";text-decoration:line-through"),
        (false, false) if format.underline.is_some() || format.strike.is_some() => {
            css.push_str(";text-decoration:none")
        }
        (false, false) => {}
    }
    if set(format.superscript) {
        css.push_str(";vertical-align:super");
    } else if set(format.subscript) {
        css.push_str(";vertical-align:sub");
    }
    let hex = |color: u32| {
        let [red, green, blue, _] = color.to_le_bytes();
        format!("#{red:02X}{green:02X}{blue:02X}")
    };
    if let Some(color) = format.color.filter(|color| *color != AUTOMATIC) {
        let _ = write!(css, ";color:{}", hex(color));
    }
    if let Some(color) = format.highlight.filter(|color| *color != AUTOMATIC) {
        let _ = write!(css, ";background:{}", hex(color));
    }
    css.replace('\'', "&#39;")
}

/// Text as HTML keeps it: markup characters escaped, line breaks as `br`, and a space after
/// `space`, whether the text follows a space or starts its paragraph, unbreakable so it
/// doesn't collapse.
fn escape(text: &str, space: &mut bool) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\u{b}' | '\n' => escaped.push_str("<br>"),
            '\t' => escaped.push_str("&nbsp;&nbsp;&nbsp; "),
            ' ' if *space => escaped.push_str("&nbsp;"),
            c => escaped.push(c),
        }
        *space = c == ' ';
    }
    escaped
}

/// A web page fragment's paragraphs, lists and tables as clips, split around its pictures
/// in order, as OneNote 2010 pastes them (lab, 2026-09-30). Text without a font or size of
/// its own is Calibri 11 in `language`, an LCID, as pasted text is. `load` makes a picture's
/// piece from its source and its `width` and `height` in CSS pixels; one it can't, or one
/// in a table, is left out.
pub fn html_pieces(
    html: &str,
    language: u32,
    load: impl FnMut(&str, [Option<f32>; 2]) -> Option<Piece>,
) -> Vec<Piece> {
    let html = html
        .split_once("<!--StartFragment-->")
        .map_or(html, |(_, fragment)| fragment);
    let html = html
        .split_once("<!--EndFragment-->")
        .map_or(html, |(fragment, _)| fragment);
    let mut reader = Reader {
        base: Format {
            font: Some("Calibri".into()),
            font_size: Some(11.0),
            language: Some(language),
            ..Format::default()
        },
        load,
        pieces: Vec::new(),
        blocks: Vec::new(),
        tables: Vec::new(),
        lists: Vec::new(),
        item: None,
        open: Vec::new(),
        runs: Vec::new(),
        shown: false,
        space: None,
    };
    reader.read(html);
    reader.end_clip();
    reader.pieces
}

/// A paragraph or table read, before it becomes a clip's.
enum Block {
    Text {
        runs: Vec<(String, Format)>,
        /// How many lists it sits in, its own not counted.
        depth: usize,
        list: Option<ListStyle>,
    },
    Table {
        rows: Vec<Vec<Vec<Block>>>,
        widths: Vec<Option<f32>>,
        depth: usize,
    },
}

#[derive(Default)]
struct TableRead {
    rows: Vec<Vec<Vec<Block>>>,
    widths: Vec<Option<f32>>,
    /// The open cell's blocks.
    cell: Option<Vec<Block>>,
}

/// An open element: its name, the formatting it gives what it holds, and where a link's
/// field code went in the runs, if it is one.
struct Open {
    name: String,
    format: Format,
    link: Option<usize>,
}

struct Reader<F> {
    base: Format,
    load: F,
    pieces: Vec<Piece>,
    blocks: Vec<Block>,
    tables: Vec<TableRead>,
    lists: Vec<ListStyle>,
    /// The list of the item whose text has yet to come.
    item: Option<ListStyle>,
    open: Vec<Open>,
    runs: Vec<(String, Format)>,
    /// Whether the paragraph read so far shows anything.
    shown: bool,
    /// The formatting of white space after what it shows, which shows as one space if
    /// more follows.
    space: Option<Format>,
}

/// Elements that hold no text and have no closing tag.
const VOID: [&str; 9] = [
    "br", "img", "meta", "link", "input", "hr", "col", "wbr", "area",
];

/// Elements that begin and end a paragraph.
const BLOCKS: [&str; 20] = [
    "p",
    "div",
    "li",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "blockquote",
    "pre",
    "dt",
    "dd",
    "section",
    "article",
    "header",
    "footer",
    "figure",
    "figcaption",
    "address",
];

impl<F: FnMut(&str, [Option<f32>; 2]) -> Option<Piece>> Reader<F> {
    fn read(&mut self, html: &str) {
        // Elements whose content is not text: script, style, head and title.
        let mut hidden: Option<String> = None;
        let mut rest = html;
        while !rest.is_empty() {
            if let Some(after) = rest.strip_prefix("<!--") {
                rest = after.split_once("-->").map_or("", |(_, after)| after);
                continue;
            }
            let Some(after) = rest.strip_prefix('<') else {
                let end = rest.find('<').unwrap_or(rest.len());
                if hidden.is_none() {
                    self.text(&decode_entities(&rest[..end]));
                }
                rest = &rest[end..];
                continue;
            };
            let end = tag_end(after);
            let tag = &after[..end];
            rest = after.get(end + 1..).unwrap_or("");
            let (closing, tag) = match tag.strip_prefix('/') {
                Some(tag) => (true, tag),
                None => (false, tag),
            };
            let name = tag
                .split(|c: char| c.is_whitespace() || c == '/')
                .next()
                .unwrap_or_default()
                .to_ascii_lowercase();
            if let Some(until) = &hidden {
                if closing && name == *until {
                    hidden = None;
                }
                continue;
            }
            match (name.as_str(), closing) {
                ("script" | "style" | "head" | "title", false) => hidden = Some(name),
                (_, false) => self.start(&name, tag),
                (_, true) => self.end(&name),
            }
        }
    }

    fn start(&mut self, name: &str, tag: &str) {
        let block = BLOCKS.contains(&name);
        if block || matches!(name, "br" | "table" | "tr" | "td" | "th" | "ul" | "ol") {
            self.paragraph();
        }
        match name {
            "br" => return,
            "img" => {
                let css = |name| {
                    attribute(tag, name)
                        .and_then(|value| value.trim_end_matches("px").parse::<f32>().ok())
                        .filter(|pixels| *pixels > 0.0)
                };
                if self.tables.is_empty()
                    && let Some(picture) = attribute(tag, "src").and_then(|source| {
                        (self.load)(&decode_entities(source), [css("width"), css("height")])
                    })
                {
                    self.end_clip();
                    self.pieces.push(picture);
                }
                return;
            }
            "ul" | "ol" => {
                let kind = attribute(tag, "type")
                    .map(str::to_owned)
                    .or_else(|| style(tag, "list-style-type"));
                self.lists.push(list_style(name, kind.as_deref()));
            }
            "li" => {
                self.implicit_end("li");
                self.item = Some(self.lists.last().copied().unwrap_or(ListStyle::BULLET));
            }
            "p" => self.implicit_end("p"),
            "table" => self.tables.push(TableRead::default()),
            "tr" => {
                if let Some(table) = self.tables.last_mut() {
                    table.end_cell();
                    table.rows.push(Vec::new());
                }
            }
            "td" | "th" => {
                if let Some(table) = self.tables.last_mut() {
                    table.end_cell();
                    if table.rows.is_empty() {
                        table.rows.push(Vec::new());
                    }
                    let column = table.rows.last().unwrap().len();
                    if table.widths.len() <= column {
                        table.widths.resize(column + 1, None);
                    }
                    let width = style(tag, "width")
                        .and_then(|width| length(&width, 12.0))
                        .or_else(|| {
                            attribute(tag, "width")?
                                .parse::<f32>()
                                .ok()
                                .map(|px| px * 0.75)
                        });
                    table.widths[column] = table.widths[column].or(width);
                    table.cell = Some(Vec::new());
                }
            }
            _ => {}
        }
        if VOID.contains(&name) {
            return;
        }
        let current = self.format();
        let mut format = element_format(name, tag, &current);
        let link = (name == "a")
            .then(|| attribute(tag, "href"))
            .flatten()
            .map(|href| {
                if let Some(space) = self.space.take() {
                    self.push(' ', space);
                }
                let mut code = format.inherit(&current);
                code.hidden = Some(true);
                code.hyperlink = Some(true);
                code.hyperlink_label = Some(true);
                self.runs.push((field_code(&decode_entities(href)), code));
                format.hyperlink = Some(true);
                format.hyperlink_label = Some(true);
                self.runs.len() - 1
            });
        self.open.push(Open {
            name: name.to_owned(),
            format,
            link,
        });
    }

    fn end(&mut self, name: &str) {
        if BLOCKS.contains(&name) || matches!(name, "table" | "tr" | "td" | "th" | "ul" | "ol") {
            self.paragraph();
        }
        if let Some(at) = self.open.iter().rposition(|open| open.name == name) {
            for open in self.open.drain(at..) {
                // A link that showed nothing keeps no field code.
                if let Some(code) = open.link
                    && code + 1 == self.runs.len()
                {
                    self.runs.pop();
                }
            }
        }
        match name {
            "ul" | "ol" => {
                self.lists.pop();
            }
            "li" => self.item = None,
            "td" | "th" => {
                if let Some(table) = self.tables.last_mut() {
                    table.end_cell();
                }
            }
            "table" => {
                if let Some(mut table) = self.tables.pop() {
                    table.end_cell();
                    table.rows.retain(|row| !row.is_empty());
                    if !table.rows.is_empty() {
                        let block = Block::Table {
                            rows: table.rows,
                            widths: table.widths,
                            depth: self.lists.len(),
                        };
                        self.sink().push(block);
                    }
                }
            }
            _ => {}
        }
    }

    /// Ends an unclosed `name`, as a `p` or `li` is by the next, within the innermost list
    /// or table.
    fn implicit_end(&mut self, name: &str) {
        let boundary =
            |open: &Open| matches!(open.name.as_str(), "ul" | "ol" | "table" | "td" | "th");
        if let Some(at) = self
            .open
            .iter()
            .rposition(|open| open.name == name || boundary(open))
            && self.open[at].name == name
        {
            self.open.truncate(at);
        }
    }

    fn format(&self) -> Format {
        self.open.iter().fold(self.base.clone(), |format, open| {
            open.format.inherit(&format)
        })
    }

    fn text(&mut self, text: &str) {
        let format = self.format();
        for c in text.chars() {
            if c.is_whitespace() && c != '\u{a0}' {
                if self.shown && self.space.is_none() {
                    self.space = Some(format.clone());
                }
                continue;
            }
            if let Some(space) = self.space.take() {
                self.push(' ', space);
            }
            self.shown = true;
            self.push(if c == '\u{a0}' { ' ' } else { c }, format.clone());
        }
    }

    fn push(&mut self, c: char, format: Format) {
        match self.runs.last_mut() {
            Some((text, last)) if *last == format => text.push(c),
            _ => self.runs.push((c.to_string(), format)),
        }
    }

    /// Ends the paragraph being read, which goes in if it showed anything; a link's field
    /// code begun in one that didn't carries on into the next.
    fn paragraph(&mut self) {
        self.space = None;
        if !std::mem::take(&mut self.shown) {
            return;
        }
        let mut runs = std::mem::take(&mut self.runs);
        for open in &mut self.open {
            open.link = None;
        }
        let list = self.item.take();
        let depth = self.lists.len() - usize::from(list.is_some() && !self.lists.is_empty());
        runs.retain(|(text, _)| !text.is_empty());
        if runs.is_empty() {
            runs.push((String::new(), self.format()));
        }
        self.sink().push(Block::Text { runs, depth, list });
    }

    /// Where a block read goes: the open cell, else the clip.
    fn sink(&mut self) -> &mut Vec<Block> {
        match self.tables.last_mut() {
            Some(table) => table.cell.get_or_insert_with(Vec::new),
            None => &mut self.blocks,
        }
    }

    /// Makes what was read since the last picture a clip.
    fn end_clip(&mut self) {
        self.paragraph();
        let blocks = std::mem::take(&mut self.blocks);
        if blocks.is_empty() {
            return;
        }
        let mut definitions = BTreeMap::new();
        if let Ok(paragraphs) = nodes(blocks, &mut definitions)
            && !paragraphs.is_empty()
        {
            self.pieces.push(Piece::Clip(Clip {
                paragraphs,
                definitions,
            }));
        }
    }
}

impl TableRead {
    fn end_cell(&mut self) {
        if let Some(cell) = self.cell.take() {
            match self.rows.last_mut() {
                Some(row) => row.push(cell),
                None => self.rows.push(vec![cell]),
            }
        }
    }
}

/// `blocks` as paragraphs, each the child of the one before it at a lower level, with the
/// list definitions they name added to `definitions`.
fn nodes(
    blocks: Vec<Block>,
    definitions: &mut BTreeMap<ExGuid, Definition>,
) -> Result<Vec<PageParagraph>, EditError> {
    let depth = |block: &Block| match block {
        Block::Text { depth, .. } | Block::Table { depth, .. } => *depth,
    };
    let least = blocks.iter().map(depth).min().unwrap_or(0);
    let mut nodes = Vec::new();
    let mut ancestors: Vec<(u32, ExGuid)> = Vec::new();
    for block in blocks {
        let level = (depth(&block) - least + 1) as u32;
        let mut paragraph = match block {
            Block::Text { runs, list, .. } => {
                let first = runs[0].1.clone();
                let mut paragraph = node(Paragraph::from_runs(runs), Format::default())?;
                if let Some(style) = list {
                    let id = new_id()?;
                    definitions.insert(id, list_definition(style, &first));
                    paragraph.lists = vec![id];
                }
                paragraph
            }
            Block::Table { rows, widths, .. } => table(rows, widths, definitions)?,
        };
        while ancestors.last().is_some_and(|(above, _)| *above >= level) {
            ancestors.pop();
        }
        paragraph.parent = ancestors.last().map(|(_, id)| *id);
        paragraph.level = level;
        ancestors.push((level, paragraph.id));
        nodes.push(paragraph);
    }
    Ok(nodes)
}

fn table(
    rows: Vec<Vec<Vec<Block>>>,
    widths: Vec<Option<f32>>,
    definitions: &mut BTreeMap<ExGuid, Definition>,
) -> Result<PageParagraph, EditError> {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(1).max(1);
    let rows = rows
        .into_iter()
        .map(|mut cells| {
            cells.resize_with(columns, Vec::new);
            Ok(TableRow {
                id: new_id()?,
                cells: cells
                    .into_iter()
                    .map(|blocks| {
                        let mut paragraphs = nodes(blocks, definitions)?;
                        if paragraphs.is_empty() {
                            let empty = Paragraph::new(String::new(), Format::default());
                            paragraphs.push(node(empty, Format::default())?);
                        }
                        Ok(TableCell {
                            id: new_id()?,
                            layout: Default::default(),
                            indents: Vec::new(),
                            shading: None,
                            paragraphs,
                            unsupported: Vec::new(),
                        })
                    })
                    .collect::<Result<_, EditError>>()?,
            })
        })
        .collect::<Result<_, EditError>>()?;
    let mut paragraph = node(
        Paragraph::new(String::new(), Format::default()),
        Format::default(),
    )?;
    paragraph.content = ParagraphContent::Table(Table {
        id: new_id()?,
        columns: (0..columns)
            .map(|column| TableColumn {
                width: widths
                    .get(column)
                    .copied()
                    .flatten()
                    .unwrap_or(COLUMN_WIDTH)
                    .max(COLUMN_WIDTH),
                locked: false,
            })
            .collect(),
        rows,
        borders: Some(true),
        layout: Default::default(),
        tags: Vec::new(),
    });
    Ok(paragraph)
}

/// The list style an `ul` or `ol` of `kind`, its `type` or `list-style-type`, gives.
fn list_style(name: &str, kind: Option<&str>) -> ListStyle {
    let number = |format: &str| {
        let index = NUMBER_LIBRARY.iter().position(|known| *known == format);
        ListStyle::Number(index.unwrap_or(0))
    };
    match (name, kind.map(str::trim)) {
        ("ul", Some("circle")) => ListStyle::Bullet(3),
        ("ul", Some("square")) => ListStyle::Bullet(11),
        ("ul", _) => ListStyle::BULLET,
        (_, Some("a" | "lower-alpha" | "lower-latin")) => number("\u{fffd}\u{4}."),
        (_, Some("A" | "upper-alpha" | "upper-latin")) => number("\u{fffd}\u{3}."),
        (_, Some("i" | "lower-roman")) => number("\u{fffd}\u{2}."),
        (_, Some("I" | "upper-roman")) => number("\u{fffd}\u{1}."),
        _ => ListStyle::NUMBER,
    }
}

/// The formatting an element gives its text: its own, as `b` and `font` give, then its
/// `style`'s. `current` is the formatting around it.
fn element_format(name: &str, tag: &str, current: &Format) -> Format {
    let mut format = Format::default();
    match name {
        "b" | "strong" => format.bold = Some(true),
        "i" | "em" | "cite" | "dfn" | "var" => format.italic = Some(true),
        "u" | "ins" => format.underline = Some(true),
        "s" | "strike" | "del" => format.strike = Some(true),
        "sup" => format.superscript = Some(true),
        "sub" => format.subscript = Some(true),
        "code" | "tt" | "pre" | "kbd" | "samp" => format.font = Some("Courier New".into()),
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
            const SIZES: [f32; 6] = [24.0, 18.0, 14.0, 12.0, 10.0, 8.0];
            format.bold = Some(true);
            format.font_size = Some(SIZES[usize::from(name.as_bytes()[1] - b'1')]);
        }
        "font" => {
            format.font = attribute(tag, "face").and_then(family);
            format.color = attribute(tag, "color").and_then(color).flatten();
            format.font_size = attribute(tag, "size")
                .and_then(|size| size.trim().parse::<usize>().ok())
                .map(|size| [8.0, 10.0, 12.0, 14.0, 18.0, 24.0, 36.0][size.clamp(1, 7) - 1]);
        }
        _ => {}
    }
    let Some(css) = attribute(tag, "style") else {
        return format;
    };
    let size = format
        .font_size
        .or(current.font_size)
        .unwrap_or(DEFAULT_FONT_SIZE);
    for declaration in decode_entities(css).split(';') {
        let Some((property, value)) = declaration.split_once(':') else {
            continue;
        };
        let value = value.trim().trim_end_matches("!important").trim();
        match property.trim().to_ascii_lowercase().as_str() {
            "font-weight" => {
                format.bold = match value {
                    "bold" | "bolder" => Some(true),
                    "normal" | "lighter" => Some(false),
                    weight => weight.parse::<u32>().ok().map(|weight| weight >= 600),
                }
            }
            "font-style" => format.italic = Some(matches!(value, "italic" | "oblique")),
            "text-decoration" | "text-decoration-line" => {
                format.underline = Some(value.contains("underline"));
                format.strike = Some(value.contains("line-through"));
            }
            "font-family" => format.font = family(value).or(format.font),
            "font-size" => format.font_size = length(value, size).or(format.font_size),
            "color" => {
                if let Some(color) = color(value) {
                    format.color = color;
                }
            }
            "background" | "background-color" | "mso-highlight" => {
                if let Some(color) = value.split_whitespace().find_map(color) {
                    format.highlight = color;
                }
            }
            "vertical-align" => match value {
                "super" => format.superscript = Some(true),
                "sub" => format.subscript = Some(true),
                _ => {}
            },
            _ => {}
        }
    }
    format
}

/// The first family a `font-family` names, unless it is a generic one or a browser's name
/// for the system's.
fn family(value: &str) -> Option<String> {
    let first = value
        .split(',')
        .next()?
        .trim()
        .trim_matches(['"', '\''])
        .trim();
    let generic = [
        "serif",
        "sans-serif",
        "monospace",
        "cursive",
        "fantasy",
        "system-ui",
        "inherit",
        "blinkmacsystemfont",
    ];
    let lower = first.to_ascii_lowercase();
    (!first.is_empty() && !first.starts_with('-') && !generic.contains(&lower.as_str()))
        .then(|| first.to_owned())
}

/// A CSS length in points, `em` and `%` of `size`.
fn length(value: &str, size: f32) -> Option<f32> {
    let value = value.trim();
    let keyword = match value {
        "xx-small" => Some(7.0),
        "x-small" => Some(7.5),
        "small" => Some(10.0),
        "medium" => Some(12.0),
        "large" => Some(13.5),
        "x-large" => Some(18.0),
        "xx-large" => Some(24.0),
        _ => None,
    };
    if keyword.is_some() {
        return keyword;
    }
    let split = value
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
        .unwrap_or(value.len());
    let (number, unit) = value.split_at(split);
    let number: f32 = number.parse().ok()?;
    let points = match unit.trim().to_ascii_lowercase().as_str() {
        "pt" => number,
        "px" | "" => number * 0.75,
        "in" => number * 72.0,
        "cm" => number * 72.0 / 2.54,
        "mm" => number * 72.0 / 25.4,
        "pc" => number * 12.0,
        "em" | "rem" => number * size,
        "%" => number * size / 100.0,
        _ => return None,
    };
    (points.is_finite() && points > 0.0).then_some(points)
}

/// A CSS colour as a COLORREF: `Some(None)` for black and the defaults, which pasted text
/// takes as automatic so it follows the page; none where it isn't one.
fn color(value: &str) -> Option<Option<u32>> {
    let value = value.trim().to_ascii_lowercase();
    let rgb = |red: u32, green: u32, blue: u32| Some(red | green << 8 | blue << 16);
    let parsed = if let Some(hex) = value.strip_prefix('#') {
        let digits: Vec<u32> = hex.chars().map(|c| c.to_digit(16)).collect::<Option<_>>()?;
        match digits[..] {
            [r, g, b] => rgb(r * 17, g * 17, b * 17),
            [r1, r0, g1, g0, b1, b0] => rgb(r1 * 16 + r0, g1 * 16 + g0, b1 * 16 + b0),
            _ => return None,
        }
    } else if let Some(arguments) = value
        .strip_prefix("rgb(")
        .or_else(|| value.strip_prefix("rgba("))
    {
        let channels: Vec<u32> = arguments
            .trim_end_matches(')')
            .split(',')
            .take(3)
            .map(|channel| {
                channel
                    .trim()
                    .parse::<f32>()
                    .ok()
                    .map(|c| c.clamp(0.0, 255.0) as u32)
            })
            .collect::<Option<_>>()?;
        match channels[..] {
            [r, g, b] => rgb(r, g, b),
            _ => return None,
        }
    } else {
        match value.as_str() {
            "black" | "windowtext" | "auto" | "inherit" | "initial" | "currentcolor" | "none"
            | "transparent" => None,
            "white" => rgb(255, 255, 255),
            "red" => rgb(255, 0, 0),
            "green" => rgb(0, 128, 0),
            "lime" => rgb(0, 255, 0),
            "blue" => rgb(0, 0, 255),
            "yellow" => rgb(255, 255, 0),
            "orange" => rgb(255, 165, 0),
            "purple" => rgb(128, 0, 128),
            "gray" | "grey" => rgb(128, 128, 128),
            "silver" => rgb(192, 192, 192),
            "maroon" => rgb(128, 0, 0),
            "navy" => rgb(0, 0, 128),
            "teal" => rgb(0, 128, 128),
            "olive" => rgb(128, 128, 0),
            "aqua" | "cyan" => rgb(0, 255, 255),
            "fuchsia" | "magenta" => rgb(255, 0, 255),
            "darkred" => rgb(139, 0, 0),
            "darkblue" => rgb(0, 0, 139),
            "darkgreen" => rgb(0, 100, 0),
            _ => return None,
        }
    };
    Some(parsed.filter(|color| *color != 0))
}

/// The value `name` takes in a tag's `style`.
fn style(tag: &str, name: &str) -> Option<String> {
    decode_entities(attribute(tag, "style")?)
        .split(';')
        .filter_map(|declaration| declaration.split_once(':'))
        .find(|(property, _)| property.trim().eq_ignore_ascii_case(name))
        .map(|(_, value)| value.trim().to_owned())
}

/// Where a tag's text ends, before its `>`, outside quoted attribute values.
fn tag_end(tag: &str) -> usize {
    let mut quote = None;
    for (at, c) in tag.char_indices() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(open), _) if c == open => quote = None,
            (None, '>') => return at,
            _ => {}
        }
    }
    tag.len()
}

/// The value of attribute `name` in `tag`, entities still encoded.
fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let mut rest = tag;
    while let Some(at) = rest.find('=') {
        let key = rest[..at].trim_end().rsplit(char::is_whitespace).next()?;
        let value = rest[at + 1..].trim_start();
        let (value, after) = match value.chars().next()? {
            quote @ ('"' | '\'') => value[1..].split_once(quote)?,
            _ => value.split_at(value.find(char::is_whitespace).unwrap_or(value.len())),
        };
        if key.eq_ignore_ascii_case(name) {
            return Some(value);
        }
        rest = after;
    }
    None
}

fn decode_entities(text: &str) -> String {
    let mut decoded = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        decoded.push_str(&rest[..at]);
        rest = &rest[at..];
        let entity = rest[1..]
            .find(';')
            .filter(|end| *end <= 10)
            .map(|end| &rest[1..end + 1]);
        let c = entity.and_then(|entity| match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some('\u{a0}'),
            _ => {
                let number = entity.strip_prefix('#')?;
                let code = match number.strip_prefix(['x', 'X']) {
                    Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                    None => number.parse().ok()?,
                };
                char::from_u32(code)
            }
        });
        match (c, entity) {
            (Some(c), Some(entity)) => {
                decoded.push(c);
                rest = &rest[entity.len() + 2..];
            }
            _ => {
                decoded.push('&');
                rest = &rest[1..];
            }
        }
    }
    decoded.push_str(rest);
    decoded
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clips(html: &str) -> Vec<Clip> {
        html_pieces(html, 0x409, |_, _| None)
            .into_iter()
            .map(|piece| match piece {
                Piece::Clip(clip) => clip,
                _ => panic!("a picture where none was loaded"),
            })
            .collect()
    }

    const ONENOTE: &str = include_str!("../../../../corpus/clipboard/onenote-2010.html");

    /// What OneNote 2010 copied (`corpus/clipboard`) pastes with its formatting, links,
    /// lists and table; its tag is not in its HTML.
    #[test]
    fn onenotes_html_pastes_with_its_formatting() {
        let [clip] = &clips(ONENOTE)[..] else {
            panic!("one clip")
        };
        assert_eq!(
            clip.outline(),
            [
                "<b>Bold line</>",
                "plain <i>italic</> <color=0000c0>red</> <size=16>big</> \
                 <font=Times New Roman>times</> <u>under</> <highlight=00ffff>hi</> \
                 <link>link</> -> https://example.com/",
                "• bullet one",
                "  • nested bullet",
                "• bullet two",
                "1. number one",
                "1. number two",
                "tagged todo",
                "after",
                "table 2x2",
                "  | 0,0: a1",
                "  | 0,1: <b>b1</>",
                "  | 1,0: a2",
                "  | 1,1: b2",
                "last line",
            ]
        );
    }

    /// Snowbound's HTML reads back as the clip it was written from.
    #[test]
    fn snowbound_html_reads_back_as_written() {
        let [clip] = &clips(ONENOTE)[..] else {
            panic!("one clip")
        };
        let written = clip.html();
        let [read] = &clips(&written)[..] else {
            panic!("one clip")
        };
        assert_eq!(read.outline(), clip.outline());
        let runs = |clip: &Clip| {
            crate::document::leaves(&clip.paragraphs, None)
                .map(|(.., node)| node.text().unwrap().text.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(runs(read), runs(clip));
    }

    /// Markup in text is escaped, spaces kept, and a paragraph that shows nothing still
    /// takes a line.
    #[test]
    fn text_survives_html() {
        let clip = &clips("<p>a &lt;b&gt; &amp;&nbsp;&nbsp;c</p><p>&nbsp;</p><p>d</p>")[0];
        assert_eq!(clip.text(), "a <b> &  c\n \nd");
        let read = &clips(&clip.html())[0];
        assert_eq!(read.text(), clip.text());
    }

    /// Other applications' markup: tags and inline styles, unclosed items, sizes in pixels
    /// and colours by name.
    #[test]
    fn web_markup_gives_formatting() {
        let clip = &clips(
            "<div style=\"font-family: Arial, sans-serif; font-size: 16px\">\
             <strong>Bold</strong> <em>it</em> <font color=red size=5>big</font> \
             <span style=\"font-weight:700; text-decoration:line-through\">gone</span>\
             x<sup>2</sup></div><ol type=a><li>first<li>second</ol>",
        )[0];
        assert_eq!(
            clip.outline(),
            [
                "<b font=Arial size=12>Bold</><font=Arial size=12> </>\
                 <i font=Arial size=12>it</><font=Arial size=12> </>\
                 <color=0000ff font=Arial size=18>big</><font=Arial size=12> </>\
                 <b s font=Arial size=12>gone</><font=Arial size=12>x</>\
                 <sup font=Arial size=12>2</>",
                "a. first",
                "a. second",
            ]
        );
    }

    /// As OneNote 2010 pasted `<p>before <img> after</p>` (lab, 2026-09-30): the text on
    /// either side of the picture, the picture at the size its attributes give; one in a
    /// table is left out.
    #[test]
    fn a_pasted_page_gives_its_text_and_pictures_in_order() {
        let html = "Version:0.9\r\n<html><head><style>p{}</style><title>T</title></head>\
            <body><!--StartFragment--><p>before <img alt=\"a > b\" \
            src=\"file:///C:/work/pic.png\" width=\"200\" height=\"100px\"> after</p>\
            <p>Fish &amp; chips&nbsp;&#x2014;&#8212;</p><ul><li>one<li>two<br>three</ul>\
            <table><tr><td><img src=\"cell.png\">cell</td></tr></table>\
            <img src=\"missing.png\"><IMG SRC='https://example.invalid/a.png' width=40>\
            <!--EndFragment--></body></html>";
        let mut asked = Vec::new();
        let pieces = html_pieces(html, 0x409, |source, css| {
            asked.push((source.to_owned(), css));
            match source {
                "missing.png" => None,
                "https://example.invalid/a.png" => Some(Piece::Awaited),
                _ => Some(Piece::Picture(vec![0; 3], [150.0, 75.0])),
            }
        });
        assert_eq!(
            asked,
            [
                (
                    "file:///C:/work/pic.png".to_owned(),
                    [Some(200.0), Some(100.0)]
                ),
                ("missing.png".to_owned(), [None, None]),
                (
                    "https://example.invalid/a.png".to_owned(),
                    [Some(40.0), None]
                ),
            ]
        );
        let shown: Vec<Vec<String>> = pieces
            .iter()
            .map(|piece| match piece {
                Piece::Clip(clip) => clip.outline(),
                Piece::Picture(bytes, size) => vec![format!("[{} {size:?}]", bytes.len())],
                Piece::Awaited => vec!["[awaited]".to_owned()],
            })
            .collect();
        assert_eq!(
            shown,
            [
                vec!["before"],
                vec!["[3 [150.0, 75.0]]"],
                vec![
                    "after",
                    "Fish & chips \u{2014}\u{2014}",
                    "• one",
                    "• two",
                    "  three",
                    "table 1x1",
                    "  | 0,0: cell",
                ],
                vec!["[awaited]"],
            ]
        );
    }
}
