//! Equations as OneNote stores them: a linear text where U+FDD0 opens an inline object,
//! U+FDEE separates its arguments and U+FDEF closes it, with the object's kind on the run
//! data of the opening character. `Math::parse` builds the tree and `mathml` renders it the
//! way OneNote's own export does for the kinds a native fixture has verified (superscript
//! and fraction); other kinds follow the text object model's meaning without a native check.

use super::text::Paragraph;
use crate::document::MathObject;

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Math {
    Identifier(char),
    Number(String),
    Operator(char),
    Object {
        kind: u32,
        symbols: Vec<char>,
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
            arguments: Vec<Vec<Math>>,
        }
        let mut frames: Vec<Frame> = Vec::new();
        let mut sequences: Vec<Vec<Math>> = vec![Vec::new()];
        let mut number = String::new();
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
            match character {
                OBJECT_START => {
                    // The span lookup treats a boundary offset as the span ending there, so the
                    // opening character's own span is the one containing the offset after it.
                    let object = object_at(here + 1)
                        .ok_or_else(|| invalid("An equation object has no run data"))?;
                    frames.push(Frame {
                        kind: object.kind,
                        symbols: object.symbols.clone(),
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
                        arguments: frame.arguments,
                    });
                }
                c if c.is_whitespace() => {}
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
            Math::Number(n) => tag(out, "mn", n),
            Math::Operator(c) => tag(out, "mo", &c.to_string()),
            Math::Object {
                kind,
                symbols,
                arguments,
            } => {
                let argument = |out: &mut String, index: usize| match arguments
                    .get(index)
                    .map(Vec::as_slice)
                {
                    Some([single]) => single.write(out),
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
                    (31, 2) => wrapped(out, "msup", &[0, 1]),
                    (29, 2) => wrapped(out, "msub", &[0, 1]),
                    (30, 3) => wrapped(out, "msubsup", &[0, 1, 2]),
                    (16, 2) | (26, 2) => wrapped(out, "mfrac", &[0, 1]),
                    (25, 2) if !arguments[0].is_empty() => wrapped(out, "mroot", &[1, 0]),
                    (25, _) => wrapped(out, "msqrt", &[arguments.len() - 1]),
                    (19, 2) => wrapped(out, "munder", &[0, 1]),
                    (33, 2) => wrapped(out, "mover", &[0, 1]),
                    (13, _) => {
                        out.push_str("<mml:mrow>");
                        if let Some(open) = symbols.first() {
                            tag(out, "mo", &open.to_string());
                        }
                        for index in 0..arguments.len() {
                            argument(out, index);
                        }
                        if let Some(close) = symbols.get(1) {
                            tag(out, "mo", &close.to_string());
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

fn tag(out: &mut String, element: &str, content: &str) {
    out.push_str(&format!("<mml:{element}>"));
    for c in content.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '&' => out.push_str("&amp;"),
            '>' => out.push_str("&gt;"),
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
