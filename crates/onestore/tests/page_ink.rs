#[path = "support/ops.rs"]
mod ops;
use onestore::{
    ExGuid, RevisionIndex, Store,
    document::Document,
    page::{Ink, InkStroke, Page, PageObject, text::new_id},
};


/// Two drawings made with the mouse in OneNote 2010 (`tools/native/ink.ahk`): a diamond and a
/// diagonal line, whose native read reports their positions and sizes.
const NATIVE: &[u8] =
    include_bytes!("../../../corpus/native-ink/cold-ui-ink/notebook/synthetic.one");

fn first_page(bytes: &[u8]) -> (ExGuid, Page) {
    let store = Store::parse(bytes).unwrap();
    assert!(store.checksum_mismatches.is_empty());
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let (space, _) = document.pages().unwrap()[0];
    (space, Page::from_space(&document, space).unwrap())
}

fn drawings(page: &Page) -> Vec<&Ink> {
    page.objects
        .iter()
        .filter_map(|object| match object {
            PageObject::Ink(ink) => Some(ink),
            _ => None,
        })
        .collect()
}

fn close(actual: [f32; 4], expected: [f32; 4]) -> bool {
    actual
        .iter()
        .zip(expected)
        .all(|(a, e)| (a - e).abs() < 0.002)
}

#[test]
fn native_ink_drawings_read_as_strokes_in_page_coordinates() {
    let (_, page) = first_page(NATIVE);
    let found = drawings(&page);
    let [diamond, line] = found.as_slice() else {
        panic!("{:?}", found.len());
    };
    assert_eq!(diamond.strokes.len(), 1);
    assert_eq!(diamond.strokes[0].points.len(), 253);
    // OneNote reports a drawing's size one HIMETRIC unit larger than its point extent.
    let unit = 72.0 / 2540.0;
    let native = |[x, y, w, h]: [f32; 4]| [x, y, w + unit, h + unit];
    let bounds = (
        native(diamond.bounds().unwrap()),
        native(line.bounds().unwrap()),
    );
    assert!(
        close(bounds.0, [421.5118, 84.7559, 60.00945, 60.00944])
            && close(bounds.1, [504.0, 92.23936, 30.0189, 45.04252]),
        "{bounds:?}"
    );
    let stroke = &diamond.strokes[0];
    assert!(close(
        [stroke.points[0][0], stroke.points[0][1], 0.0, 0.0],
        [451.50235, 84.755905, 0.0, 0.0]
    ));
    assert!((stroke.width - 35.0 * 72.0 / 2540.0).abs() < 1e-5);
    assert_eq!(
        (stroke.color, stroke.transparency, stroke.pen_tip),
        (None, None, None)
    );
    assert!(diamond.groups.is_empty());
    for stroke in [&diamond.strokes[0], &line.strokes[0]] {
        assert!(stroke.points.windows(2).all(|pair| {
            let [dx, dy] = [pair[1][0] - pair[0][0], pair[1][1] - pair[0][1]];
            dx.abs() < 3.0 && dy.abs() < 3.0
        }));
    }
}

use onestore::page::ink::snap;

fn stroke(points: &[[f32; 2]], color: Option<u32>) -> InkStroke {
    InkStroke {
        id: new_id().unwrap(),
        points: points.iter().map(|p| [snap(p[0]), snap(p[1])]).collect(),
        width: snap(1.0),
        height: snap(1.0),
        color,
        transparency: None,
        pen_tip: None,
    }
}

fn drawing() -> Ink {
    Ink {
        id: new_id().unwrap(),
        layout: Default::default(),
        strokes: vec![
            stroke(
                &[
                    [300.0, 120.0],
                    [360.0, 120.0],
                    [360.0, 180.0],
                    [300.0, 180.0],
                    [300.0, 120.0],
                ],
                None,
            ),
            stroke(&[[300.0, 120.0], [360.0, 180.0]], Some(0x0000ff)),
        ],
        groups: Vec::new(),
    }
}

fn page_in(bytes: &[u8], space: ExGuid) -> Page {
    let store = Store::parse(bytes).unwrap();
    assert!(store.checksum_mismatches.is_empty());
    let index = RevisionIndex::parse(&store).unwrap();
    Page::from_space(&Document::parse(&index).unwrap(), space).unwrap()
}

fn export(variable: &str, name: &str, written: &[u8]) {
    if let Some(directory) = std::env::var_os(variable) {
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(directory.join(name), written).unwrap();
        let file_id = Store::parse(written).unwrap().header.file_id;
        std::fs::write(
            directory.join("Open Notebook.onetoc2"),
            onestore::create_table_of_contents("Open Notebook.onetoc2", &[(name, file_id)])
                .unwrap(),
        )
        .unwrap();
    }
}

/// `ONESTORE_INK_EXPORT` names a new directory receiving the candidate for a cold reopen.
#[test]
fn an_ink_drawing_is_written_erased_stroke_by_stroke_and_removed() {
    let source = onestore::create_section("ink.one", "Beside the drawing", "Author").unwrap();
    let (space, before) = first_page(&source);
    let mut after = before.clone();
    let ink = drawing();
    after.objects.push(PageObject::Ink(ink.clone()));
    let written = ops::saved(&source, space, &after).unwrap();
    let stored = page_in(written.as_slice(), space);
    let mut expected = after.clone();
    expected.title = stored.title.clone();
    assert_eq!(stored, expected);
    assert!(close(
        drawings(&stored)[0].bounds().unwrap(),
        [
            snap(300.0),
            snap(120.0),
            snap(360.0) - snap(300.0),
            snap(180.0) - snap(120.0)
        ]
    ));
    assert_eq!(
        ops::saved(written.as_slice(), space, &stored)
            .unwrap()
            .as_slice(),
        written.as_slice()
    );
    export("ONESTORE_INK_EXPORT", "ink.one", written.as_slice());
    let mut erased = stored.clone();
    for object in &mut erased.objects {
        if let PageObject::Ink(ink) = object {
            ink.strokes.remove(1);
            ink.strokes
                .push(stroke(&[[330.0, 100.0], [330.0, 200.0]], Some(0xff0000)));
        }
    }
    let again = ops::saved(written.as_slice(), space, &erased).unwrap();
    let stored = page_in(again.as_slice(), space);
    assert_eq!(stored, erased);
    let mut removed = stored.clone();
    removed.objects.retain(|object| object.id() != ink.id);
    let last = ops::saved(again.as_slice(), space, &removed).unwrap();
    let stored = page_in(last.as_slice(), space);
    assert!(drawings(&stored).is_empty());
    assert_eq!(stored, removed);
}

/// `ONESTORE_INK_PARAGRAPH_EXPORT` names a new directory receiving the candidate for a cold
/// reopen.
#[test]
fn handwriting_is_written_as_paragraph_content() {
    use onestore::page::{PageParagraph, ParagraphContent};
    let source = onestore::create_section("ink.one", "Above the handwriting", "Author").unwrap();
    let (space, before) = first_page(&source);
    let mut after = before.clone();
    let outline = after
        .objects
        .iter_mut()
        .find_map(|object| match object {
            PageObject::Outline(outline) if !outline.title => Some(outline),
            _ => None,
        })
        .unwrap();
    let mut paragraph: PageParagraph = outline.paragraphs[0].clone();
    paragraph.id = new_id().unwrap();
    paragraph.parent = None;
    paragraph.level = 1;
    paragraph.lists.clear();
    paragraph.tags.clear();
    paragraph.style = None;
    paragraph.collapsed = false;
    paragraph.format = Default::default();
    paragraph.content = ParagraphContent::Ink(drawing());
    outline.paragraphs.push(paragraph);
    let written = ops::saved(&source, space, &after).unwrap();
    let stored = page_in(written.as_slice(), space);
    let mut expected = after.clone();
    expected.title = stored.title.clone();
    assert_eq!(stored, expected);
    export(
        "ONESTORE_INK_PARAGRAPH_EXPORT",
        "ink.one",
        written.as_slice(),
    );
}

#[test]
fn stored_strokes_keep_their_paths() {
    let (space, page) = first_page(NATIVE);
    let mut moved = page.clone();
    for object in &mut moved.objects {
        if let PageObject::Ink(ink) = object {
            ink.strokes[0].points[0][0] += 1.0;
        }
    }
    assert!(ops::saved(NATIVE, space, &moved).is_err());
}
