//! Equations as OneNote stores them: a linear text where U+FDD0 opens an inline object,
//! U+FDEE separates its arguments and U+FDEF closes it, with the object's kind on the run
//! data of the opening character. `Math::parse` builds the tree and `mathml` renders it the
//! way OneNote's own export does, verified against native fixtures for every kind the
//! equation editor produced: scripts, fractions, fences, n-ary operators with any limits,
//! radicals, limit objects, accents, overbars, boxes, matrices and equation arrays.

use super::text::Paragraph;
use crate::document::MathObject;

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Math {
    Identifier(char),
    /// Upright letters, as the editor stores a function name such as `lim`.
    Function(String),
    Number(String),
    Operator(char),
    Object {
        kind: u32,
        symbols: Vec<char>,
        /// Column count of a matrix (arguments are its cells, row by row) or an equation
        /// array (arguments are its rows).
        columns: Option<u8>,
        arguments: Vec<Vec<Math>>,
    },
}

const OBJECT_START: char = '\u{fdd0}';
const ARGUMENT_SEPARATOR: char = '\u{fdee}';
const OBJECT_END: char = '\u{fdef}';

impl Math {
    /// Whether a paragraph's text is stored as an equation.
    pub fn is_equation(paragraph: &Paragraph) -> bool {
        paragraph.text().contains(OBJECT_START)
            || paragraph
                .spans()
                .iter()
                .any(|span| span.format.math == Some(true))
    }

    /// The equation a paragraph's text holds, as a sequence of nodes.
    pub fn parse(paragraph: &Paragraph) -> Result<Vec<Math>, crate::Error> {
        let invalid = |message| crate::Error { offset: 0, message };
        let object_at = |offset: usize| -> Option<&MathObject> {
            paragraph
                .format_at(u32::try_from(offset).ok()?)
                .ok()
                .and_then(|f| f.math_object.as_ref())
        };
        struct Frame {
            kind: u32,
            symbols: Vec<char>,
            columns: Option<u8>,
            arguments: Vec<Vec<Math>>,
        }
        let mut frames: Vec<Frame> = Vec::new();
        let mut sequences: Vec<Vec<Math>> = vec![Vec::new()];
        let mut number = String::new();
        let mut word = String::new();
        let mut offset = 0;
        for character in paragraph.text().chars() {
            let here = offset;
            offset += character.len_utf16();
            if character.is_ascii_digit() {
                number.push(character);
                continue;
            }
            if !number.is_empty() {
                let number = std::mem::take(&mut number);
                sequences.last_mut().unwrap().push(Math::Number(number));
            }
            if character.is_ascii_alphabetic() {
                word.push(character);
                continue;
            }
            if !word.is_empty() {
                let word = std::mem::take(&mut word);
                sequences.last_mut().unwrap().push(Math::Function(word));
            }
            match character {
                OBJECT_START => {
                    // The span lookup treats a boundary offset as the span ending there, so the
                    // opening character's own span is the one containing the offset after it.
                    let object = object_at(here + 1)
                        .ok_or_else(|| invalid("An equation object has no run data"))?;
                    frames.push(Frame {
                        kind: object.kind,
                        symbols: object.symbols.clone(),
                        columns: object.columns,
                        arguments: Vec::new(),
                    });
                    sequences.push(Vec::new());
                }
                ARGUMENT_SEPARATOR => {
                    let argument = sequences.pop().unwrap();
                    frames
                        .last_mut()
                        .ok_or_else(|| invalid("An equation argument separator has no object"))?
                        .arguments
                        .push(argument);
                    sequences.push(Vec::new());
                }
                OBJECT_END => {
                    let argument = sequences.pop().unwrap();
                    let mut frame = frames
                        .pop()
                        .ok_or_else(|| invalid("An equation object ends before it starts"))?;
                    frame.arguments.push(argument);
                    sequences.last_mut().unwrap().push(Math::Object {
                        kind: frame.kind,
                        symbols: frame.symbols,
                        columns: frame.columns,
                        arguments: frame.arguments,
                    });
                }
                c if c.is_whitespace() => sequences.last_mut().unwrap().push(Math::Operator(' ')),
                c if c.is_alphabetic() => sequences
                    .last_mut()
                    .unwrap()
                    .push(Math::Identifier(plain(c))),
                c => sequences.last_mut().unwrap().push(Math::Operator(c)),
            }
        }
        if !number.is_empty() {
            sequences.last_mut().unwrap().push(Math::Number(number));
        }
        if !word.is_empty() {
            sequences.last_mut().unwrap().push(Math::Function(word));
        }
        if !frames.is_empty() {
            return Err(invalid("An equation object never ends"));
        }
        Ok(sequences.pop().unwrap())
    }

    /// MathML for a node sequence, without the `math` wrapper, in OneNote's export form
    /// (`mml:` prefixed elements, one `mi` per letter).
    pub fn mathml(nodes: &[Math]) -> String {
        let mut out = String::new();
        for node in nodes {
            node.write(&mut out);
        }
        out
    }

    fn write(&self, out: &mut String) {
        match self {
            Math::Identifier(c) => tag(out, "mi", &c.to_string()),
            Math::Function(name) => tag(out, "mi", name),
            Math::Number(n) => tag(out, "mn", n),
            Math::Operator(c) => tag(out, "mo", &c.to_string()),
            Math::Object {
                kind,
                symbols,
                columns,
                arguments,
            } => {
                // A lone letter, number or operator stands bare; anything else is a row.
                let argument = |out: &mut String, index: usize| match arguments
                    .get(index)
                    .map(Vec::as_slice)
                {
                    Some([single])
                        if !matches!(single, Math::Object { .. } | Math::Function(_)) =>
                    {
                        single.write(out)
                    }
                    Some(many) => {
                        out.push_str("<mml:mrow>");
                        for node in many {
                            node.write(out);
                        }
                        out.push_str("</mml:mrow>");
                    }
                    None => out.push_str("<mml:mrow/>"),
                };
                let wrapped = |out: &mut String, element: &str, order: &[usize]| {
                    out.push_str(&format!("<mml:{element}>"));
                    for index in order {
                        argument(out, *index);
                    }
                    out.push_str(&format!("</mml:{element}>"));
                };
                match (kind, arguments.len()) {
                    (11, 1) => wrapped(out, "mpadded", &[0]),
                    (12, 1) => {
                        out.push_str("<mml:menclose notation=\"box\">");
                        argument(out, 0);
                        out.push_str("</mml:menclose>");
                    }
                    // Matrix cells row by row; an equation array's rows carry `&` alignment
                    // marks, exported as an alignment group at the row start and a mark at each.
                    (20, _) | (15, _) => {
                        let width = if *kind == 20 {
                            usize::from(columns.unwrap_or(1).max(1))
                        } else {
                            1
                        };
                        out.push_str("<mml:mtable>");
                        for (r, row) in arguments.chunks(width).enumerate() {
                            out.push_str("<mml:mtr>");
                            for (c, cell) in row.iter().enumerate() {
                                out.push_str("<mml:mtd>");
                                if *kind == 15 {
                                    out.push_str("<mml:maligngroup/>");
                                    for node in cell {
                                        match node {
                                            Math::Operator('&') => {
                                                out.push_str("<mml:malignmark/>")
                                            }
                                            node => node.write(out),
                                        }
                                    }
                                } else {
                                    argument(out, r * width + c);
                                }
                                out.push_str("</mml:mtd>");
                            }
                            out.push_str("</mml:mtr>");
                        }
                        out.push_str("</mml:mtable>");
                    }
                    (31, 2) => wrapped(out, "msup", &[0, 1]),
                    (29, 2) => wrapped(out, "msub", &[0, 1]),
                    (30, 3) => wrapped(out, "msubsup", &[0, 1, 2]),
                    (16, 2) | (26, 2) => wrapped(out, "mfrac", &[0, 1]),
                    (25, 2) if !arguments[0].is_empty() => wrapped(out, "mroot", &[1, 0]),
                    (25, _) => wrapped(out, "msqrt", &[arguments.len() - 1]),
                    (19, 2) => wrapped(out, "munder", &[0, 1]),
                    (33, 2) => wrapped(out, "mover", &[0, 1]),
                    // Parentheses are the default fences; other pairs name themselves.
                    (13, _) => {
                        let (open, close) = (symbols.first(), symbols.get(1));
                        if open == Some(&'(') && close == Some(&')') {
                            wrapped(out, "mfenced", &[0]);
                        } else {
                            out.push_str("<mml:mfenced");
                            if let Some(open) = open {
                                out.push_str(&format!(" open=\"{}\"", escaped(*open)));
                            }
                            if let Some(close) = close {
                                out.push_str(&format!(" close=\"{}\"", escaped(*close)));
                            }
                            out.push('>');
                            argument(out, 0);
                            out.push_str("</mml:mfenced>");
                        }
                    }
                    (10, 1) | (23, 1) => {
                        let accent = *kind == 10;
                        out.push_str(&format!("<mml:mover accent=\"{accent}\">"));
                        argument(out, 0);
                        if accent {
                            tag(
                                out,
                                "mo",
                                &spacing(symbols.first().copied().unwrap_or('^')).to_string(),
                            );
                        } else {
                            out.push_str("<mml:mo stretchy=\"true\">");
                            out.push(symbols.first().copied().unwrap_or('¯'));
                            out.push_str("</mml:mo>");
                        }
                        out.push_str("</mml:mover>");
                    }
                    // Lower limit, upper limit, body; integrals take their limits as scripts,
                    // other operators above and below, and an empty limit leaves its side out.
                    (21, 3) => {
                        let operator = symbols.first().copied().unwrap_or('∑');
                        let scripts = ('\u{222b}'..='\u{2233}').contains(&operator);
                        let (lower, upper) = (!arguments[0].is_empty(), !arguments[1].is_empty());
                        let element = match (lower, upper, scripts) {
                            (true, true, true) => "msubsup",
                            (true, true, false) => "munderover",
                            (true, false, true) => "msub",
                            (true, false, false) => "munder",
                            (false, true, true) => "msup",
                            (false, true, false) => "mover",
                            (false, false, _) => "mrow",
                        };
                        out.push_str(&format!(
                            "<mml:{element}><mml:mo stretchy=\"false\">{operator}</mml:mo>"
                        ));
                        if lower {
                            argument(out, 0);
                        }
                        if upper {
                            argument(out, 1);
                        }
                        out.push_str(&format!("</mml:{element}>"));
                        out.push_str("<mml:mrow>");
                        for node in &arguments[2] {
                            node.write(out);
                        }
                        out.push_str("</mml:mrow>");
                    }
                    _ => {
                        out.push_str("<mml:mrow>");
                        for symbol in symbols {
                            tag(out, "mo", &symbol.to_string());
                        }
                        for index in 0..arguments.len() {
                            argument(out, index);
                        }
                        out.push_str("</mml:mrow>");
                    }
                }
            }
        }
    }
}

fn escaped(c: char) -> String {
    match c {
        '<' => "&lt;".into(),
        '&' => "&amp;".into(),
        '"' => "&quot;".into(),
        c => c.to_string(),
    }
}

/// The spacing character OneNote exports for a combining accent.
fn spacing(c: char) -> char {
    match c {
        '\u{302}' => '^',
        '\u{303}' => '~',
        '\u{308}' => '¨',
        c => c,
    }
}

fn tag(out: &mut String, element: &str, content: &str) {
    out.push_str(&format!("<mml:{element}>"));
    for c in content.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '&' => out.push_str("&amp;"),
            '>' => out.push_str("&gt;"),
            ' ' => out.push_str("&nbsp;"),
            c => out.push(c),
        }
    }
    out.push_str(&format!("</mml:{element}>"));
}

/// OneNote exports mathematical italic Latin letters as their plain letters and leaves every
/// other alphabet (Greek, double-struck, …) as stored.
fn plain(c: char) -> char {
    match u32::from(c) {
        code @ 0x1d434..=0x1d44d => char::from_u32(code - 0x1d434 + u32::from('A')).unwrap(),
        code @ 0x1d44e..=0x1d467 => char::from_u32(code - 0x1d44e + u32::from('a')).unwrap(),
        0x210e => 'h',
        _ => c,
    }
}

impl Math {
    /// The paragraph OneNote stores for `nodes`: linear text with object controls, every run
    /// formatted the way the equation editor formats it (Cambria Math, italic, the math flags
    /// and the math language over `base`), and the run data each object carries.
    pub fn paragraph(nodes: &[Math], base: &crate::document::Format) -> Paragraph {
        let mut style = base.clone();
        style.italic = Some(true);
        style.hidden = Some(false);
        style.hyperlink = Some(false);
        style.math = Some(true);
        style.embedded_object = Some(true);
        style.font = Some("Cambria Math".into());
        style.language = Some(0x1007f);
        let mut runs: Vec<(String, Option<MathObject>)> = Vec::new();
        write_sequence(nodes, &mut runs);
        Paragraph::from_runs(runs.into_iter().map(|(text, object)| {
            let mut format = style.clone();
            format.math_object = Some(object.unwrap_or(MathObject {
                kind: PLAIN_RUN,
                arguments: None,
                columns: None,
                symbols: Vec::new(),
            }));
            (text, format)
        }))
    }
}

/// The object kind OneNote gives runs between objects.
const PLAIN_RUN: u32 = 0x9000_0000;

fn write_sequence(nodes: &[Math], runs: &mut Vec<(String, Option<MathObject>)>) {
    let mut leaf = String::new();
    for node in nodes {
        match node {
            Math::Identifier(c) => leaf.push(italic(*c)),
            Math::Function(name) => leaf.push_str(name),
            Math::Number(n) => leaf.push_str(n),
            Math::Operator(c) => leaf.push(*c),
            Math::Object {
                kind,
                symbols,
                columns,
                arguments,
            } => {
                if !leaf.is_empty() {
                    runs.push((std::mem::take(&mut leaf), None));
                }
                runs.push((
                    OBJECT_START.to_string(),
                    Some(MathObject {
                        kind: *kind,
                        arguments: Some(arguments.len() as u32),
                        columns: *columns,
                        symbols: symbols.clone(),
                    }),
                ));
                for (index, argument) in arguments.iter().enumerate() {
                    let before = runs.len();
                    write_sequence(argument, runs);
                    let control = if index + 1 == arguments.len() {
                        OBJECT_END
                    } else {
                        ARGUMENT_SEPARATOR
                    };
                    let object = MathObject {
                        kind: *kind,
                        arguments: (index > 0).then_some(index as u32),
                        columns: None,
                        symbols: Vec::new(),
                    };
                    let extended = runs.len() > before;
                    match runs.last_mut() {
                        Some((text, slot)) if extended && slot.is_none() => {
                            text.push(control);
                            *slot = Some(object);
                        }
                        _ => runs.push((control.to_string(), Some(object))),
                    }
                }
            }
        }
    }
    if !leaf.is_empty() {
        runs.push((leaf, None));
    }
}

/// The equation editor stores Latin letters as mathematical italics.
fn italic(c: char) -> char {
    match c {
        'h' => '\u{210e}',
        'A'..='Z' => char::from_u32(0x1d434 + u32::from(c) - u32::from('A')).unwrap(),
        'a'..='z' => char::from_u32(0x1d44e + u32::from(c) - u32::from('a')).unwrap(),
        c => c,
    }
}
