//! Two-dimensional layout for equations, from the tree `onestore::page::Math` parses out of
//! the linear text OneNote stores.

use crate::layout::{LayoutError, TextEngine, TextLayout};
use onestore::document::Format;
use onestore::page::Math;
use onestore::page::text::Paragraph;

/// An equation laid out for drawing; coordinates are relative to its top-left.
#[derive(Clone)]
pub struct MathLayout {
    pub size: [f32; 2],
    /// COLORREF for rules and strokes.
    pub color: u32,
    /// Distance from the top to the baseline of the equation's main row.
    pub baseline: f32,
    pub items: Vec<MathItem>,
}

#[derive(Clone)]
pub enum MathItem {
    Text {
        layout: TextLayout,
        origin: [f32; 2],
    },
    Rule([f32; 4]),
    /// A pen stroke, for radical signs.
    Stroke {
        from: [f32; 2],
        to: [f32; 2],
        width: f32,
    },
}

/// Items are relative to the box origin on its baseline; y grows downward.
struct Bx {
    width: f32,
    ascent: f32,
    descent: f32,
    items: Vec<MathItem>,
}

impl Bx {
    fn empty() -> Self {
        Self {
            width: 0.0,
            ascent: 0.0,
            descent: 0.0,
            items: Vec::new(),
        }
    }

    /// Places `other` with its baseline origin at `at` in this box's coordinates.
    fn place(&mut self, other: Bx, at: [f32; 2]) {
        self.ascent = self.ascent.max(other.ascent - at[1]);
        self.descent = self.descent.max(other.descent + at[1]);
        self.width = self.width.max(at[0] + other.width);
        self.items
            .extend(other.items.into_iter().map(|item| offset(item, at)));
    }

    fn append(&mut self, other: Bx) {
        let x = self.width;
        self.place(other, [x, 0.0]);
    }

    fn space(&mut self, width: f32) {
        self.width += width;
    }

    fn height(&self) -> f32 {
        self.ascent + self.descent
    }
}

fn offset(item: MathItem, [dx, dy]: [f32; 2]) -> MathItem {
    match item {
        MathItem::Text { layout, origin } => MathItem::Text {
            layout,
            origin: [origin[0] + dx, origin[1] + dy],
        },
        MathItem::Rule([x0, y0, x1, y1]) => MathItem::Rule([x0 + dx, y0 + dy, x1 + dx, y1 + dy]),
        MathItem::Stroke { from, to, width } => MathItem::Stroke {
            from: [from[0] + dx, from[1] + dy],
            to: [to[0] + dx, to[1] + dy],
            width,
        },
    }
}

const SCRIPT: f32 = 0.7;
const AXIS: f32 = 0.25;
const RULE: f32 = 0.06;

struct Context<'a> {
    engine: &'a mut TextEngine,
    color: Option<u32>,
}

impl Context<'_> {
    fn text(&mut self, text: &str, size: f32) -> Result<Bx, LayoutError> {
        let layout = self.engine.layout(
            &Paragraph::new(
                text.into(),
                Format {
                    font: Some("Cambria Math".into()),
                    font_size: Some(size),
                    color: self.color,
                    ..Format::default()
                },
            ),
            // Atoms never wrap.
            f32::MAX / 4.0,
        )?;
        let (line, bounds) = layout
            .lines()
            .next()
            .ok_or(LayoutError::InvalidFontMetrics)?;
        let (width, baseline) = (line.metrics().advance, bounds.baseline);
        // Math fonts' line boxes span their tallest glyphs; placement follows the ink instead.
        let [ascent, descent] = ink(&layout);
        Ok(Bx {
            width,
            ascent,
            descent,
            items: vec![MathItem::Text {
                layout,
                origin: [0.0, -baseline],
            }],
        })
    }

    fn row(&mut self, nodes: &[Math], size: f32) -> Result<Bx, LayoutError> {
        let mut result = Bx::empty();
        for (index, node) in nodes.iter().enumerate() {
            let spaced = match node {
                Math::Operator(c) if index > 0 => spacing(*c),
                _ => 0.0,
            } * size;
            result.space(spaced);
            result.append(self.node(node, size)?);
            if index + 1 < nodes.len() {
                result.space(spaced);
            }
        }
        Ok(result)
    }

    fn node(&mut self, node: &Math, size: f32) -> Result<Bx, LayoutError> {
        match node {
            Math::Identifier(c) => self.text(&italic(*c).to_string(), size),
            Math::Function(name) | Math::Number(name) => self.text(name, size),
            Math::Operator(' ') => {
                let mut space = Bx::empty();
                space.space(0.2 * size);
                Ok(space)
            }
            Math::Operator(c) => self.text(&c.to_string(), size),
            Math::Object {
                kind,
                symbols,
                columns,
                arguments,
            } => self.object(*kind, symbols, *columns, arguments, size),
        }
    }

    fn argument(
        &mut self,
        arguments: &[Vec<Math>],
        index: usize,
        size: f32,
    ) -> Result<Bx, LayoutError> {
        self.row(arguments.get(index).map_or(&[][..], Vec::as_slice), size)
    }

    fn object(
        &mut self,
        kind: u32,
        symbols: &[char],
        columns: Option<u8>,
        arguments: &[Vec<Math>],
        size: f32,
    ) -> Result<Bx, LayoutError> {
        let script = size * SCRIPT;
        Ok(match (kind, arguments.len()) {
            (31, 2) => self.scripts(None, Some(1), arguments, size)?,
            (29, 2) => self.scripts(Some(1), None, arguments, size)?,
            (30, 3) => self.scripts(Some(1), Some(2), arguments, size)?,
            (16, 2) | (26, 2) => {
                let numerator = self.argument(arguments, 0, size)?;
                let denominator = self.argument(arguments, 1, size)?;
                let width = numerator.width.max(denominator.width) + 0.2 * size;
                let (axis, rule, gap) = (AXIS * size, RULE * size, 0.12 * size);
                let mut result = Bx::empty();
                let top = -axis - rule / 2.0 - gap - numerator.descent;
                let bottom = -axis + rule / 2.0 + gap + denominator.ascent;
                result.place(numerator.centered(width), [0.0, top]);
                result.place(denominator.centered(width), [0.0, bottom]);
                result.items.push(MathItem::Rule([
                    0.0,
                    -axis - rule / 2.0,
                    width,
                    -axis + rule / 2.0,
                ]));
                result
            }
            (25, _) => {
                let body = self.argument(arguments, arguments.len().saturating_sub(1), size)?;
                let degree = (arguments.len() == 2 && !arguments[0].is_empty())
                    .then(|| self.row(&arguments[0], size * 0.5))
                    .transpose()?;
                radical(body, degree, size)
            }
            (19, 2) | (33, 2) => {
                let base = self.argument(arguments, 0, size)?;
                let limit = self.argument(arguments, 1, script)?;
                if kind == 19 {
                    stacked(base, Some(limit), None, size)
                } else {
                    stacked(base, None, Some(limit), size)
                }
            }
            (13, _) => {
                let body = self.argument(arguments, 0, size)?;
                let open = symbols
                    .first()
                    .map(|c| self.fence(*c, &body, size))
                    .transpose()?;
                let close = symbols
                    .get(1)
                    .map(|c| self.fence(*c, &body, size))
                    .transpose()?;
                let mut result = Bx::empty();
                for part in [open, Some(body), close].into_iter().flatten() {
                    result.append(part);
                }
                result
            }
            (10, 1) => {
                let base = self.argument(arguments, 0, size)?;
                let accent = self.text(
                    &spacing_accent(symbols.first().copied().unwrap_or('\u{302}')).to_string(),
                    size,
                )?;
                let mut result = Bx::empty();
                let width = base.width;
                // The accent's ink bottom sits just above the base's ink top.
                let lift = -base.ascent - 0.05 * size - accent.descent;
                result.place(base, [0.0, 0.0]);
                let x = (width - accent.width) / 2.0;
                result.place(accent, [x.max(0.0), lift]);
                result
            }
            (23, 1) => {
                let base = self.argument(arguments, 0, size)?;
                let (rule, gap) = (RULE * size, 0.1 * size);
                let mut result = Bx::empty();
                let top = -base.ascent - gap;
                let width = base.width;
                result.place(base, [0.0, 0.0]);
                result
                    .items
                    .push(MathItem::Rule([0.0, top - rule, width, top]));
                result.ascent = result.ascent.max(-(top - rule));
                result
            }
            (12, 1) => {
                let body = self.argument(arguments, 0, size)?;
                let (rule, pad) = (RULE * size, 0.12 * size);
                let mut result = Bx::empty();
                let [w, a, d] = [
                    body.width + 2.0 * pad,
                    body.ascent + pad,
                    body.descent + pad,
                ];
                result.place(body, [pad, 0.0]);
                for rect in [
                    [0.0, -a, w, -a + rule],
                    [0.0, d - rule, w, d],
                    [0.0, -a, rule, d],
                    [w - rule, -a, w, d],
                ] {
                    result.items.push(MathItem::Rule(rect));
                }
                result.ascent = result.ascent.max(a);
                result.descent = result.descent.max(d);
                result.width = w;
                result
            }
            (21, 3) => {
                let operator = symbols.first().copied().unwrap_or('∑');
                let integral = ('\u{222b}'..='\u{2233}').contains(&operator);
                // Display-size operators, as OneNote draws them.
                let scale = if integral { 1.9 } else { 1.75 };
                let glyph = self
                    .text(&operator.to_string(), size * scale)?
                    .on_axis(size);
                let lower = (!arguments[0].is_empty())
                    .then(|| self.row(&arguments[0], script))
                    .transpose()?;
                let upper = (!arguments[1].is_empty())
                    .then(|| self.row(&arguments[1], script))
                    .transpose()?;
                let mut result = if integral {
                    let mut result = Bx::empty();
                    let (ascent, descent, width) = (glyph.ascent, glyph.descent, glyph.width);
                    result.append(glyph);
                    let x = width + 0.05 * size;
                    if let Some(upper) = upper {
                        let y = -ascent + upper.ascent;
                        result.place(upper, [x, y]);
                    }
                    if let Some(lower) = lower {
                        let y = descent - lower.descent;
                        result.place(lower, [x - 0.2 * size, y]);
                    }
                    result
                } else {
                    stacked(glyph, lower, upper, size)
                };
                result.space(0.15 * size);
                result.append(self.argument(arguments, 2, size)?);
                result
            }
            (20, _) | (15, _) => {
                let width = if kind == 20 {
                    usize::from(columns.unwrap_or(1).max(1))
                } else {
                    1
                };
                let cells = arguments
                    .iter()
                    .map(|cell| {
                        let cell: Vec<Math> = cell
                            .iter()
                            .filter(|n| **n != Math::Operator('&'))
                            .cloned()
                            .collect();
                        self.row(&cell, size)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                grid(cells, width, kind == 20, size)
            }
            (11, 1) => self.argument(arguments, 0, size)?,
            _ => {
                let mut result = Bx::empty();
                for symbol in symbols {
                    result.append(self.text(&symbol.to_string(), size)?);
                }
                for index in 0..arguments.len() {
                    result.append(self.argument(arguments, index, size)?);
                }
                result
            }
        })
    }

    /// The first argument with the arguments at `lower` and `upper` as its scripts.
    fn scripts(
        &mut self,
        lower: Option<usize>,
        upper: Option<usize>,
        arguments: &[Vec<Math>],
        size: f32,
    ) -> Result<Bx, LayoutError> {
        let script = size * SCRIPT;
        let mut result = self.argument(arguments, 0, size)?;
        let x = result.width + 0.03 * size;
        let both = lower.is_some() && upper.is_some();
        if let Some(index) = upper {
            let upper = self.argument(arguments, index, script)?;
            result.place(upper, [x, -0.4 * size]);
        }
        if let Some(index) = lower {
            let lower = self.argument(arguments, index, script)?;
            result.place(lower, [x, if both { 0.3 } else { 0.2 } * size]);
        }
        Ok(result)
    }

    /// A delimiter tall enough for `body`, centered on the math axis.
    fn fence(&mut self, symbol: char, body: &Bx, size: f32) -> Result<Bx, LayoutError> {
        let axis = AXIS * size;
        let reach = (body.ascent - axis).max(body.descent + axis);
        let grown = (reach * 2.0 / (size * 1.15)).max(1.0) * size;
        Ok(self.text(&symbol.to_string(), grown)?.on_axis(size))
    }
}

impl Bx {
    /// This box moved vertically so its extent centers on the math axis.
    fn on_axis(self, size: f32) -> Bx {
        let shift = (self.ascent - self.descent) / 2.0 - AXIS * size;
        Bx {
            width: self.width,
            ascent: self.ascent - shift,
            descent: self.descent + shift,
            items: self
                .items
                .into_iter()
                .map(|i| offset(i, [0.0, shift]))
                .collect(),
        }
    }

    /// This box horizontally centered in `width`.
    fn centered(self, width: f32) -> Bx {
        let dx = (width - self.width) / 2.0;
        Bx {
            width,
            ascent: self.ascent,
            descent: self.descent,
            items: self
                .items
                .into_iter()
                .map(|i| offset(i, [dx, 0.0]))
                .collect(),
        }
    }
}

/// `base` with `lower` below and `upper` above, all centered on the widest.
fn stacked(base: Bx, lower: Option<Bx>, upper: Option<Bx>, size: f32) -> Bx {
    let gap = 0.08 * size;
    let width = [Some(&base), lower.as_ref(), upper.as_ref()]
        .into_iter()
        .flatten()
        .map(|b| b.width)
        .fold(0.0, f32::max);
    let (ascent, descent) = (base.ascent, base.descent);
    let mut result = Bx::empty();
    result.place(base.centered(width), [0.0, 0.0]);
    if let Some(lower) = lower {
        let y = descent + gap + lower.ascent;
        result.place(lower.centered(width), [0.0, y]);
    }
    if let Some(upper) = upper {
        let y = -ascent - gap - upper.descent;
        result.place(upper.centered(width), [0.0, y]);
    }
    result
}

/// A radical sign drawn with the pen over `body`, with an optional small `degree`.
fn radical(body: Bx, degree: Option<Bx>, size: f32) -> Bx {
    let (rule, gap) = (RULE * size, 0.12 * size);
    let top = -body.ascent - gap;
    let bottom = body.descent;
    let sign = 0.55 * size;
    let mut result = Bx::empty();
    let lead = degree
        .as_ref()
        .map_or(0.0, |d| (d.width - sign * 0.4).max(0.0));
    let [x0, x1, x2] = [lead, lead + sign * 0.35, lead + sign * 0.6];
    let mid = bottom - (bottom - top) * 0.45;
    for (from, to, width) in [
        ([x0, mid + 0.05 * size], [x0 + sign * 0.12, mid], rule),
        ([x0 + sign * 0.12, mid], [x1, bottom], rule * 1.6),
        ([x1, bottom], [x2, top + rule / 2.0], rule),
    ] {
        result.items.push(MathItem::Stroke { from, to, width });
    }
    let body_width = body.width;
    result.place(body, [x2 + 0.05 * size, 0.0]);
    result.items.push(MathItem::Rule([
        x2,
        top,
        x2 + 0.05 * size + body_width + 0.05 * size,
        top + rule,
    ]));
    result.width = x2 + 0.1 * size + body_width;
    result.ascent = result.ascent.max(-top);
    result.descent = result.descent.max(bottom);
    if let Some(degree) = degree {
        let y = mid - 0.1 * size - degree.descent;
        result.place(degree, [0.0, y]);
    }
    result
}

/// Cells in rows of `width` columns, centered in each column (left-aligned for equation
/// arrays), the whole grid centered on the math axis.
fn grid(cells: Vec<Bx>, width: usize, centered: bool, size: f32) -> Bx {
    let (column_gap, row_gap) = (0.8 * size, 0.2 * size);
    let mut columns = vec![0.0_f32; width];
    for (index, cell) in cells.iter().enumerate() {
        columns[index % width] = columns[index % width].max(cell.width);
    }
    let rows: Vec<(f32, f32)> = cells
        .chunks(width)
        .map(|row| {
            row.iter().fold((0.0_f32, 0.0_f32), |(a, d), cell| {
                (a.max(cell.ascent), d.max(cell.descent))
            })
        })
        .collect();
    let height: f32 = rows.iter().map(|(a, d)| a + d).sum::<f32>()
        + row_gap * rows.len().saturating_sub(1) as f32;
    let mut y = -AXIS * size - height / 2.0;
    let mut result = Bx::empty();
    let mut cells = cells.into_iter();
    for (ascent, descent) in rows {
        let mut x = 0.0;
        for column in &columns {
            let Some(cell) = cells.next() else { break };
            let dx = if centered {
                (column - cell.width) / 2.0
            } else {
                0.0
            };
            result.place(cell, [x + dx, y + ascent]);
            x += column + column_gap;
        }
        y += ascent + descent + row_gap;
    }
    result.width = columns.iter().sum::<f32>() + column_gap * width.saturating_sub(1) as f32;
    result
}

/// Extra space each side of an operator, in em: medium for binary operators, thick for
/// relations and arrows.
fn spacing(c: char) -> f32 {
    match c {
        '+' | '-' | '−' | '±' | '∓' | '×' | '÷' | '·' | '∙' | '*' => 4.0 / 18.0,
        '=' | '<' | '>' | '≤' | '≥' | '≠' | '≈' | '≡' | '→' | '←' | '↔' | '⇒' | '∈' | '∉' => {
            5.0 / 18.0
        }
        _ => 0.0,
    }
}

/// The spacing form of a combining accent, drawn above its base.
fn spacing_accent(c: char) -> char {
    match c {
        '\u{302}' => 'ˆ',
        '\u{303}' => '˜',
        '\u{307}' => '˙',
        '\u{308}' => '¨',
        '\u{301}' => '´',
        '\u{300}' => '`',
        '\u{306}' => '˘',
        '\u{30c}' => 'ˇ',
        '\u{20d7}' => '→',
        c => c,
    }
}

/// Latin letters draw in mathematical italic, as OneNote's equation editor stores them.
fn italic(c: char) -> char {
    match c {
        'h' => '\u{210e}',
        'a'..='z' => char::from_u32(0x1d44e + (c as u32 - 'a' as u32)).unwrap(),
        'A'..='Z' => char::from_u32(0x1d434 + (c as u32 - 'A' as u32)).unwrap(),
        c => c,
    }
}

/// Height above and depth below the baseline of the glyphs' outlines; the depth is negative
/// for ink that floats above the baseline, as an accent's does.
fn ink(layout: &TextLayout) -> [f32; 2] {
    use skrifa::{MetadataProvider, instance::Size};
    let mut extent = [f32::NEG_INFINITY; 2];
    for line in layout.shaped.lines() {
        for item in line.items() {
            let parley::PositionedLayoutItem::GlyphRun(glyphs) = item else {
                continue;
            };
            let run = glyphs.run();
            let font = &run.font().font;
            let Ok(font) = skrifa::FontRef::from_index(font.data.as_ref(), font.index) else {
                continue;
            };
            let metrics = font.glyph_metrics(
                Size::new(run.font_size()),
                skrifa::instance::LocationRef::default(),
            );
            for glyph in glyphs.glyphs() {
                match metrics.bounds(skrifa::GlyphId::new(glyph.id)) {
                    // Blank glyphs, such as spaces, have no ink.
                    Some(bounds) if bounds.y_max > bounds.y_min => {
                        extent[0] = extent[0].max(bounds.y_max - glyph.y);
                        extent[1] = extent[1].max(glyph.y - bounds.y_min);
                    }
                    _ => {}
                }
            }
        }
    }
    extent.map(|v| if v.is_finite() { v } else { 0.0 })
}

/// Lays out an equation paragraph at the font size and colour of its first run.
pub fn layout(engine: &mut TextEngine, paragraph: &Paragraph) -> Result<MathLayout, LayoutError> {
    let nodes = Math::parse(paragraph).map_err(|_| LayoutError::UnsupportedContent)?;
    let format = &paragraph.spans()[0].format;
    let size = format.font_size.unwrap_or(11.0);
    let color = format.color.filter(|color| *color != 0xff00_0000);
    // An equation line is at least as tall as a line of text at its size.
    let strut = engine.layout(
        &Paragraph::new(
            " ".into(),
            Format {
                font_size: Some(size),
                ..Format::default()
            },
        ),
        100.0,
    )?;
    let (_, line) = strut
        .lines()
        .next()
        .ok_or(LayoutError::InvalidFontMetrics)?;
    let mut body = Context { engine, color }.row(&nodes, size)?;
    body.ascent = body.ascent.max(line.baseline);
    body.descent = body.descent.max(line.height - line.baseline);
    Ok(MathLayout {
        size: [body.width, body.height()],
        color: color.unwrap_or(0),
        baseline: body.ascent,
        items: body
            .items
            .into_iter()
            .map(|item| offset(item, [0.0, body.ascent]))
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn object(kind: u32, symbols: &[char], arguments: Vec<Vec<Math>>) -> Math {
        Math::Object {
            kind,
            symbols: symbols.to_vec(),
            columns: None,
            arguments,
        }
    }

    /// Each text's left edge and baseline.
    fn texts(layout: &MathLayout) -> Vec<[f32; 2]> {
        layout
            .items
            .iter()
            .filter_map(|item| match item {
                MathItem::Text { origin, layout } => Some([
                    origin[0],
                    origin[1] + layout.lines().next().unwrap().1.baseline,
                ]),
                _ => None,
            })
            .collect()
    }

    fn laid_out(nodes: &[Math]) -> MathLayout {
        let mut engine = TextEngine::default();
        layout(&mut engine, &Math::paragraph(nodes, &Format::default())).unwrap()
    }

    #[test]
    fn scripts_fractions_and_accents_stack_as_equations_do() {
        let x = || vec![Math::Identifier('x')];
        let scripted = laid_out(&[object(
            30,
            &[],
            vec![
                x(),
                vec![Math::Identifier('i')],
                vec![Math::Number("2".into())],
            ],
        )]);
        let [base, upper, lower] = texts(&scripted)[..] else {
            panic!()
        };
        assert!(lower[0] > base[0] && upper[0] == lower[0]);
        assert!(upper[1] < base[1] && lower[1] > base[1]);

        let fraction = laid_out(&[object(16, &[], vec![x(), vec![Math::Identifier('y')]])]);
        let [numerator, denominator] = texts(&fraction)[..] else {
            panic!()
        };
        let bar = fraction.items.iter().find_map(|item| match item {
            MathItem::Rule(rect) => Some(*rect),
            _ => None,
        });
        let [_, top, _, bottom] = bar.unwrap();
        assert!(numerator[1] < top && denominator[1] > numerator[1] && bottom > top);
        assert!(fraction.size[1] > laid_out(&[Math::Identifier('x')]).size[1]);

        let accented = laid_out(&[object(10, &['\u{302}'], vec![x()])]);
        let [base, accent] = texts(&accented)[..] else {
            panic!()
        };
        assert!(accent[1] < base[1]);

        let radical = laid_out(&[object(25, &[], vec![vec![], x()])]);
        assert!(
            radical
                .items
                .iter()
                .any(|item| matches!(item, MathItem::Stroke { .. }))
        );
        assert!(
            radical
                .items
                .iter()
                .any(|item| matches!(item, MathItem::Rule(_)))
        );
    }

    #[test]
    fn a_plain_equation_keeps_the_height_of_a_line_of_text() {
        let mut engine = TextEngine::default();
        let line = engine
            .layout(&Paragraph::new("x".into(), Format::default()), 100.0)
            .unwrap();
        let equation = laid_out(&[
            Math::Identifier('x'),
            Math::Operator('+'),
            Math::Number("1".into()),
        ]);
        assert!((equation.size[1] - line.height()).abs() < 0.01);
        assert!(equation.size[0] > 0.0);
    }
}
