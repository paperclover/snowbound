#[path = "support/ops.rs"]
mod ops;
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
/// A third session: a matrix, an equation array, upper and lower limit objects, boxes, a
/// nested fraction and a sum with only an upper limit.
const EDITOR_3: &[u8] =
    include_bytes!("../../../corpus/math-edit/native-editor-3/notebook/links.one");
const EDITOR_3_READ: &str =
    include_str!("../../../corpus/math-edit/native-editor-3/read/page-000.xml");
/// A fourth session repeating most of the others and adding an exponent, Greek letters, a
/// fraction of sums, a Pythagorean sum, a limit applied as a function and braces.
const EDITOR_4: &[u8] =
    include_bytes!("../../../corpus/math-edit/native-editor-4/notebook/links.one");

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
            // Inline math follows ordinary text in the same paragraph, and a line break
            // divides an equation: each run of math is an equation of its own.
            let mut at = 0;
            let mut zones: Vec<Vec<(String, onestore::document::Format)>> = vec![Vec::new()];
            for span in text.text.spans() {
                let piece = text.text.text()[at..span.end].to_owned();
                at = span.end;
                if span.format.math == Some(true) {
                    zones.last_mut().unwrap().push((piece, span.format.clone()));
                } else if !zones.last().unwrap().is_empty() {
                    zones.push(Vec::new());
                }
            }
            for zone in zones.into_iter().filter(|zone| !zone.is_empty()) {
                let equation = onestore::page::Paragraph::from_runs(zone);
                rendered.push(Math::mathml(&Math::parse(&equation).unwrap()));
            }
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

/// Enter inside equations (`tools/native_enter.py`): a line break dividing one, and an
/// argument made an equation array.
const ENTER: &[u8] = include_bytes!("../../../corpus/math-edit/native-enter/notebook/links.one");
const ENTER_READ: &str = include_str!("../../../corpus/math-edit/native-enter/read/page-000.xml");

#[test]
fn equation_editor_expressions_render_to_the_mathml_onenote_exports() {
    assert_eq!(
        rendered_equations(ENTER, "Read about Rust the Rust site"),
        native_mathml(ENTER_READ)
    );
    assert_eq!(
        rendered_equations(EDITOR, "Read about Rust the Rust site"),
        native_mathml(EDITOR_READ)
    );
    assert_eq!(
        rendered_equations(EDITOR_2, "Read about Rust the Rust site"),
        native_mathml(EDITOR_2_READ)
    );
    assert_eq!(
        rendered_equations(EDITOR_3, "Read about Rust the Rust site"),
        native_mathml(EDITOR_3_READ)
    );
}

fn object(kind: u32, symbols: &str, arguments: Vec<Vec<Math>>) -> Math {
    Math::Object {
        kind,
        symbols: symbols.chars().collect(),
        columns: None,
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
    let written = ops::saved(&source, space, &after).unwrap();
    let store = Store::parse(written.as_slice()).unwrap();
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
        rendered_equations(written.as_slice(), &stored.title),
        expected_mathml
    );
    if let Some(directory) = std::env::var_os("ONESTORE_MATH_EXPORT") {
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(directory.join("math.one"), written.as_slice()).unwrap();
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

/// Backspacing through inline math leaves plain text in the model, but the stored text object
/// still holds the math runs, which the ordinary text edit refuses.
#[test]
fn a_paragraph_emptied_of_its_inline_math_saves() {
    let store = Store::parse(NATIVE).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let (space, before) = document
        .pages()
        .unwrap()
        .into_iter()
        .map(|(space, _)| (space, Page::from_space(&document, space).unwrap()))
        .find(|(_, page)| page.title == "Equation controls")
        .unwrap();
    let mut after = before.clone();
    let text = after
        .objects
        .iter_mut()
        .filter_map(|object| match object {
            PageObject::Outline(outline) => Some(outline),
            _ => None,
        })
        .flat_map(|outline| &mut outline.paragraphs)
        .filter_map(|paragraph| match &mut paragraph.content {
            onestore::page::ParagraphContent::Text(text) => Some(text),
            _ => None,
        })
        .find(|text| text.text.text() == "Inline 𝛼+𝛽")
        .unwrap();
    let plain = text.text.spans()[0].format.clone();
    assert_ne!(plain.math, Some(true));
    text.text = onestore::page::Paragraph::new(String::new(), plain);
    let written = ops::saved(NATIVE, space, &after).unwrap();
    let store = Store::parse(written.as_slice()).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    assert_eq!(Page::from_space(&document, space).unwrap(), after);
}

type MathRun = (String, Option<onestore::document::MathObject>, Option<bool>);

/// The math runs of each equation paragraph on the page titled `title`: text, run data and
/// whether the run is italic, which is what the equation editor decides.
fn stored_equations(bytes: &[u8], title: &str) -> Vec<Vec<MathRun>> {
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
    let mut equations = Vec::new();
    for object in &page.objects {
        let PageObject::Outline(outline) = object else {
            continue;
        };
        for paragraph in &outline.paragraphs {
            if let Some(text) = paragraph
                .text()
                .filter(|text| Math::is_equation(&text.text))
            {
                equations.push(runs(&text.text));
            }
        }
    }
    equations
}

fn runs(paragraph: &onestore::page::Paragraph) -> Vec<MathRun> {
    let mut at = 0;
    paragraph
        .spans()
        .iter()
        .filter_map(|span| {
            let text = paragraph.text()[at..span.end].to_owned();
            at = span.end;
            (span.format.math == Some(true))
                .then(|| (text, span.format.math_object.clone(), span.format.italic))
        })
        .collect()
}

/// What OneNote 2010's equation editor stored for text typed into it, each followed by the
/// space that builds it up, in the order the equations appear; `None` skips an equation the
/// editor built in a way this reading does not follow.
const TYPED: &[(&[u8], &[Option<&str>])] = &[
    (
        EDITOR,
        &[
            Some("a_1+b_2"),
            Some("x_i^2"),
            Some("(a+b)"),
            Some("\\int_0^1 x dx"),
            Some("\\sum_(i=1)^n i"),
        ],
    ),
    (
        EDITOR_2,
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
        EDITOR_3,
        &[
            Some("\\matrix(1&2@3&4)"),
            Some("\\eqarray(x&=1@y&=2)"),
            Some("x\\above 2"),
            Some("x\\below 2"),
            Some("\\box(x)"),
            Some("\\rect(x)"),
            // `\underline` became an unrelated symbol.
            None,
            Some("\\iint x dx dy"),
            Some("f(x)/(x^2+1)"),
            Some("\\sum^n x"),
        ],
    ),
    (
        EDITOR_4,
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

#[test]
fn typed_linear_text_builds_up_as_the_equation_editor_stores_it() {
    for (bytes, typed) in TYPED {
        let stored = stored_equations(bytes, "Read about Rust the Rust site");
        assert_eq!(stored.len(), typed.len());
        for (stored, typed) in stored.iter().zip(*typed) {
            let Some(typed) = typed else {
                continue;
            };
            let built = Math::paragraph(
                &Math::from_linear(&format!("{typed} ")),
                &onestore::document::Format::default(),
            );
            assert_eq!(&runs(&built), stored, "{typed}");
        }
    }
}

/// Every native equation reads back from the linear text written for it, except the two
/// the linear format here cannot write: a limit applied as a function and an empty
/// equation's placeholder (`EDITOR_4`).
#[test]
fn native_equations_survive_their_linear_form() {
    let mut unwritten = 0;
    for (bytes, title) in [
        (NATIVE, "Equation controls"),
        (EDITOR, "Read about Rust the Rust site"),
        (EDITOR_2, "Read about Rust the Rust site"),
        (EDITOR_3, "Read about Rust the Rust site"),
        (EDITOR_4, "Read about Rust the Rust site"),
    ] {
        for stored in stored_equations(bytes, title) {
            let paragraph = onestore::page::Paragraph::from_runs(stored.iter().map(
                |(text, object, italic)| {
                    let format = onestore::document::Format {
                        math: Some(true),
                        math_object: object.clone(),
                        italic: *italic,
                        ..Default::default()
                    };
                    (text.clone(), format)
                },
            ));
            let Some(linear) = Math::linear(&Math::parse(&paragraph).unwrap()) else {
                unwritten += 1;
                continue;
            };
            let rebuilt = Math::paragraph(&Math::from_linear(&linear), &Default::default());
            // Linear text without objects stores no run data until it is built up.
            if stored.iter().all(|(_, object, _)| object.is_none()) {
                assert_eq!(rebuilt.text(), paragraph.text());
                continue;
            }
            assert_eq!(runs(&rebuilt), stored, "{linear}");
        }
    }
    assert_eq!(unwritten, 2);
}

/// OneNote 2010's Linear on equations typed through its editor (`corpus/math-edit/native-linear`):
/// the tree built from the typed text writes as the linear text OneNote showed and stored,
/// and that text builds up to the same tree.
#[test]
fn equations_write_the_linear_text_onenote_shows() {
    const LINEAR: &[u8] =
        include_bytes!("../../../corpus/math-edit/native-linear/notebook/links.one");
    let typed = [
        "x^2+1",
        "(a+b)/(c+d)",
        "a_1+b_2",
        "x_i^2",
        "\\sum_(i=1)^n i",
        "\\int_0^1 x dx",
        "\\sqrt x+1",
        "\\cbrt(x)",
        "\\sqrt(n&x)",
        "a/b",
        "\\prod_(k=1)^n k",
        "[a+b]",
        "\\overline(x)",
        "x\\hat",
        "\\matrix(1&2@3&4)",
        "\\eqarray(x&=1@y&=2)",
        "x\\above 2",
        "x\\below 2",
        "\\box(x)",
        "\\rect(x)",
        "f(x)/(x^2+1)",
        "\\sum^n x",
        "e^(x+1)",
        "\\alpha+\\beta",
    ];
    let shown: Vec<String> = stored_equations(LINEAR, "Read about Rust the Rust siteR")
        .into_iter()
        .map(|runs| runs.into_iter().map(|(text, ..)| text).collect())
        .collect();
    assert_eq!(shown.len(), typed.len());
    for (shown, typed) in shown.iter().zip(typed) {
        let built = Math::from_linear(&format!("{typed} "));
        assert_eq!(
            Math::linear(&built).as_deref(),
            Some(shown.as_str()),
            "{typed}"
        );
        let base = onestore::document::Format::default();
        assert_eq!(
            Math::paragraph(&Math::from_linear(shown), &base),
            Math::paragraph(&built, &base),
            "{typed}"
        );
    }
}
