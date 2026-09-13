use onestore::{
    RevisionIndex, Store,
    document::Document,
    page::{Math, Page, PageObject},
};

/// Three equations authored through OneNote 2010's equation editor, whose COM read exports
/// MathML (`corpus/m6/native-math-01`).
const NATIVE: &[u8] = include_bytes!("../../../corpus/m6/native-math-01/notebook/synthetic.one");
const NATIVE_READ: &str = include_str!("../../../corpus/m6/native-math-01/read/page-000.xml");
/// Five more expressions typed through the equation editor by `tools/native_math.py`:
/// subscripts, a sub-superscript, parentheses, an integral and a sum with limits.
const EDITOR: &[u8] = include_bytes!("../../../corpus/math-edit/native-editor/notebook/links.one");
const EDITOR_READ: &str = include_str!("../../../corpus/math-edit/native-editor/read/page-000.xml");
/// A second editor session: square and n-th roots, a plain fraction, a limit, a product,
/// square brackets, an overbar and a hat.
const EDITOR_2: &[u8] =
    include_bytes!("../../../corpus/math-edit/native-editor-2/notebook/links.one");
const EDITOR_2_READ: &str =
    include_str!("../../../corpus/math-edit/native-editor-2/read/page-000.xml");

fn native_mathml(read: &str) -> Vec<String> {
    read.split("<mml:math")
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

fn rendered_equations(bytes: &[u8], title: &str) -> Vec<String> {
    let store = Store::parse(bytes).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let page = document
        .pages()
        .unwrap()
        .into_iter()
        .map(|(space, _)| Page::from_space(&document, space).unwrap())
        .find(|page| page.title == title)
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
    rendered
}

#[test]
fn native_equations_render_to_the_mathml_onenote_exports() {
    assert_eq!(
        rendered_equations(NATIVE, "Equation controls"),
        native_mathml(NATIVE_READ)
    );
}

#[test]
fn equation_editor_expressions_render_to_the_mathml_onenote_exports() {
    assert_eq!(
        rendered_equations(EDITOR, "Read about Rust the Rust site"),
        native_mathml(EDITOR_READ)
    );
    assert_eq!(
        rendered_equations(EDITOR_2, "Read about Rust the Rust site"),
        native_mathml(EDITOR_2_READ)
    );
}

fn object(kind: u32, symbols: &str, arguments: Vec<Vec<Math>>) -> Math {
    Math::Object {
        kind,
        symbols: symbols.chars().collect(),
        arguments,
    }
}

fn built_equations() -> Vec<(Vec<Math>, &'static str)> {
    vec![
        (
            vec![
                object(
                    31,
                    "^",
                    vec![vec![Math::Identifier('x')], vec![Math::Number("2".into())]],
                ),
                Math::Operator('+'),
                Math::Number("1".into()),
            ],
            "<mml:msup><mml:mi>x</mml:mi><mml:mn>2</mml:mn></mml:msup><mml:mo>+</mml:mo><mml:mn>1</mml:mn>",
        ),
        (
            vec![object(
                16,
                "/",
                vec![
                    vec![
                        Math::Identifier('a'),
                        Math::Operator('+'),
                        Math::Identifier('b'),
                    ],
                    vec![object(
                        31,
                        "^",
                        vec![vec![Math::Identifier('c')], vec![Math::Number("2".into())]],
                    )],
                ],
            )],
            "<mml:mfrac><mml:mrow><mml:mi>a</mml:mi><mml:mo>+</mml:mo><mml:mi>b</mml:mi></mml:mrow><mml:mrow><mml:msup><mml:mi>c</mml:mi><mml:mn>2</mml:mn></mml:msup></mml:mrow></mml:mfrac>",
        ),
    ]
}

#[test]
fn equations_built_from_the_tree_store_the_native_linear_form_and_parse_back() {
    for (nodes, mathml) in built_equations() {
        let paragraph = Math::paragraph(&nodes, &onestore::document::Format::default());
        assert!(Math::is_equation(&paragraph));
        assert_eq!(Math::parse(&paragraph).unwrap(), nodes);
        assert_eq!(Math::mathml(&nodes), mathml);
    }
    let paragraph = Math::paragraph(&built_equations()[0].0, &Default::default());
    assert_eq!(paragraph.text(), "\u{fdd0}\u{1d465}\u{fdee}2\u{fdef}+1");
    let kinds: Vec<(Option<u32>, Option<u32>)> = paragraph
        .spans()
        .iter()
        .map(|s| {
            let object = s.format.math_object.as_ref().unwrap();
            (Some(object.kind), object.arguments)
        })
        .collect();
    assert_eq!(
        kinds,
        [
            (Some(31), Some(2)),
            (Some(31), None),
            (Some(31), Some(1)),
            (Some(0x9000_0000), None)
        ]
    );
}

/// `ONESTORE_MATH_EXPORT` names a new directory receiving the candidate for a cold reopen:
/// every native equation copied onto a fresh page, then two built from the tree.
#[test]
fn equations_are_written_and_read_back() {
    use onestore::page::{ParagraphContent, TextObject, text::new_id};
    let source = onestore::create_section("math.one", "Equations below", "Author").unwrap();
    let store = Store::parse(&source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let (space, _) = document.pages().unwrap()[0];
    let before = Page::from_space(&document, space).unwrap();
    let mut after = before.clone();
    let mut expected_mathml = Vec::new();
    let mut paragraphs = Vec::new();
    for (bytes, title, read) in [
        (NATIVE, "Equation controls", NATIVE_READ),
        (EDITOR, "Read about Rust the Rust site", EDITOR_READ),
    ] {
        expected_mathml.extend(native_mathml(read));
        let store = Store::parse(bytes).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let document = Document::parse(&index).unwrap();
        let page = document
            .pages()
            .unwrap()
            .into_iter()
            .map(|(space, _)| Page::from_space(&document, space).unwrap())
            .find(|page| page.title == title)
            .unwrap();
        for object in &page.objects {
            let PageObject::Outline(outline) = object else {
                continue;
            };
            for paragraph in &outline.paragraphs {
                if paragraph.text().is_some_and(|t| Math::is_equation(&t.text)) {
                    paragraphs.push(paragraph.clone());
                }
            }
        }
    }
    for (nodes, mathml) in built_equations() {
        let mut paragraph = paragraphs[0].clone();
        paragraph.content = ParagraphContent::Text(TextObject {
            id: new_id().unwrap(),
            date_field: None,
            text: Math::paragraph(&nodes, &Default::default()),
            tags: Vec::new(),
        });
        paragraphs.push(paragraph);
        expected_mathml.push(mathml.to_owned());
    }
    let outline = after
        .objects
        .iter_mut()
        .find_map(|object| match object {
            PageObject::Outline(outline) if !outline.title => Some(outline),
            _ => None,
        })
        .unwrap();
    for mut paragraph in paragraphs {
        paragraph.id = new_id().unwrap();
        paragraph.parent = None;
        paragraph.level = 1;
        paragraph.lists.clear();
        paragraph.tags.clear();
        paragraph.style = None;
        paragraph.collapsed = false;
        if let ParagraphContent::Text(text) = &mut paragraph.content {
            text.id = new_id().unwrap();
            text.tags.clear();
            // Paragraph-level spacing came from the source page's paragraph style.
            let mut at = 0;
            text.text =
                onestore::page::Paragraph::from_runs(text.text.spans().iter().map(|span| {
                    let piece = text.text.text()[at..span.end].to_owned();
                    at = span.end;
                    let mut format = span.format.clone();
                    format.space_before = None;
                    format.space_after = None;
                    format.line_spacing = None;
                    (piece, format)
                }));
        }
        outline.paragraphs.push(paragraph);
    }
    let written = onestore::PreparedEdit::page(&source, space, &after, "Math author").unwrap();
    let store = Store::parse(written.as_bytes()).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let stored = Page::from_space(&document, space).unwrap();
    let mut expected = after.clone();
    expected.title = stored.title.clone();
    let body = |page: &Page| {
        page.objects
            .iter()
            .find_map(|object| match object {
                PageObject::Outline(outline) if !outline.title => Some(outline.paragraphs.clone()),
                _ => None,
            })
            .unwrap()
    };
    for (index, (a, b)) in body(&stored).iter().zip(body(&expected)).enumerate() {
        if a != &b {
            let (ta, tb) = (a.text().unwrap(), b.text().unwrap());
            assert_eq!(ta.text.text(), tb.text.text(), "paragraph {index} text");
            for (sa, sb) in ta.text.spans().iter().zip(tb.text.spans()) {
                assert_eq!(sa, sb, "paragraph {index} span");
            }
            assert_eq!(
                ta.text.spans().len(),
                tb.text.spans().len(),
                "paragraph {index} spans"
            );
            assert_eq!(a, &b, "paragraph {index}");
        }
    }
    assert_eq!(stored, expected);
    assert_eq!(
        rendered_equations(written.as_bytes(), &stored.title),
        expected_mathml
    );
    if let Some(directory) = std::env::var_os("ONESTORE_MATH_EXPORT") {
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(directory.join("math.one"), written.as_bytes()).unwrap();
        std::fs::write(
            directory.join("Open Notebook.onetoc2"),
            onestore::create_table_of_contents(
                "Open Notebook.onetoc2",
                &[("math.one", store.header.file_id)],
            )
            .unwrap(),
        )
        .unwrap();
        std::fs::write(
            directory.join("expected-mathml.json"),
            serde_json::to_string_pretty(&expected_mathml).unwrap(),
        )
        .unwrap();
    }
}
