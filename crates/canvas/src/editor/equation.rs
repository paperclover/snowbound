//! Equations as OneNote 2010's equation editor edits them (`corpus/math-edit/native-linear`):
//! Alt+= starts an equation at the caret or makes one of the selected text, text typed into
//! an equation is its linear format until a space builds it up, and Linear and Professional
//! switch an equation between the forms, both of which OneNote stores.

use super::link::format_after;
use super::*;
use onestore::page::Math;

const OBJECT_START: char = '\u{fdd0}';
const ARGUMENT_SEPARATOR: char = '\u{fdee}';
const OBJECT_END: char = '\u{fdef}';

/// The run of math in `text` that UTF-16 `offset` lies in or touches.
pub(crate) fn zone(text: &Paragraph, offset: u32) -> Option<Range<u32>> {
    let (mut byte, mut unit) = (0, 0);
    let mut current: Option<Range<u32>> = None;
    for span in text.spans() {
        let end = unit + text.text()[byte..span.end].encode_utf16().count() as u32;
        if span.format.math == Some(true) {
            current = Some(
                current
                    .filter(|zone| zone.end == unit)
                    .map_or(unit, |zone| zone.start)..end,
            );
        } else if current
            .as_ref()
            .is_some_and(|zone| zone.start <= offset && offset <= zone.end)
        {
            return current;
        } else {
            current = None;
        }
        (byte, unit) = (span.end, end);
    }
    current.filter(|zone| zone.start <= offset && offset <= zone.end)
}

/// An object of an equation holding a position: its range, from its opening control to past
/// its closing one, and the argument the position lies in.
#[derive(PartialEq)]
struct Holder {
    range: Range<u32>,
    argument: usize,
    placeholder: bool,
}

/// The objects of `text` holding UTF-16 `offset` strictly inside, outermost first.
fn holders(text: &Paragraph, offset: u32) -> Vec<Holder> {
    // Each object's range and whether it is a placeholder; open objects and held ones by
    // index, with the argument reached.
    let mut objects: Vec<(Range<u32>, bool)> = Vec::new();
    let (mut open, mut held) = (Vec::new(), None);
    let mut unit = 0;
    for c in text.text().chars() {
        if unit == offset {
            held = Some(open.clone());
        }
        match c {
            OBJECT_START => {
                // An equation's empty placeholder, "Type equation here.", is a box OneNote
                // marks with this symbol.
                let placeholder = text.format_at(unit + 1).is_ok_and(|format| {
                    format
                        .math_object
                        .as_ref()
                        .is_some_and(|object| object.kind == 11 && object.symbols == ['⬚'])
                });
                open.push((objects.len(), 0));
                objects.push((unit..unit, placeholder));
            }
            ARGUMENT_SEPARATOR => {
                if let Some((_, argument)) = open.last_mut() {
                    *argument += 1;
                }
            }
            OBJECT_END => {
                if let Some((index, _)) = open.pop() {
                    objects[index].0.end = unit + 1;
                }
            }
            _ => {}
        }
        unit += c.len_utf16() as u32;
    }
    held.unwrap_or(open)
        .into_iter()
        .map(|(index, argument)| Holder {
            range: objects[index].0.clone(),
            argument,
            placeholder: objects[index].1,
        })
        .collect()
}

/// `range` as OneNote 2010's equation editor selects it (`corpus/equation-select`): an end
/// inside an object not holding the other end in the same argument takes the whole object, as
/// does an end inside a placeholder, and across paragraphs an object so taken at the end of
/// its paragraph takes the paragraph's end with it.
pub(crate) fn widen(
    document: &TextDocument,
    range: Range<TextPosition>,
) -> Result<Range<TextPosition>, EditError> {
    let text = |paragraph| document.paragraph(paragraph).ok_or(EditError::InvalidRange);
    let last = text(range.end.paragraph)?;
    let starts = holders(text(range.start.paragraph)?, range.start.offset);
    let ends = holders(last, range.end.offset);
    let common = if range.start.paragraph == range.end.paragraph {
        starts.iter().zip(&ends).take_while(|(a, b)| a == b).count()
    } else {
        0
    };
    let level = |holders: &[Holder]| {
        holders
            .iter()
            .position(|holder| holder.placeholder)
            .map_or(common, |placeholder| placeholder.min(common))
    };
    let mut widened = range.clone();
    if let Some(holder) = starts.get(level(&starts)) {
        widened.start.offset = holder.range.start;
    }
    if let Some(holder) = ends.get(level(&ends)) {
        widened.end.offset = holder.range.end;
        let next = range.end.paragraph + 1;
        let container = |paragraph| document.leaf(paragraph).map(|(container, ..)| container);
        if range.start.paragraph != range.end.paragraph
            && widened.end.offset == last.utf16_offset(last.text().len())?
            && container(next).is_some_and(|next| Some(next) == container(range.end.paragraph))
        {
            widened.end = TextPosition {
                paragraph: next,
                offset: 0,
            };
        }
    }
    Ok(widened)
}

/// The format text typed after an equation in `text` takes: the paragraph's last text's, or
/// its first run's without the equation editor's.
pub(super) fn text_after(text: &Paragraph) -> Format {
    let mut format = text
        .spans()
        .iter()
        .rev()
        .find(|span| span.format.math != Some(true))
        .map_or_else(
            || text.spans()[0].format.clone(),
            |span| span.format.clone(),
        );
    format.math = format.math.map(|_| false);
    format.embedded_object = None;
    format.math_object = None;
    if format.font.as_deref() == Some("Cambria Math") {
        // OneNote's body font, where the paragraph has no text to take one from.
        format.font = Some("Calibri".into());
        format.italic = format.italic.map(|_| false);
        // The writers' default, where the paragraph has no text to take a language from.
        format.language = Some(0x409);
    }
    format
}

impl CanvasEditor {
    /// The paragraph and math run at the caret.
    fn caret_zone(&self) -> Option<(usize, Range<u32>)> {
        let [anchor, focus] = self.active_outline().selection.positions;
        let caret = anchor.min(focus);
        let text = self.active_outline().document.paragraph(caret.paragraph)?;
        Some((caret.paragraph, zone(text, caret.offset)?))
    }

    /// Widens the selection an edit is about to replace as the equation editor selects.
    pub(super) fn take_objects(&mut self) -> Result<(), EditError> {
        let [anchor, focus] = self.active_outline().selection.positions;
        let range = anchor.min(focus)..anchor.max(focus);
        let widened = widen(&self.active_outline().document, range.clone())?;
        if widened != range {
            self.active_outline_mut().selection = Selection {
                positions: [widened.start, widened.end],
                affinities: [Affinity::Downstream, Affinity::Upstream],
            };
        }
        Ok(())
    }

    /// Whether the caret is in an equation, which Linear and Professional act on.
    pub(crate) fn in_equation(&self) -> bool {
        self.caret_zone().is_some()
    }

    /// Professional: builds the equation at the caret up from its linear text.
    pub fn build_equation(&mut self, engine: &mut TextEngine) -> Result<bool, EditorError> {
        let Some((paragraph, zone)) = self.caret_zone() else {
            return Ok(false);
        };
        self.rebuild(engine, paragraph, zone, None)
    }

    /// Linear: shows the equation at the caret in its linear format, as text to edit.
    pub fn linear_equation(&mut self, engine: &mut TextEngine) -> Result<bool, EditorError> {
        let Some((paragraph, zone)) = self.caret_zone() else {
            return Ok(false);
        };
        let text = self
            .active_outline()
            .document
            .paragraph(paragraph)
            .ok_or(EditError::InvalidRange)?;
        let math = text.slice(zone.clone())?;
        let Ok(nodes) = Math::parse(&math) else {
            return Ok(false);
        };
        let Some(linear) = Math::linear_paragraph(&nodes, &math.spans()[0].format) else {
            return Ok(false);
        };
        let end = zone.start + linear.utf16_offset(linear.text().len())?;
        self.replace_zone(engine, paragraph, zone, linear, end)
    }

    /// Alt+=: the selected text becomes an equation, the equation at the caret builds up, or
    /// what is typed next at the caret starts one.
    pub fn insert_equation(&mut self, engine: &mut TextEngine) -> Result<(), EditorError> {
        let outline = self.active_outline();
        let [anchor, focus] = outline.selection.positions;
        let (start, end) = (anchor.min(focus), anchor.max(focus));
        if start == end {
            if self.caret_zone().is_some() {
                self.build_equation(engine)?;
                return Ok(());
            }
            let format = Math::format(&self.typing_format(start)?);
            self.pending = Some((outline.id, start, format));
            return Ok(());
        }
        // Across paragraphs each paragraph's part becomes an equation of its own, as in
        // OneNote 2010, in one undo step.
        let depth = self.undo.len();
        let result = (|| {
            for paragraph in start.paragraph..=end.paragraph {
                let text = self
                    .active_outline()
                    .document
                    .paragraph(paragraph)
                    .ok_or(EditError::InvalidRange)?;
                let from = if paragraph == start.paragraph {
                    start.offset
                } else {
                    0
                };
                let to = if paragraph == end.paragraph {
                    end.offset
                } else {
                    text.utf16_offset(text.text().len())?
                };
                if from == to {
                    continue;
                }
                let linear = text
                    .slice(from..to)?
                    .project()?
                    .text()
                    .text()
                    .replace('\n', " ");
                let base = format_after(text, from).cloned().unwrap_or_default();
                let math = Math::paragraph(&Math::from_linear(&linear), &base);
                let caret = from + math.utf16_offset(math.text().len())?;
                self.replace_zone(engine, paragraph, from..to, math, caret)?;
            }
            Ok(())
        })();
        self.group(depth, false);
        result
    }

    /// Builds the equation up after the space typed at `typed` ended part of it, as the
    /// equation editor builds as it is typed. Only a caret at the end of the equation, or
    /// only closing its objects before that end, builds; the caret stays as deep in the built
    /// objects.
    pub(super) fn build_typed_equation(
        &mut self,
        engine: &mut TextEngine,
        typed: TextPosition,
    ) -> Result<(), EditorError> {
        let text = self
            .active_outline()
            .document
            .paragraph(typed.paragraph)
            .ok_or(EditError::InvalidRange)?;
        if format_after(text, typed.offset).is_none_or(|format| format.math != Some(true)) {
            return Ok(());
        }
        let Some(zone) = zone(text, typed.offset) else {
            return Ok(());
        };
        let caret = typed.offset + 1;
        let after = &text.text()[text.byte_offset(caret)?..text.byte_offset(zone.end)?];
        if !after.chars().all(|c| c == OBJECT_END) {
            return Ok(());
        }
        let depth = after.chars().count() as u32;
        self.rebuild(engine, typed.paragraph, zone, Some(depth))?;
        Ok(())
    }

    /// Right at the end of an equation ending its paragraph leaves it: what is typed next is
    /// text again.
    pub(super) fn leave_equation(&mut self) -> Result<bool, EditError> {
        let outline = self.active_outline();
        let [anchor, focus] = outline.selection.positions;
        let Some(text) = outline.document.paragraph(focus.paragraph) else {
            return Ok(false);
        };
        let end = text.utf16_offset(text.text().len())?;
        let math = self.typing_format(focus)?.math == Some(true);
        if anchor != focus || focus.offset != end || !math {
            return Ok(false);
        }
        self.pending = Some((outline.id, focus, text_after(text)));
        Ok(true)
    }

    /// Rebuilds math run `zone` of paragraph `paragraph` from its linear text. With `depth`,
    /// the caret goes that many objects before the end of the built run, and into an
    /// argument left empty there; without, to the run's end.
    fn rebuild(
        &mut self,
        engine: &mut TextEngine,
        paragraph: usize,
        zone: Range<u32>,
        depth: Option<u32>,
    ) -> Result<bool, EditorError> {
        let text = self
            .active_outline()
            .document
            .paragraph(paragraph)
            .ok_or(EditError::InvalidRange)?;
        let math = text.slice(zone.clone())?;
        let Some(linear) = Math::parse(&math)
            .ok()
            .and_then(|nodes| Math::linear(&nodes))
        else {
            return Ok(false);
        };
        let nodes = if depth.is_some() {
            Math::typed(&linear)
        } else {
            Math::from_linear(&linear)
        };
        let built = Math::paragraph(&nodes, &math.spans()[0].format);
        if built == math {
            return Ok(false);
        }
        let units: Vec<char> = built.text().chars().collect();
        let mut caret = units.len() - depth.unwrap_or(0).min(units.len() as u32) as usize;
        if depth.is_some()
            && caret >= 2
            && units[caret - 1] == OBJECT_END
            && matches!(units[caret - 2], ARGUMENT_SEPARATOR | OBJECT_START)
        {
            caret -= 1;
        }
        let caret = zone.start
            + units[..caret]
                .iter()
                .map(|c| c.len_utf16() as u32)
                .sum::<u32>();
        self.replace_zone(engine, paragraph, zone, built, caret)
    }

    /// Enter inside an equation, as OneNote 2010's equation editor takes it
    /// (`corpus/math-edit/native-enter`): in the equation's own row a line break the
    /// paragraph keeps, in an object's argument a new row of it, the argument becoming an
    /// equation array where it is not one. False where the caret is not inside an equation.
    pub(super) fn break_equation(&mut self, engine: &mut TextEngine) -> Result<bool, EditorError> {
        let caret = self.active_outline().selection.positions[1];
        let Some((paragraph, zone)) = self.caret_zone() else {
            return Ok(false);
        };
        let text = self
            .active_outline()
            .document
            .paragraph(paragraph)
            .ok_or(EditError::InvalidRange)?;
        let units: Vec<char> = text.slice(zone.clone())?.text().chars().collect();
        let at = text.slice(zone.start..caret.offset)?.text().chars().count();
        if at == 0 || at == units.len() || !units.contains(&OBJECT_START) {
            return Ok(false);
        }
        // The argument around the caret: from the control before it at its depth to the one
        // after.
        let mut depth = 0;
        let start = units[..at].iter().rposition(|&c| {
            match c {
                OBJECT_END => depth += 1,
                OBJECT_START if depth > 0 => depth -= 1,
                OBJECT_START | ARGUMENT_SEPARATOR if depth == 0 => return true,
                _ => {}
            }
            false
        });
        let Some(start) = start else {
            // The equation's own row breaks its line, outside the math.
            let mut updated = text.clone();
            let break_format = text_after(text);
            updated.apply(onestore::page::text::Edit {
                range: caret.offset..caret.offset,
                replacement: Paragraph::new("\r".into(), break_format),
            })?;
            let after = TextPosition {
                paragraph,
                offset: caret.offset + 1,
            };
            self.rewrite(engine, paragraph, updated, [after; 2].into())?;
            return Ok(true);
        };
        let mut depth = 0;
        let end = at
            + units[at..]
                .iter()
                .position(|&c| {
                    match c {
                        OBJECT_START => depth += 1,
                        OBJECT_END if depth > 0 => depth -= 1,
                        OBJECT_END | ARGUMENT_SEPARATOR if depth == 0 => return true,
                        _ => {}
                    }
                    false
                })
                .ok_or(EditError::InvalidStructure)?;
        // The object the argument belongs to opens at the start of its first argument.
        let mut depth = 0;
        let opening = units[..=start]
            .iter()
            .rposition(|&c| {
                match c {
                    OBJECT_END => depth += 1,
                    OBJECT_START if depth > 0 => depth -= 1,
                    OBJECT_START => return true,
                    _ => {}
                }
                false
            })
            .ok_or(EditError::InvalidStructure)?;
        let math = text.slice(zone.clone())?;
        let offset =
            |index: usize| -> u32 { units[..index].iter().map(|c| c.len_utf16() as u32).sum() };
        let kind = math
            .format_at(offset(opening) + 1)?
            .math_object
            .as_ref()
            .map(|object| object.kind);
        let format = math.format_at(offset(at))?.clone();
        let control = |c: char, kind: u32| {
            let mut format = format.clone();
            format.math_object = Some(onestore::document::MathObject {
                kind,
                arguments: None,
                columns: Some(1),
                symbols: vec!['█'],
            });
            (c.to_string(), format)
        };
        let piece = |range: Range<usize>| -> Result<Paragraph, EditError> {
            math.slice(offset(range.start)..offset(range.end))
        };
        let mut parts = vec![piece(0..start + 1)?];
        if kind != Some(15) {
            parts.push(Paragraph::from_runs([control(OBJECT_START, 15)]));
        }
        parts.push(piece(start + 1..at)?);
        parts.push(Paragraph::from_runs([control(ARGUMENT_SEPARATOR, 15)]));
        parts.push(piece(at..end)?);
        if kind != Some(15) {
            parts.push(Paragraph::from_runs([control(OBJECT_END, 15)]));
        }
        parts.push(piece(end..units.len())?);
        let mut broken = parts.remove(0);
        for part in parts {
            broken.append(part)?;
        }
        let Ok(nodes) = Math::parse(&broken) else {
            return Ok(false);
        };
        let built = Math::paragraph(&nodes, &math.spans()[0].format);
        // The caret starts the new row: after as many characters other than spaces as came
        // before it.
        let before = broken
            .text()
            .chars()
            .take(at + if kind == Some(15) { 1 } else { 2 });
        let count = before.filter(|c| *c != ' ').count();
        let mut seen = 0;
        let mut caret = zone.start;
        for c in built.text().chars() {
            if seen == count {
                break;
            }
            seen += usize::from(c != ' ');
            caret += c.len_utf16() as u32;
        }
        self.replace_zone(engine, paragraph, zone, built, caret)
    }

    fn replace_zone(
        &mut self,
        engine: &mut TextEngine,
        paragraph: usize,
        zone: Range<u32>,
        math: Paragraph,
        caret: u32,
    ) -> Result<bool, EditorError> {
        let mut text = self
            .active_outline()
            .document
            .paragraph(paragraph)
            .ok_or(EditError::InvalidRange)?
            .clone();
        text.apply(onestore::page::text::Edit {
            range: zone,
            replacement: math,
        })?;
        let caret = TextPosition {
            paragraph,
            offset: caret,
        };
        self.rewrite(engine, paragraph, text, [caret; 2].into())?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use onestore::document::MathObject;
    use onestore::page::PageObject;

    type Runs = Vec<(String, Option<MathObject>, Option<bool>)>;

    fn runs(text: &Paragraph) -> Runs {
        let mut start = 0;
        text.spans()
            .iter()
            .map(|span| {
                let run = text.text()[start..span.end].to_owned();
                start = span.end;
                (run, span.format.math_object.clone(), span.format.italic)
            })
            .collect()
    }

    fn equations(bytes: &[u8]) -> Vec<Runs> {
        let store = onestore::Store::parse(bytes).unwrap();
        let index = onestore::RevisionIndex::parse(&store).unwrap();
        let document = onestore::document::Document::parse(&index).unwrap();
        let (space, _) = document.pages().unwrap()[0];
        Page::from_space(&document, space)
            .unwrap()
            .objects
            .iter()
            .filter_map(|object| match object {
                PageObject::Outline(outline) => Some(outline),
                _ => None,
            })
            .flat_map(|outline| &outline.paragraphs)
            .filter_map(|paragraph| paragraph.text())
            .filter(|text| Math::is_equation(&text.text))
            .map(|text| {
                // Text typed after an equation, outside it, is not the editor's.
                let mut runs = runs(&text.text);
                runs.retain(|(_, object, _)| object.is_some());
                runs
            })
            .collect()
    }

    /// Alt+= and typing, each space its own key, store what OneNote 2010's equation editor
    /// stored for the same keys (`tools/native_math.py`), building up as they are typed.
    #[test]
    fn typed_equations_build_up_as_the_equation_editor_stores_them() {
        let sessions: [(&[u8], &[Option<&str>]); 4] = [
            (
                include_bytes!("../../../../corpus/math-edit/native-editor/notebook/links.one"),
                &[
                    Some("a_1+b_2"),
                    Some("x_i^2"),
                    Some("(a+b)"),
                    Some("\\int_0^1 x dx"),
                    Some("\\sum_(i=1)^n i"),
                ],
            ),
            (
                include_bytes!("../../../../corpus/math-edit/native-editor-2/notebook/links.one"),
                &[
                    Some("\\sqrt x+1"),
                    Some("\\cbrt(x)"),
                    Some("\\sqrt(n&x)"),
                    Some("a/b"),
                    Some("\\lim_(x\\to 0) f(x)"),
                    Some("\\prod_(k=1)^n k"),
                    Some("[a+b]"),
                    Some("\\overline(x)"),
                    Some("x\\hat"),
                ],
            ),
            (
                include_bytes!("../../../../corpus/math-edit/native-editor-3/notebook/links.one"),
                &[
                    Some("\\matrix(1&2@3&4)"),
                    Some("\\eqarray(x&=1@y&=2)"),
                    Some("x\\above 2"),
                    Some("x\\below 2"),
                    Some("\\box(x)"),
                    Some("\\rect(x)"),
                    None,
                    Some("\\iint x dx dy"),
                    Some("f(x)/(x^2+1)"),
                    Some("\\sum^n x"),
                ],
            ),
            (
                include_bytes!("../../../../corpus/math-edit/native-editor-4/notebook/links.one"),
                &[
                    Some("a_1+b_2"),
                    // The editor was still starting this one and kept its placeholder.
                    None,
                    Some("\\sum_(i=1)^n i"),
                    Some("\\int_0^1 x dx"),
                    Some("\\sqrt x+1"),
                    Some("\\cbrt(x)"),
                    Some("\\sqrt(n&x)"),
                    Some("a/b"),
                    Some("\\prod_(k=1)^n k"),
                    Some("[a+b]"),
                    Some("\\overline(x)"),
                    Some("x\\hat"),
                    Some("\\matrix(1&2@3&4)"),
                    Some("\\eqarray(x&=1@y&=2)"),
                    Some("x\\above 2"),
                    Some("x\\below 2"),
                    Some("\\box(x)"),
                    Some("\\rect(x)"),
                    Some("f(x)/(x^2+1)"),
                    Some("\\sum^n x"),
                    Some("e^(x+1)"),
                    Some("\\alpha+\\beta"),
                    Some("(a+b)/(c+d)"),
                    Some("x^2+y^2=z^2"),
                    // A function applied to a limit, an object this reading does not build.
                    None,
                    Some("{a+b}"),
                ],
            ),
        ];
        let mut engine = TextEngine::default();
        for (bytes, typed) in sessions {
            let native = equations(bytes);
            assert_eq!(native.len(), typed.len());
            for (native, typed) in native.iter().zip(typed) {
                let Some(typed) = typed else {
                    continue;
                };
                let mut editor = CanvasEditor::new(
                    &mut engine,
                    TextDocument::new(vec![Paragraph::new(String::new(), Format::default())])
                        .unwrap(),
                    400.0,
                )
                .unwrap();
                editor.insert_equation(&mut engine).unwrap();
                for (index, word) in format!("{typed} ").split(' ').enumerate() {
                    if index > 0 {
                        editor.insert(&mut engine, " ").unwrap();
                    }
                    if !word.is_empty() {
                        editor.insert(&mut engine, word).unwrap();
                    }
                }
                let text = editor.active_outline().document.paragraph(0).unwrap();
                assert_eq!(&runs(text), native, "{typed}");
            }
        }
    }
}
