use onestore::{
    ExGuid, RevisionIndex, Store,
    document::Document,
    page::{Ink, Page, PageObject},
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
