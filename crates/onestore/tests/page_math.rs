use onestore::{
    RevisionIndex, Store,
    document::Document,
    page::{Math, Page, PageObject},
};

/// Three equations authored through OneNote 2010's equation editor, whose COM read exports
/// MathML (`corpus/m6/native-math-01`).
const NATIVE: &[u8] = include_bytes!("../../../corpus/m6/native-math-01/notebook/synthetic.one");
const NATIVE_READ: &str = include_str!("../../../corpus/m6/native-math-01/read/page-000.xml");

fn native_mathml() -> Vec<String> {
    NATIVE_READ
        .split("<mml:math")
        .skip(1)
        .map(|rest| {
            let body = &rest[rest.find('>').unwrap() + 1..rest.find("</mml:math>").unwrap()];
            let mut decoded = String::new();
            let mut remainder = body;
            while let Some(start) = remainder.find("&#") {
                decoded.push_str(&remainder[..start]);
                let end = start + remainder[start..].find(';').unwrap();
                let code: u32 = remainder[start + 2..end].parse().unwrap();
                decoded.push(char::from_u32(code).unwrap());
                remainder = &remainder[end + 1..];
            }
            decoded.push_str(remainder);
            decoded
        })
        .collect()
}

#[test]
fn native_equations_render_to_the_mathml_onenote_exports() {
    let store = Store::parse(NATIVE).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let page = document
        .pages()
        .unwrap()
        .into_iter()
        .map(|(space, _)| Page::from_space(&document, space).unwrap())
        .find(|page| page.title == "Equation controls")
        .unwrap();
    let mut rendered = Vec::new();
    for object in &page.objects {
        let PageObject::Outline(outline) = object else {
            continue;
        };
        for paragraph in &outline.paragraphs {
            let Some(text) = paragraph.text() else {
                continue;
            };
            if !Math::is_equation(&text.text) {
                continue;
            }
            // Inline math follows ordinary text in the same paragraph; only the math runs
            // form the equation.
            let mut at = 0;
            let math_runs: Vec<(String, onestore::document::Format)> = text
                .text
                .spans()
                .iter()
                .map(|span| {
                    let piece = text.text.text()[at..span.end].to_owned();
                    at = span.end;
                    (piece, span.format.clone())
                })
                .filter(|(_, format)| format.math == Some(true))
                .collect();
            let equation = onestore::page::Paragraph::from_runs(math_runs);
            rendered.push(Math::mathml(&Math::parse(&equation).unwrap()));
        }
    }
    assert_eq!(rendered, native_mathml());
}
