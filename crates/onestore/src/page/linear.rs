//! Equations in the linear format of OneNote's equation editor (UnicodeMath): what Linear
//! shows and what typing builds up. `Math::from_linear` reads the typed or shown text into the
//! tree `Math::paragraph` stores; `Math::linear` writes a tree the way Linear shows it. Both
//! follow the equation editor as observed (`corpus/math-edit/native-linear`): a space that
//! ends a built object is consumed, other spaces stay, and an n-ary operator takes the rest
//! of its row as its body.

use super::Math;
use super::math::{italic, plain};
use super::text::Paragraph;
use crate::document::Format;

/// N-ary operators, which take limits and a body when a script follows them.
const NARY: &[char] = &[
    '∑', '∏', '∐', '∫', '∬', '∭', '∮', '∯', '∰', '⋀', '⋁', '⋂', '⋃', '⨀', '⨁', '⨂', '⨄', '⨅', '⨆',
];

/// Function names the equation editor sets upright; those that take a limit below take it
/// from a subscript.
const FUNCTIONS: &[&str] = &[
    "lim", "max", "min", "sup", "inf", "det", "gcd", "sin", "cos", "tan", "sec", "csc", "cot",
    "sinh", "cosh", "tanh", "log", "ln", "exp", "arg",
];
const LIMITS: &[&str] = &["lim", "max", "min", "sup", "inf", "det", "gcd"];

/// The equation editor's control words: `\name` and the space after it become the symbol.
const CONTROL_WORDS: &[(&str, char)] = &[
    ("alpha", 'α'),
    ("beta", 'β'),
    ("gamma", 'γ'),
    ("delta", 'δ'),
    ("epsilon", 'ϵ'),
    ("varepsilon", 'ε'),
    ("zeta", 'ζ'),
    ("eta", 'η'),
    ("theta", 'θ'),
    ("vartheta", 'ϑ'),
    ("iota", 'ι'),
    ("kappa", 'κ'),
    ("lambda", 'λ'),
    ("mu", 'μ'),
    ("nu", 'ν'),
    ("xi", 'ξ'),
    ("pi", 'π'),
    ("rho", 'ρ'),
    ("sigma", 'σ'),
    ("tau", 'τ'),
    ("upsilon", 'υ'),
    ("phi", 'ϕ'),
    ("varphi", 'φ'),
    ("chi", 'χ'),
    ("psi", 'ψ'),
    ("omega", 'ω'),
    ("Gamma", 'Γ'),
    ("Delta", 'Δ'),
    ("Theta", 'Θ'),
    ("Lambda", 'Λ'),
    ("Xi", 'Ξ'),
    ("Pi", 'Π'),
    ("Sigma", 'Σ'),
    ("Upsilon", 'Υ'),
    ("Phi", 'Φ'),
    ("Psi", 'Ψ'),
    ("Omega", 'Ω'),
    ("to", '→'),
    ("rightarrow", '→'),
    ("leftarrow", '←'),
    ("times", '×'),
    ("div", '÷'),
    ("pm", '±'),
    ("mp", '∓'),
    ("infty", '∞'),
    ("le", '≤'),
    ("ge", '≥'),
    ("ne", '≠'),
    ("approx", '≈'),
    ("equiv", '≡'),
    ("sim", '∼'),
    ("propto", '∝'),
    ("cdot", '⋅'),
    ("partial", '∂'),
    ("nabla", '∇'),
    ("in", '∈'),
    ("notin", '∉'),
    ("subset", '⊂'),
    ("supset", '⊃'),
    ("cup", '∪'),
    ("cap", '∩'),
    ("forall", '∀'),
    ("exists", '∃'),
    ("ll", '≪'),
    ("gg", '≫'),
    ("ldots", '…'),
    ("cdots", '⋯'),
    ("sum", '∑'),
    ("prod", '∏'),
    ("coprod", '∐'),
    ("int", '∫'),
    ("iint", '∬'),
    ("iiint", '∭'),
    ("oint", '∮'),
    ("bigcap", '⋂'),
    ("bigcup", '⋃'),
    ("sqrt", '√'),
    ("cbrt", '∛'),
    ("qdrt", '∜'),
    ("matrix", '■'),
    ("eqarray", '█'),
    ("box", '□'),
    ("rect", '▭'),
    ("overline", '¯'),
    ("above", '┴'),
    ("below", '┬'),
    ("naryand", '▒'),
    ("hat", '\u{302}'),
    ("tilde", '\u{303}'),
    ("bar", '\u{305}'),
    ("dot", '\u{307}'),
    ("ddot", '\u{308}'),
    ("check", '\u{30c}'),
    ("vec", '\u{20d7}'),
];

fn object(kind: u32, symbols: Vec<char>, columns: Option<u8>, arguments: Vec<Vec<Math>>) -> Math {
    Math::Object {
        kind,
        symbols,
        columns,
        arguments,
    }
}

fn accent(c: char) -> bool {
    matches!(u32::from(c), 0x300..=0x36f | 0x20d0..=0x20ff)
}

fn closing(open: char) -> Option<char> {
    Some(match open {
        '(' => ')',
        '[' => ']',
        '{' => '}',
        '⟨' => '⟩',
        '〖' => '〗',
        _ => return None,
    })
}

struct Parser {
    chars: Vec<char>,
    at: usize,
    /// Text still being typed: a construct whose last operand is missing stays as typed.
    typing: bool,
}

impl Parser {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.at).copied()
    }

    /// A space ending a built object triggered the build and is not kept.
    fn built(&mut self, out: &mut Vec<Math>, node: Math) {
        out.push(node);
        if self.peek() == Some(' ') {
            self.at += 1;
        }
    }

    fn sequence(&mut self, stops: &[char]) -> Vec<Math> {
        let mut out = Vec::new();
        while let Some(c) = self.peek() {
            if stops.contains(&c) {
                break;
            }
            self.element(&mut out, stops);
        }
        out
    }

    fn element(&mut self, out: &mut Vec<Math>, stops: &[char]) {
        let c = self.peek().unwrap();
        let next = self.chars.get(self.at + 1).copied();
        match c {
            '/' => {
                self.at += 1;
                let denominator = stripped(self.run(stops));
                if self.typing && denominator.is_empty() {
                    out.push(Math::Operator(c));
                    return;
                }
                let start = out
                    .iter()
                    .rposition(|node| matches!(node, Math::Operator(_)))
                    .map_or(0, |at| at + 1);
                let numerator = stripped(out.split_off(start));
                self.built(
                    out,
                    object(16, vec!['/'], None, vec![numerator, denominator]),
                );
            }
            '^' | '_' => self.script(out),
            '┴' | '┬' => {
                self.at += 1;
                let argument = self.operand();
                if self.typing && argument.is_empty() {
                    out.push(Math::Operator(c));
                    return;
                }
                let base = out.pop().into_iter().collect();
                let kind = if c == '┴' { 33 } else { 19 };
                self.built(out, object(kind, vec![c], None, vec![base, argument]));
            }
            '\u{a0}' if next.is_some_and(accent) => self.at += 1,
            c if accent(c) => {
                self.at += 1;
                if matches!(out.last(), Some(Math::Operator(' ' | '\u{a0}'))) {
                    out.pop();
                }
                let base = stripped(out.pop().into_iter().collect());
                self.built(out, object(10, vec![c], None, vec![base]));
            }
            c if NARY.contains(&c) && matches!(next, Some('_' | '^' | '▒')) => {
                self.at += 1;
                let limits = self.at;
                let (mut lower, mut upper) = (None, None);
                loop {
                    match self.peek() {
                        Some('_') if lower.is_none() => {
                            self.at += 1;
                            lower = Some(self.operand());
                        }
                        Some('^') if upper.is_none() => {
                            self.at += 1;
                            upper = Some(self.operand());
                        }
                        _ => break,
                    }
                }
                if self.typing && [&lower, &upper].contains(&&Some(Vec::new())) {
                    self.at = limits;
                    out.push(Math::Operator(c));
                    return;
                }
                let body = match self.peek() {
                    Some('▒') => {
                        self.at += 1;
                        self.operand()
                    }
                    Some(' ') => {
                        self.at += 1;
                        self.sequence(stops)
                    }
                    _ => self.sequence(stops),
                };
                out.push(object(
                    21,
                    vec![c],
                    None,
                    vec![lower.unwrap_or_default(), upper.unwrap_or_default(), body],
                ));
            }
            '〖' => {
                self.at += 1;
                let inner = self.sequence(&['〗']);
                if self.peek() == Some('〗') {
                    self.at += 1;
                }
                out.extend(inner);
            }
            _ => {
                let node = self.factor();
                if matches!(node, Math::Object { .. }) {
                    self.built(out, node);
                } else {
                    out.push(node);
                }
            }
        }
    }

    fn script(&mut self, out: &mut Vec<Math>) {
        let first = self.peek().unwrap();
        self.at += 1;
        let script = self.operand();
        if self.typing && script.is_empty() {
            out.push(Math::Operator(first));
            return;
        }
        let base: Vec<Math> = out.pop().into_iter().collect();
        let other = if first == '_' { '^' } else { '_' };
        let node = if self.peek() == Some(other) {
            self.at += 1;
            let second = self.operand();
            let (lower, upper) = if first == '_' {
                (script, second)
            } else {
                (second, script)
            };
            object(30, vec!['_'], None, vec![base, lower, upper])
        } else if first == '_'
            && matches!(&base[..], [Math::Function(name)] if LIMITS.contains(&name.as_str()))
        {
            object(19, vec!['_'], None, vec![base, script])
        } else {
            let kind = if first == '^' { 31 } else { 29 };
            object(kind, vec![first], None, vec![base, script])
        };
        self.built(out, node);
    }

    /// One factor as an object's argument: parentheses and the invisible grouping brackets
    /// only group it.
    fn operand(&mut self) -> Vec<Math> {
        match self.peek() {
            Some(open @ ('(' | '〖')) => {
                let start = self.at;
                self.at += 1;
                let close = closing(open).unwrap();
                let inner = self.sequence(&[close]);
                if self.peek() == Some(close) {
                    self.at += 1;
                } else if self.typing {
                    // A group still open is still being typed.
                    self.at = start;
                    return Vec::new();
                }
                inner
            }
            // An argument not typed yet stays empty for the caret.
            Some(' ') | None => Vec::new(),
            Some(_) => vec![self.factor()],
        }
    }

    /// Juxtaposed factors with their scripts, as a fraction's denominator.
    fn run(&mut self, stops: &[char]) -> Vec<Math> {
        let mut out = Vec::new();
        while let Some(c) = self.peek() {
            if stops.contains(&c) {
                break;
            }
            let starts = c.is_alphanumeric()
                || closing(c).is_some()
                || matches!(c, '√' | '∛' | '∜' | '■' | '█' | '□' | '▭' | '¯');
            if matches!(c, '^' | '_') && !out.is_empty() {
                self.script(&mut out);
            } else if starts {
                out.push(self.factor());
            } else {
                break;
            }
        }
        out
    }

    fn factor(&mut self) -> Math {
        let c = self.peek().unwrap();
        self.at += 1;
        if c.is_ascii_digit() {
            let mut number = c.to_string();
            while let Some(digit) = self.peek().filter(char::is_ascii_digit) {
                number.push(digit);
                self.at += 1;
            }
            return Math::Number(number);
        }
        if c.is_ascii_alphabetic() {
            let word: String = self.chars[self.at - 1..]
                .iter()
                .take_while(|c| c.is_ascii_alphabetic())
                .collect();
            if FUNCTIONS.contains(&word.as_str()) {
                self.at += word.len() - 1;
                return Math::Function(word);
            }
            return Math::Identifier(c);
        }
        if c.is_alphabetic() {
            return Math::Identifier(plain(c));
        }
        if let Some(close) = closing(c) {
            let start = self.at;
            let inner = self.sequence(&[close]);
            if self.peek() == Some(close) {
                self.at += 1;
                return object(13, vec![c, close], None, vec![inner]);
            }
            self.at = start;
            return Math::Operator(c);
        }
        match c {
            '√' | '∛' | '∜' => {
                let degree = match c {
                    '∛' => vec![Math::Number("3".into())],
                    '∜' => vec![Math::Number("4".into())],
                    _ => Vec::new(),
                };
                let (degree, radicand) = if c == '√' && self.peek() == Some('(') {
                    self.at += 1;
                    let first = self.sequence(&[')', '&']);
                    let parts = if self.peek() == Some('&') {
                        self.at += 1;
                        (first, self.sequence(&[')']))
                    } else {
                        (degree, first)
                    };
                    if self.peek() == Some(')') {
                        self.at += 1;
                    }
                    parts
                } else {
                    (degree, self.operand())
                };
                if self.typing && radicand.is_empty() {
                    return Math::Operator(c);
                }
                object(25, vec!['√'], None, vec![degree, radicand])
            }
            '■' | '█' if self.peek() == Some('(') => {
                self.at += 1;
                let matrix = c == '■';
                let mut rows = vec![Vec::new()];
                loop {
                    let stops: &[char] = if matrix {
                        &[')', '&', '@']
                    } else {
                        &[')', '@']
                    };
                    let cell = self.sequence(stops);
                    rows.last_mut().unwrap().push(cell);
                    match self.peek() {
                        Some('&') => self.at += 1,
                        Some('@') => {
                            self.at += 1;
                            rows.push(Vec::new());
                        }
                        Some(_) => {
                            self.at += 1;
                            break;
                        }
                        None => break,
                    }
                }
                if matrix {
                    let columns = rows.iter().map(Vec::len).max().unwrap_or(1);
                    let arguments = rows
                        .into_iter()
                        .flat_map(|mut row| {
                            row.resize(columns, Vec::new());
                            row
                        })
                        .collect();
                    object(20, vec!['■'], u8::try_from(columns).ok(), arguments)
                } else {
                    let arguments = rows.into_iter().flatten().collect();
                    object(15, vec!['█'], Some(1), arguments)
                }
            }
            '□' | '▭' | '¯' => {
                let kind = match c {
                    '□' => 11,
                    '▭' => 12,
                    _ => 23,
                };
                let inner = self.operand();
                if self.typing && inner.is_empty() {
                    return Math::Operator(c);
                }
                object(kind, vec![c], None, vec![inner])
            }
            c => Math::Operator(c),
        }
    }
}

/// A lone parenthesized group as an argument only groups it.
fn stripped(nodes: Vec<Math>) -> Vec<Math> {
    match <[Math; 1]>::try_from(nodes) {
        Ok(
            [
                Math::Object {
                    kind: 13,
                    symbols,
                    mut arguments,
                    ..
                },
            ],
        ) if symbols == ['(', ')'] && arguments.len() == 1 => arguments.pop().unwrap(),
        Ok([node]) => vec![node],
        Err(nodes) => nodes,
    }
}

/// Replaces control words with their symbols; an unknown one stays as typed.
fn expand(text: &str) -> Vec<char> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::with_capacity(chars.len());
    let mut at = 0;
    while at < chars.len() {
        if chars[at] == '\\' {
            let name: String = chars[at + 1..]
                .iter()
                .take_while(|c| c.is_ascii_alphabetic())
                .collect();
            if let Some((_, symbol)) = CONTROL_WORDS.iter().find(|(word, _)| *word == name) {
                out.push(*symbol);
                at += 1 + name.len();
                if chars.get(at) == Some(&' ') {
                    at += 1;
                }
                continue;
            }
        }
        out.push(chars[at]);
        at += 1;
    }
    out
}

impl Math {
    /// The equation typed or shown as `text` in the linear format, built up.
    pub fn from_linear(text: &str) -> Vec<Math> {
        Parser {
            chars: expand(text),
            at: 0,
            typing: false,
        }
        .sequence(&[])
    }

    /// What the equation editor builds as `text` is typed: as [`Math::from_linear`], but a
    /// construct still missing its last operand stays as typed, and an n-ary operator's
    /// empty body waits for the caret.
    pub fn typed(text: &str) -> Vec<Math> {
        Parser {
            chars: expand(text),
            at: 0,
            typing: true,
        }
        .sequence(&[])
    }

    /// The paragraph OneNote stores for an equation shown in the linear format: its linear
    /// text in one run of math over `base`.
    pub fn linear_paragraph(nodes: &[Math], base: &Format) -> Option<Paragraph> {
        Some(Paragraph::new(Self::linear(nodes)?, Self::format(base)))
    }

    /// The linear format Linear shows for `nodes`, letters in mathematical italic; `None`
    /// where an object has no linear form here.
    pub fn linear(nodes: &[Math]) -> Option<String> {
        let mut out = String::new();
        write(nodes, &mut out)?;
        Some(out)
    }
}

fn write(nodes: &[Math], out: &mut String) -> Option<()> {
    let mut after_object = false;
    for node in nodes {
        match node {
            Math::Identifier(c) => out.push(italic(*c)),
            Math::Function(name) => out.push_str(name),
            Math::Number(n) => out.push_str(n),
            // Reading consumes one space after an object.
            Math::Operator(' ') if after_object => out.push_str("  "),
            Math::Operator(c) => out.push(*c),
            Math::Object {
                kind,
                symbols,
                arguments,
                columns,
            } => write_object(*kind, symbols, *columns, arguments, out)?,
        }
        after_object = matches!(node, Math::Object { .. });
    }
    Some(())
}

/// An argument that reads back as one factor stands bare; others are parenthesized.
fn argument(nodes: &[Math], out: &mut String) -> Option<()> {
    let bare = match nodes {
        [Math::Identifier(_) | Math::Number(_) | Math::Function(_)] => true,
        [Math::Object { kind, symbols, .. }] => {
            matches!(kind, 11 | 12 | 15 | 20 | 23 | 25) || *kind == 13 && symbols[..] != ['(', ')']
        }
        _ => false,
    };
    if bare {
        return write(nodes, out);
    }
    out.push('(');
    write(nodes, out)?;
    out.push(')');
    Some(())
}

/// A fraction's part stands bare when it reads back as one run of factors.
fn part(nodes: &[Math], out: &mut String) -> Option<()> {
    let bare = !nodes.is_empty()
        && !matches!(nodes, [Math::Object { kind: 13, symbols, .. }] if symbols[..] == ['(', ')'])
        && nodes.iter().all(|node| match node {
            Math::Operator(_) => false,
            Math::Object { kind, .. } => !matches!(kind, 10 | 16 | 19 | 21 | 33),
            _ => true,
        });
    if bare {
        return write(nodes, out);
    }
    out.push('(');
    write(nodes, out)?;
    out.push(')');
    Some(())
}

fn write_object(
    kind: u32,
    symbols: &[char],
    columns: Option<u8>,
    arguments: &[Vec<Math>],
    out: &mut String,
) -> Option<()> {
    match (kind, arguments) {
        (31, [base, sup]) => {
            argument(base, out)?;
            out.push('^');
            argument(sup, out)
        }
        (29, [base, sub]) => {
            argument(base, out)?;
            out.push('_');
            argument(sub, out)
        }
        (30, [base, sub, sup]) => {
            argument(base, out)?;
            out.push('_');
            argument(sub, out)?;
            out.push('^');
            argument(sup, out)
        }
        (16, [numerator, denominator]) => {
            part(numerator, out)?;
            out.push('/');
            part(denominator, out)
        }
        (13, [inner]) => {
            out.push(symbols.first().copied().unwrap_or('('));
            write(inner, out)?;
            out.push(symbols.get(1).copied().unwrap_or(')'));
            Some(())
        }
        (21, [lower, upper, body]) => {
            out.push(symbols.first().copied().unwrap_or('∑'));
            if !lower.is_empty() {
                out.push('_');
                argument(lower, out)?;
            }
            if !upper.is_empty() {
                out.push('^');
                argument(upper, out)?;
            }
            out.push('▒');
            match &body[..] {
                [Math::Identifier(_) | Math::Number(_)] => write(body, out),
                _ => {
                    out.push('〖');
                    write(body, out)?;
                    out.push('〗');
                    Some(())
                }
            }
        }
        (25, [degree, radicand]) => {
            match &degree[..] {
                [] => out.push('√'),
                [Math::Number(n)] if n == "3" => out.push('∛'),
                [Math::Number(n)] if n == "4" => out.push('∜'),
                _ => {
                    out.push_str("√(");
                    write(degree, out)?;
                    out.push('&');
                    write(radicand, out)?;
                    out.push(')');
                    return Some(());
                }
            }
            argument(radicand, out)
        }
        (19, [base, below]) => {
            argument(base, out)?;
            out.push(if symbols == ['_'] { '_' } else { '┬' });
            argument(below, out)
        }
        (33, [base, above]) => {
            argument(base, out)?;
            out.push('┴');
            argument(above, out)
        }
        (10, [base]) => {
            argument(base, out)?;
            out.push('\u{a0}');
            out.push(*symbols.first()?);
            Some(())
        }
        (11, [inner]) if symbols == ['□'] => {
            out.push('□');
            argument(inner, out)
        }
        (12, [inner]) if symbols == ['▭'] => {
            out.push('▭');
            argument(inner, out)
        }
        (23, [inner]) if symbols == ['¯'] => {
            out.push('¯');
            argument(inner, out)
        }
        (20, cells) => {
            let width = usize::from(columns?.max(1));
            out.push_str("■(");
            for (index, cell) in cells.iter().enumerate() {
                if index > 0 {
                    out.push(if index % width == 0 { '@' } else { '&' });
                }
                write(cell, out)?;
            }
            out.push(')');
            Some(())
        }
        (15, rows) => {
            out.push_str("█(");
            for (index, row) in rows.iter().enumerate() {
                if index > 0 {
                    out.push('@');
                }
                write(row, out)?;
            }
            out.push(')');
            Some(())
        }
        _ => None,
    }
}
